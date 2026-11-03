//! A typed client for the Harvest V2 API.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;
use serde::de::DeserializeOwned;
use url::Url;

use crate::budget::ProjectBudget;
use crate::calendar_date::CalendarDate;
use crate::models::{
    ClientRecord, HarvestCompany, HarvestUser, Page, PaginatedItem, ProjectAssignment, TimeEntry,
    decode_described,
};

pub const BASE_URL: &str = "https://api.harvestapp.com/v2/";
/// Harvest answers 400 unless a request identifies its client.
pub const USER_AGENT: &str = "Sheaves (https://github.com/rainhead/sheaves)";

/// A Harvest personal access token plus the account it addresses.
///
/// One token can reach several Harvest and Forecast accounts, so the account id
/// travels with it: Harvest requires both on every request. Create one at
/// <https://id.getharvest.com/developers>.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
pub struct Credentials {
    pub account_id: String,
    pub token: String,
}

impl Credentials {
    /// Trims whitespace pasted around either value.
    pub fn new(account_id: impl AsRef<str>, token: impl AsRef<str>) -> Self {
        Self {
            account_id: account_id.as_ref().trim().to_string(),
            token: token.as_ref().trim().to_string(),
        }
    }

    pub fn is_complete(&self) -> bool {
        !self.account_id.is_empty() && !self.token.is_empty()
    }
}

/// Never prints the token, so credentials can appear in a log line safely.
impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("account_id", &self.account_id)
            .field("token", &"…")
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum HarvestError {
    /// No credentials have been supplied yet.
    #[error("Sheaves isn’t connected to Harvest yet.")]
    NotConfigured,
    /// 401: the token or account id is wrong, or the token was revoked. Nothing
    /// will succeed until the credentials change.
    #[error("Harvest rejected the token. Check the token and account ID.")]
    Unauthorized,
    /// 403: the token is good, but may not do this. Harvest answers this way for a
    /// report or a record the user's role does not reach, so it refuses one request
    /// rather than the account.
    #[error("Harvest says this account may not do that.")]
    Forbidden,
    /// 429: Harvest allows 100 requests per 15 seconds.
    #[error("Harvest is rate limiting requests. Retrying in {}s.", .retry_after.round())]
    RateLimited { retry_after: f64 },
    #[error("That record no longer exists in Harvest.")]
    NotFound,
    /// 422 and friends; `message` is Harvest's own explanation where it gave one.
    #[error("{}", if .message.is_empty() { "Harvest rejected the change." } else { .message.as_str() })]
    Rejected { status: u16, message: String },
    #[error("Harvest returned an error ({status}). Try again shortly.")]
    Server { status: u16 },
    /// The response was not HTTP at all.
    #[error("Harvest returned something Sheaves couldn’t read.")]
    InvalidResponse,
    /// The response was well-formed HTTP that did not match the expected shape.
    /// `detail` names the field and path, which is the whole point of the case.
    #[error("Couldn’t read Harvest’s {endpoint} response: {detail}")]
    Decoding { endpoint: String, detail: String },
    /// The request never got an answer: no network, a timeout, a refused TLS
    /// handshake. `reason` is the transport's own description.
    #[error("{reason}")]
    Network { reason: String },
}

impl HarvestError {
    /// Whether retrying the identical request could plausibly succeed.
    pub fn is_transient(&self) -> bool {
        match self {
            HarvestError::RateLimited { .. }
            | HarvestError::Server { .. }
            | HarvestError::InvalidResponse
            | HarvestError::Network { .. } => true,
            // Retrying cannot change a response shape, and for a queued write the
            // change already landed: only reading the reply failed.
            HarvestError::NotConfigured
            | HarvestError::Unauthorized
            | HarvestError::Forbidden
            | HarvestError::NotFound
            | HarvestError::Rejected { .. }
            | HarvestError::Decoding { .. } => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Patch,
    Delete,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
        }
    }
}

/// An HTTP request, as the client hands it to a transport.
#[derive(Clone, Debug)]
pub struct Request {
    pub method: Method,
    pub url: Url,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The JSON body, for tests that assert what was sent.
    pub fn json(&self) -> serde_json::Value {
        self.body
            .as_deref()
            .and_then(|body| serde_json::from_slice(body).ok())
            .unwrap_or(serde_json::Value::Null)
    }
}

#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// The seam between the client and the network, so tests can answer requests
/// without a live Harvest account.
#[async_trait]
pub trait Transport: Send + Sync {
    async fn send(&self, request: Request) -> Result<Response, HarvestError>;
}

/// The real network, through reqwest with rustls, so a Linux build needs no OpenSSL.
pub struct HttpTransport {
    client: reqwest::Client,
}

impl HttpTransport {
    pub fn new() -> Self {
        Self {
            // A connection that dies silently, on a sleeping laptop or behind a
            // captive portal, would otherwise hold the request until the operating
            // system gives up, and every sync and queued change waits behind it. A
            // timeout is a network failure, which the queue retries.
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .build()
                .expect("rustls is compiled in"),
        }
    }
}

impl Default for HttpTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Transport for HttpTransport {
    async fn send(&self, request: Request) -> Result<Response, HarvestError> {
        let method = reqwest::Method::from_bytes(request.method.as_str().as_bytes())
            .expect("a standard method");
        let mut builder = self.client.request(method, request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        if let Some(body) = request.body {
            builder = builder.body(body);
        }
        let response = builder
            .send()
            .await
            .map_err(|error| HarvestError::Network {
                reason: error.to_string(),
            })?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.to_string(), value.to_string()))
            })
            .collect();
        let body = response
            .bytes()
            .await
            .map_err(|error| HarvestError::Network {
                reason: error.to_string(),
            })?
            .to_vec();
        Ok(Response {
            status,
            headers,
            body,
        })
    }
}

/// A typed client for the Harvest V2 API.
///
/// Harvest allows 100 requests per 15 seconds and answers 429 with `Retry-After`;
/// the client honours that header and retries rather than surfacing the failure, so
/// callers only see a `RateLimited` error once retries are exhausted.
pub struct HarvestClient {
    transport: Arc<dyn Transport>,
    credentials: Mutex<Option<Credentials>>,
    /// Scales the wait between retries. Tests set it to zero; nothing else should.
    backoff_scale: f64,
}

const MAX_RETRIES: u32 = 3;
/// The longest `Retry-After` the client waits out itself. A longer one goes back to
/// the caller as `RateLimited`, rather than holding every sync behind it; the Reports
/// API, at 100 requests per 15 minutes, can ask for many minutes.
const MAX_RETRY_WAIT: f64 = 60.0;

impl HarvestClient {
    pub fn new(
        credentials: Option<Credentials>,
        transport: Arc<dyn Transport>,
        backoff_scale: f64,
    ) -> Self {
        Self {
            transport,
            credentials: Mutex::new(credentials),
            backoff_scale,
        }
    }

    pub fn set_credentials(&self, credentials: Option<Credentials>) {
        *self.credentials.lock().unwrap() = credentials;
    }

    pub fn is_configured(&self) -> bool {
        self.credentials
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(Credentials::is_complete)
    }

    /// What is installed now, so a caller trying out new credentials can put these
    /// back.
    pub fn installed_credentials(&self) -> Option<Credentials> {
        self.credentials.lock().unwrap().clone()
    }

    // MARK: Account

    pub async fn current_user(&self) -> Result<HarvestUser, HarvestError> {
        self.get("users/me", &[]).await
    }

    pub async fn company(&self) -> Result<HarvestCompany, HarvestError> {
        self.get("company", &[]).await
    }

    /// Every project and task pair the current user is allowed to log time against.
    pub async fn project_assignments(&self) -> Result<Vec<ProjectAssignment>, HarvestError> {
        self.get_all_pages("users/me/project_assignments", &[])
            .await
    }

    // MARK: Time entries

    pub async fn time_entries(
        &self,
        user_id: i64,
        from: CalendarDate,
        to: CalendarDate,
    ) -> Result<Vec<TimeEntry>, HarvestError> {
        self.get_all_pages(
            "time_entries",
            &[
                ("user_id", user_id.to_string()),
                ("from", from.to_string()),
                ("to", to.to_string()),
            ],
        )
        .await
    }

    /// The user's running timer, if any. Harvest permits at most one.
    pub async fn running_time_entry(
        &self,
        user_id: i64,
    ) -> Result<Option<TimeEntry>, HarvestError> {
        let url = Self::url(
            "time_entries",
            &[
                ("user_id", user_id.to_string()),
                ("is_running", "true".to_string()),
            ],
        );
        let data = self.perform(self.request(url, Method::Get)?).await?;
        let page = Page::<TimeEntry>::decode(&data).map_err(|detail| HarvestError::Decoding {
            endpoint: "time_entries".to_string(),
            detail,
        })?;
        Ok(page.items.into_iter().next())
    }

    /// Creates a time entry.
    ///
    /// Omitting `hours` starts the timer running: that is what an absent `hours`
    /// means on duration-tracking accounts, and an absent `ended_time` on timestamp
    /// accounts, so one body covers both. Passing `hours` creates a stopped entry of
    /// exactly that length, which is how work done offline is recorded accurately
    /// rather than being measured from whenever the request happens to arrive.
    pub async fn create_time_entry(
        &self,
        project_id: i64,
        task_id: i64,
        spent_date: CalendarDate,
        notes: Option<&str>,
        hours: Option<f64>,
    ) -> Result<TimeEntry, HarvestError> {
        #[derive(Serialize)]
        struct Body<'a> {
            project_id: i64,
            task_id: i64,
            spent_date: CalendarDate,
            #[serde(skip_serializing_if = "Option::is_none")]
            notes: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            hours: Option<f64>,
        }
        self.send(
            "time_entries",
            Method::Post,
            &Body {
                project_id,
                task_id,
                spent_date,
                notes,
                hours,
            },
        )
        .await
    }

    pub async fn stop_time_entry(&self, id: i64) -> Result<TimeEntry, HarvestError> {
        self.send(&format!("time_entries/{id}/stop"), Method::Patch, &Empty {})
            .await
    }

    pub async fn restart_time_entry(&self, id: i64) -> Result<TimeEntry, HarvestError> {
        self.send(
            &format!("time_entries/{id}/restart"),
            Method::Patch,
            &Empty {},
        )
        .await
    }

    pub async fn update_time_entry(
        &self,
        id: i64,
        update: EntryUpdate<'_>,
    ) -> Result<TimeEntry, HarvestError> {
        self.send(&format!("time_entries/{id}"), Method::Patch, &update)
            .await
    }

    pub async fn delete_time_entry(&self, id: i64) -> Result<(), HarvestError> {
        let url = Self::url(&format!("time_entries/{id}"), &[]);
        self.perform(self.request(url, Method::Delete)?).await?;
        Ok(())
    }

    /// Clients, for their currencies.
    ///
    /// Readable only by an administrator or a manager who may edit clients; anyone
    /// else gets 403. That is nearly the same permission a monetary budget already
    /// requires, so a token that can see money can usually also see what kind.
    pub async fn clients(&self) -> Result<Vec<ClientRecord>, HarvestError> {
        self.get_all_pages("clients", &[]).await
    }

    // MARK: Reports

    /// Every project's budget, spend and remainder in one request.
    ///
    /// This is the Reports API, which allows 100 requests per 15 *minutes*, sixty
    /// times tighter than the rest of Harvest. The client does not pace this; the
    /// caller must.
    pub async fn project_budgets(&self) -> Result<Vec<ProjectBudget>, HarvestError> {
        self.get_all_pages("reports/project_budget", &[]).await
    }

    // MARK: Request plumbing

    async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, HarvestError> {
        let data = self
            .perform(self.request(Self::url(path, query), Method::Get)?)
            .await?;
        decode(&data, path)
    }

    /// Follows Harvest's own `links.next` rather than counting pages.
    ///
    /// Harvest's documentation is explicit that pagination URLs should be followed
    /// rather than constructed, and the reason bites here: a cursor-paginated
    /// response returns null for `page`, `next_page` and `previous_page` on every
    /// page but the first and last. Walking page numbers therefore stops after one
    /// page and drops the rest without any error to notice. Page numbers remain the
    /// fallback, for a response that carries no links.
    async fn get_all_pages<Item: PaginatedItem>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Vec<Item>, HarvestError> {
        let mut query: Vec<(&str, String)> = query.to_vec();
        query.push(("per_page", "2000".to_string()));
        query.push(("page", "1".to_string()));

        let mut collected = Vec::new();
        let mut next = Some(Self::url(path, &query));
        // A malformed or looping `links.next` would otherwise fetch forever.
        let mut visited = HashSet::new();

        while let Some(url) = next.take() {
            if !visited.insert(url.clone()) {
                break;
            }
            let data = self.perform(self.request(url, Method::Get)?).await?;
            let page = Page::<Item>::decode(&data).map_err(|detail| HarvestError::Decoding {
                endpoint: path.to_string(),
                detail,
            })?;
            collected.extend(page.items);
            next = match (page.next_link, page.next_page) {
                // The token goes with every request, so it follows a link only while
                // the link stays on the API.
                (Some(link), _) if is_api_url(&link) => Some(link),
                (Some(link), _) => {
                    return Err(HarvestError::Decoding {
                        endpoint: path.to_string(),
                        detail: format!("links.next points outside the API: {link}"),
                    });
                }
                (None, Some(number)) => {
                    let mut query = query.clone();
                    query.retain(|(key, _)| *key != "page");
                    query.push(("page", number.to_string()));
                    Some(Self::url(path, &query))
                }
                (None, None) => None,
            };
        }
        Ok(collected)
    }

    async fn send<T: DeserializeOwned>(
        &self,
        path: &str,
        method: Method,
        body: &impl Serialize,
    ) -> Result<T, HarvestError> {
        let mut request = self.request(Self::url(path, &[]), method)?;
        request.body = Some(serde_json::to_vec(body).expect("request bodies always encode"));
        request
            .headers
            .push(("Content-Type".to_string(), "application/json".to_string()));
        let data = self.perform(request).await?;
        decode(&data, path)
    }

    /// The URL for `path` under the API's base, with `query` in a stable order.
    pub fn url(path: &str, query: &[(&str, String)]) -> Url {
        let mut url = Url::parse(BASE_URL)
            .and_then(|base| base.join(path))
            .expect("paths are relative and well formed");
        if !query.is_empty() {
            let sorted: BTreeMap<&str, &str> = query
                .iter()
                .map(|(key, value)| (*key, value.as_str()))
                .collect();
            url.query_pairs_mut().extend_pairs(sorted);
        }
        url
    }

    /// Headers for a URL, including one Harvest handed us, which is how pagination
    /// is followed.
    fn request(&self, url: Url, method: Method) -> Result<Request, HarvestError> {
        let credentials = self
            .credentials
            .lock()
            .unwrap()
            .clone()
            .filter(Credentials::is_complete)
            .ok_or(HarvestError::NotConfigured)?;
        Ok(Request {
            method,
            url,
            headers: vec![
                (
                    "Authorization".to_string(),
                    format!("Bearer {}", credentials.token),
                ),
                ("Harvest-Account-Id".to_string(), credentials.account_id),
                ("User-Agent".to_string(), USER_AGENT.to_string()),
                ("Accept".to_string(), "application/json".to_string()),
            ],
            body: None,
        })
    }

    async fn perform(&self, request: Request) -> Result<Vec<u8>, HarvestError> {
        let mut attempt = 0;
        loop {
            let response = self.transport.send(request.clone()).await?;
            match response.status {
                200..=299 => return Ok(response.body),
                401 => return Err(HarvestError::Unauthorized),
                403 => return Err(HarvestError::Forbidden),
                404 => return Err(HarvestError::NotFound),
                429 => {
                    let retry_after = retry_after(&response);
                    if attempt >= MAX_RETRIES || retry_after > MAX_RETRY_WAIT {
                        return Err(HarvestError::RateLimited { retry_after });
                    }
                    attempt += 1;
                    self.pause(retry_after).await;
                }
                status @ 500.. => {
                    // Never retry a POST: the server may have committed the change
                    // before failing, and a second create means a duplicate time
                    // entry. Let the caller decide, with the request left queued.
                    if request.method == Method::Post || attempt >= MAX_RETRIES {
                        return Err(HarvestError::Server { status });
                    }
                    attempt += 1;
                    self.pause(f64::from(attempt)).await;
                }
                status => {
                    return Err(HarvestError::Rejected {
                        status,
                        message: error_message(&response.body),
                    });
                }
            }
        }
    }

    async fn pause(&self, seconds: f64) {
        let seconds = seconds * self.backoff_scale;
        if seconds > 0.0 {
            tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
        }
    }
}

/// The fields of a time entry an update may change. `None` leaves a field alone.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct EntryUpdate<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spent_date: Option<CalendarDate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hours: Option<f64>,
}

#[derive(Serialize)]
struct Empty {}

fn decode<T: DeserializeOwned>(data: &[u8], endpoint: &str) -> Result<T, HarvestError> {
    decode_described(data).map_err(|detail| HarvestError::Decoding {
        endpoint: endpoint.to_string(),
        detail,
    })
}

fn is_api_url(url: &Url) -> bool {
    let base = Url::parse(BASE_URL).expect("BASE_URL is well formed");
    url.origin() == base.origin() && url.path().starts_with(base.path())
}

/// Seconds Harvest asks us to wait, or its own window of fifteen when the header is
/// missing or unreadable.
pub(crate) fn retry_after(response: &Response) -> f64 {
    response
        .header("Retry-After")
        .and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .unwrap_or(15.0)
}

/// Harvest reports validation failures as `{"message": "..."}` or a field-keyed map.
fn error_message(data: &[u8]) -> String {
    let Ok(serde_json::Value::Object(object)) = serde_json::from_slice(data) else {
        return String::new();
    };
    if let Some(serde_json::Value::String(message)) = object.get("message") {
        return message.clone();
    }
    let mut parts: Vec<String> = object
        .iter()
        .map(|(key, value)| match value {
            serde_json::Value::String(text) => format!("{key}: {text}"),
            other => format!("{key}: {other}"),
        })
        .collect();
    parts.sort();
    parts.join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar_date::Weekday;
    use crate::hours::HoursFormat;
    use crate::test_support::fixture::{self, today};
    use crate::test_support::{Scripted, StubTransport};
    use crate::tracked_entry::timer_targets;

    fn client(transport: &Arc<StubTransport>) -> HarvestClient {
        HarvestClient::new(Some(fixture::credentials()), transport.clone(), 0.0)
    }

    mod harvest_client {
        use super::*;

        #[tokio::test]
        async fn authentication_headers() {
            let transport = StubTransport::body(fixture::CURRENT_USER);
            client(&transport).current_user().await.unwrap();

            let request = transport.request(0);
            assert_eq!(request.header("Authorization"), Some("Bearer sekrit"));
            assert_eq!(request.header("Harvest-Account-Id"), Some("12345"));
            // Harvest answers 400 to any request without a User-Agent.
            assert_eq!(request.header("User-Agent"), Some(USER_AGENT));
            assert_eq!(
                request.url.as_str(),
                "https://api.harvestapp.com/v2/users/me"
            );
        }

        #[tokio::test]
        async fn requires_credentials() {
            let client = HarvestClient::new(None, StubTransport::body("{}"), 0.0);
            assert_eq!(
                client.current_user().await,
                Err(HarvestError::NotConfigured)
            );
        }

        #[tokio::test]
        async fn decodes_user() {
            let user = client(&StubTransport::body(fixture::CURRENT_USER))
                .current_user()
                .await
                .unwrap();
            assert_eq!(user.id, 1782959);
            assert_eq!(user.name(), "Kim Allen");
        }

        #[tokio::test]
        async fn decodes_company() {
            let company = client(&StubTransport::body(fixture::COMPANY))
                .company()
                .await
                .unwrap();
            assert_eq!(company.week_start_day, Weekday::Monday);
            assert!(!company.wants_timestamp_timers);
            assert_eq!(
                HoursFormat::from_time_format(Some(&company.time_format)),
                HoursFormat::HoursMinutes
            );
        }

        #[tokio::test]
        async fn decodes_time_entry() {
            let body =
                fixture::entries_page(&[fixture::time_entry(), fixture::running_time_entry()]);
            let day = CalendarDate::new(2026, 8, 29);
            let entries = client(&StubTransport::body(&body))
                .time_entries(1782959, day, day)
                .await
                .unwrap();

            assert_eq!(entries.len(), 2);
            assert_eq!(entries[0].project.name, "Marketing Website");
            assert_eq!(entries[0].spent_date, today());
            assert_eq!(entries[0].notes.as_deref(), Some("Adding CSS styling"));
            assert!(!entries[0].is_running);
            assert_eq!(entries[1].notes, None);
            assert!(entries[1].is_running);
            assert!(entries[1].timer_started_at.is_some());
        }

        #[tokio::test]
        async fn scopes_query() {
            let transport = StubTransport::body(&fixture::entries_page(&[]));
            client(&transport)
                .time_entries(
                    1782959,
                    CalendarDate::new(2026, 8, 1),
                    CalendarDate::new(2026, 8, 31),
                )
                .await
                .unwrap();

            let query: BTreeMap<String, String> = transport
                .request(0)
                .url
                .query_pairs()
                .into_owned()
                .collect();
            // A personal access token belonging to an admin would otherwise return
            // the whole team's entries.
            assert_eq!(query["user_id"], "1782959");
            assert_eq!(query["from"], "2026-08-01");
            assert_eq!(query["to"], "2026-08-31");
        }

        #[tokio::test]
        async fn follows_pagination() {
            let transport = StubTransport::new(vec![
                Scripted::ok(&fixture::time_entries_page(
                    &[fixture::time_entry()],
                    Some(2),
                    1,
                    2,
                    None,
                )),
                Scripted::ok(&fixture::time_entries_page(
                    &[fixture::running_time_entry()],
                    None,
                    2,
                    2,
                    None,
                )),
            ]);
            let entries = client(&transport)
                .time_entries(1782959, today(), today())
                .await
                .unwrap();

            assert_eq!(entries.len(), 2);
            assert_eq!(transport.request_count(), 2);
            assert!(transport.request(1).url.as_str().contains("page=2"));
        }

        #[tokio::test]
        async fn retries_on_rate_limit() {
            let transport = StubTransport::new(vec![
                Scripted::status(429, "").with_header("Retry-After", "0"),
                Scripted::ok(fixture::CURRENT_USER),
            ]);
            let user = client(&transport).current_user().await.unwrap();
            assert_eq!(user.id, 1782959);
            assert_eq!(transport.request_count(), 2);
        }

        #[tokio::test]
        async fn gives_up_on_persistent_rate_limit() {
            let transport = StubTransport::new(
                (0..6)
                    .map(|_| Scripted::status(429, "").with_header("Retry-After", "0"))
                    .collect(),
            );
            assert_eq!(
                client(&transport).current_user().await,
                Err(HarvestError::RateLimited { retry_after: 0.0 })
            );
        }

        /// The Swift client mapped both to one error. They mean different things to
        /// the queue: a rejected token says nothing will get through, while a
        /// refusal says only this request will not.
        /// A wait too long to sit through, or too large to sleep for at all, goes
        /// straight back to the caller.
        #[tokio::test]
        async fn hands_back_a_long_wait() {
            for wait in ["600", "1e300"] {
                let transport = StubTransport::new(vec![
                    Scripted::status(429, "").with_header("Retry-After", wait),
                    Scripted::ok(fixture::CURRENT_USER),
                ]);
                let client =
                    HarvestClient::new(Some(fixture::credentials()), transport.clone(), 1.0);
                assert!(matches!(
                    client.current_user().await,
                    Err(HarvestError::RateLimited { .. })
                ));
                assert_eq!(transport.request_count(), 1);
            }
        }

        #[tokio::test]
        async fn maps_unauthorized() {
            for (status, error) in [
                (401, HarvestError::Unauthorized),
                (403, HarvestError::Forbidden),
            ] {
                let transport = StubTransport::new(vec![Scripted::status(status, "")]);
                assert_eq!(client(&transport).current_user().await, Err(error));
            }
        }

        #[tokio::test]
        async fn surfaces_rejection_message() {
            let transport = StubTransport::new(vec![Scripted::status(
                422,
                r#"{"message": "Time entry is locked."}"#,
            )]);
            assert_eq!(
                client(&transport).stop_time_entry(1).await,
                Err(HarvestError::Rejected {
                    status: 422,
                    message: "Time entry is locked.".to_string()
                })
            );
        }

        #[tokio::test]
        async fn starts_timer() {
            let transport =
                StubTransport::new(vec![Scripted::status(201, &fixture::running_time_entry())]);
            client(&transport)
                .create_time_entry(
                    14308069,
                    8083366,
                    CalendarDate::new(2026, 8, 29),
                    Some("spike"),
                    None,
                )
                .await
                .unwrap();

            let request = transport.request(0);
            assert_eq!(request.method, Method::Post);
            let json = request.json();
            assert_eq!(json["project_id"], 14308069);
            assert_eq!(json["task_id"], 8083366);
            assert_eq!(json["spent_date"], "2026-08-29");
            assert_eq!(json["notes"], "spike");
            // Omitting both is what makes Harvest start the clock, on
            // duration-tracking accounts and timestamp accounts alike.
            assert!(json.get("hours").is_none());
            assert!(json.get("ended_time").is_none());
        }

        /// Work done offline is recorded as an explicit length, so it does not
        /// depend on when the request happens to arrive.
        #[tokio::test]
        async fn creates_with_explicit_hours() {
            let transport = StubTransport::new(vec![Scripted::status(201, &fixture::time_entry())]);
            client(&transport)
                .create_time_entry(1, 2, today(), None, Some(0.5))
                .await
                .unwrap();
            assert_eq!(transport.request(0).json()["hours"], 0.5);
        }

        /// A POST may have committed before the server failed; sending it again
        /// would create a second time entry.
        #[tokio::test]
        async fn does_not_retry_post() {
            let transport = StubTransport::new((0..4).map(|_| Scripted::status(500, "")).collect());
            assert_eq!(
                client(&transport)
                    .create_time_entry(1, 2, today(), None, None)
                    .await,
                Err(HarvestError::Server { status: 500 })
            );
            assert_eq!(transport.request_count(), 1);
        }

        #[tokio::test]
        async fn stop_and_restart() {
            let transport = StubTransport::new(vec![
                Scripted::ok(&fixture::time_entry()),
                Scripted::ok(&fixture::running_time_entry()),
            ]);
            let client = client(&transport);
            client.stop_time_entry(636709355).await.unwrap();
            client.restart_time_entry(636709355).await.unwrap();

            let stop = transport.request(0);
            assert_eq!(stop.method, Method::Patch);
            assert_eq!(stop.url.path(), "/v2/time_entries/636709355/stop");
            assert_eq!(
                transport.request(1).url.path(),
                "/v2/time_entries/636709355/restart"
            );
        }

        #[tokio::test]
        async fn fetches_running_entry() {
            let transport =
                StubTransport::body(&fixture::entries_page(&[fixture::running_time_entry()]));
            let running = client(&transport)
                .running_time_entry(1782959)
                .await
                .unwrap();
            assert_eq!(running.map(|entry| entry.id), Some(636708906));
            assert!(
                transport
                    .request(0)
                    .url
                    .as_str()
                    .contains("is_running=true")
            );
        }
    }

    mod project_assignments {
        use super::*;

        #[tokio::test]
        async fn decodes() {
            let assignments = client(&StubTransport::body(fixture::PROJECT_ASSIGNMENTS_PAGE))
                .project_assignments()
                .await
                .unwrap();
            assert_eq!(assignments.len(), 2);
            assert_eq!(assignments[0].project.code.as_deref(), Some("OS1"));
            assert_eq!(assignments[0].task_assignments.len(), 2);
        }

        /// Offering an archived project or a retired task would just produce a 422
        /// on start.
        #[tokio::test]
        async fn filters_inactive() {
            let assignments = client(&StubTransport::body(fixture::PROJECT_ASSIGNMENTS_PAGE))
                .project_assignments()
                .await
                .unwrap();
            let targets = timer_targets(&assignments);
            assert_eq!(targets.len(), 1);
            assert_eq!(targets[0].project.name, "Online Store - Phase 1");
            assert_eq!(targets[0].task.name, "Graphic Design");
            assert_eq!(
                targets[0].project_label(),
                "123 Industries · Online Store - Phase 1"
            );
            assert_eq!(targets[0].id(), "14308069:8083365");
        }
    }

    mod pagination {
        use super::*;

        /// The failure this guards against is silent. Harvest returns null for
        /// `page`, `next_page` and `previous_page` on every page but the first and
        /// last of a cursor-paginated response, so a client that walks page numbers
        /// collects the first page, sees no next page, and reports success having
        /// dropped the rest.
        #[tokio::test]
        async fn follows_cursor_links() {
            let cursor = "https://api.harvestapp.com/v2/time_entries?cursor=abc123&per_page=2000";
            let transport = StubTransport::new(vec![
                Scripted::ok(&fixture::time_entries_page(
                    &[fixture::time_entry()],
                    None,
                    1,
                    1,
                    Some(cursor),
                )),
                Scripted::ok(&fixture::time_entries_page(
                    &[fixture::running_time_entry()],
                    None,
                    1,
                    1,
                    None,
                )),
            ]);
            let entries = client(&transport)
                .time_entries(1782959, today(), today())
                .await
                .unwrap();

            assert_eq!(entries.len(), 2);
            // The second request went to Harvest's own URL, cursor and all.
            assert_eq!(transport.request(1).url.as_str(), cursor);
            // And it still carries the headers Harvest requires.
            assert_eq!(
                transport.request(1).header("Harvest-Account-Id"),
                Some("12345")
            );
        }

        /// Page numbers stay the fallback for a response with no links section.
        #[tokio::test]
        async fn falls_back_to_page_numbers() {
            let transport = StubTransport::new(vec![
                Scripted::ok(&fixture::time_entries_page(
                    &[fixture::time_entry()],
                    Some(2),
                    1,
                    2,
                    None,
                )),
                Scripted::ok(&fixture::time_entries_page(
                    &[fixture::running_time_entry()],
                    None,
                    2,
                    2,
                    None,
                )),
            ]);
            let entries = client(&transport)
                .time_entries(1782959, today(), today())
                .await
                .unwrap();
            assert_eq!(entries.len(), 2);
            assert!(transport.request(1).url.as_str().contains("page=2"));
        }

        /// The token rides on every request, so a link off the API is not followed.
        #[tokio::test]
        async fn refuses_a_link_off_the_api() {
            let elsewhere = "https://example.com/v2/time_entries?page=2";
            let transport = StubTransport::new(vec![Scripted::ok(&fixture::time_entries_page(
                &[fixture::time_entry()],
                None,
                1,
                1,
                Some(elsewhere),
            ))]);
            let result = client(&transport)
                .time_entries(1782959, today(), today())
                .await;
            assert!(matches!(result, Err(HarvestError::Decoding { .. })));
            assert_eq!(transport.request_count(), 1);
        }

        /// A link pointing back at a page already fetched would otherwise never end.
        #[tokio::test]
        async fn stops_on_a_loop() {
            let day = today();
            let itself = format!(
                "https://api.harvestapp.com/v2/time_entries?from={day}&page=1&per_page=2000&to={day}&user_id=1782959"
            );
            let page =
                fixture::time_entries_page(&[fixture::time_entry()], None, 1, 1, Some(&itself));
            let transport = StubTransport::new(vec![Scripted::ok(&page), Scripted::ok(&page)]);
            let entries = client(&transport)
                .time_entries(1782959, day, day)
                .await
                .unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(transport.request_count(), 1);
        }
    }

    mod budgets {
        use super::*;
        use crate::budget::BudgetBasis;

        #[tokio::test]
        async fn decodes_report() {
            let budgets = client(&StubTransport::body(fixture::PROJECT_BUDGET_PAGE))
                .project_budgets()
                .await
                .unwrap();

            assert_eq!(budgets.len(), 3);
            let hours = budgets
                .iter()
                .find(|budget| budget.project_id == 14308069)
                .unwrap();
            assert_eq!(hours.project_name, "Online Store - Phase 1");
            assert_eq!(hours.budget_by, BudgetBasis::Project);
            assert_eq!(hours.budget, Some(40.0));
            assert_eq!(hours.budget_spent, Some(32.0));
            assert_eq!(hours.budget_remaining, Some(8.0));
            assert!(hours.has_readable_budget());

            // `budget_by: none` is a project with no budget at all.
            let none = budgets
                .iter()
                .find(|budget| budget.project_id == 14307913)
                .unwrap();
            assert!(!none.has_readable_budget());
            // A monetary budget this token may not read comes back with null
            // figures. It is indistinguishable from the case above, and means the
            // same to the UI.
            let unreadable = budgets
                .iter()
                .find(|budget| budget.project_id == 14307915)
                .unwrap();
            assert_eq!(unreadable.budget_by, BudgetBasis::TaskFees);
            assert!(unreadable.budget_is_monthly);
            assert!(!unreadable.has_readable_budget());
        }

        #[tokio::test]
        async fn requests_the_report() {
            let transport = StubTransport::body(fixture::PROJECT_BUDGET_PAGE);
            client(&transport).project_budgets().await.unwrap();
            let url = transport.request(0).url.to_string();
            assert!(url.contains("/v2/reports/project_budget"));
            assert!(url.contains("page=1"));
        }
    }

    mod retry_after {
        use super::*;

        fn response(header: Option<&str>) -> Response {
            Response {
                status: 429,
                headers: header
                    .map(|value| vec![("Retry-After".to_string(), value.to_string())])
                    .unwrap_or_default(),
                body: Vec::new(),
            }
        }

        #[test]
        fn reads_header() {
            assert_eq!(retry_after(&response(Some("15"))), 15.0);
            assert_eq!(retry_after(&response(Some("0"))), 0.0);
        }

        #[test]
        fn falls_back() {
            assert_eq!(retry_after(&response(None)), 15.0);
            assert_eq!(retry_after(&response(Some("soon"))), 15.0);
        }
    }
}
