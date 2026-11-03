//! Transports that answer requests without a Harvest account, and the fixtures
//! they answer with.

pub mod fixture;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::client::{HarvestError, Method, Request, Response, Transport};

/// One response a [`StubTransport`] hands back.
#[derive(Clone)]
pub struct Scripted {
    pub status: u16,
    pub body: String,
    pub headers: Vec<(String, String)>,
}

impl Scripted {
    pub fn ok(body: &str) -> Self {
        Self::status(200, body)
    }

    pub fn status(status: u16, body: &str) -> Self {
        Self {
            status,
            body: body.to_string(),
            headers: Vec::new(),
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }
}

/// Hands back queued responses in order and records what was asked for, so tests
/// can assert on headers, paths and query strings.
pub struct StubTransport {
    scripted: Mutex<Vec<Scripted>>,
    requests: Mutex<Vec<Request>>,
}

impl StubTransport {
    pub fn new(scripted: Vec<Scripted>) -> Arc<Self> {
        Arc::new(Self {
            scripted: Mutex::new(scripted),
            requests: Mutex::new(Vec::new()),
        })
    }

    pub fn body(body: &str) -> Arc<Self> {
        Self::new(vec![Scripted::ok(body)])
    }

    pub fn request(&self, index: usize) -> Request {
        self.requests.lock().unwrap()[index].clone()
    }

    pub fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

#[async_trait]
impl Transport for StubTransport {
    async fn send(&self, request: Request) -> Result<Response, HarvestError> {
        self.requests.lock().unwrap().push(request);
        let mut scripted = self.scripted.lock().unwrap();
        if scripted.is_empty() {
            return Err(HarvestError::InvalidResponse);
        }
        let next = scripted.remove(0);
        Ok(Response {
            status: next.status,
            headers: next.headers,
            body: next.body.into_bytes(),
        })
    }
}

/// A route a [`RoutingTransport`] answers.
#[derive(Clone)]
pub struct Route {
    pub method: Option<Method>,
    pub fragment: String,
    pub body: String,
    pub status: u16,
}

impl Route {
    pub fn new(method: Method, fragment: &str, body: &str) -> Self {
        Self {
            method: Some(method),
            fragment: fragment.to_string(),
            body: body.to_string(),
            status: 200,
        }
    }

    pub fn status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }

    fn matches(&self, request: &Request) -> bool {
        self.method.is_none_or(|method| method == request.method)
            && request.url.as_str().contains(&self.fragment)
    }
}

/// What a [`RoutingTransport`] was asked, with the body decoded, so a test can
/// assert that a POST actually happened and with what, not merely that some request
/// did.
#[derive(Clone, Debug)]
pub struct Call {
    pub method: Method,
    pub path: String,
    pub hours: Option<f64>,
    pub project_id: Option<i64>,
    pub spent_date: Option<String>,
    /// The `Harvest-Account-Id` header: whose credentials carried the request.
    pub account_id: Option<String>,
}

/// Answers by HTTP method and URL rather than call order.
///
/// The tracker fires several requests concurrently, so their arrival order is not
/// fixed and an ordered stub would hand back the wrong bodies. Matching the method
/// matters too: `POST /time_entries` and `GET /time_entries` return quite different
/// shapes, and conflating them once made a test pass for the wrong reason.
pub struct RoutingTransport {
    routes: Vec<Route>,
    offline: AtomicBool,
    calls: Mutex<Vec<Call>>,
}

impl RoutingTransport {
    pub fn new(routes: Vec<Route>) -> Arc<Self> {
        Arc::new(Self {
            routes,
            offline: AtomicBool::new(false),
            calls: Mutex::new(Vec::new()),
        })
    }

    /// From now on every request fails as if the network were gone.
    pub fn go_offline(&self) {
        self.offline.store(true, Ordering::SeqCst);
    }

    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    pub fn call_count(&self, fragment: &str) -> usize {
        self.calls()
            .iter()
            .filter(|call| call.path.contains(fragment))
            .count()
    }

    pub fn calls_to(&self, method: Method, fragment: &str) -> Vec<Call> {
        self.calls()
            .into_iter()
            .filter(|call| call.method == method && call.path.contains(fragment))
            .collect()
    }

    pub fn first_index(&self, method: Method, fragment: &str) -> Option<usize> {
        self.calls()
            .iter()
            .position(|call| call.method == method && call.path.contains(fragment))
    }

    pub fn first_index_excluding(&self, method: Method, fragment: &str) -> Option<usize> {
        self.calls()
            .iter()
            .position(|call| call.method == method && !call.path.contains(fragment))
    }

    pub fn hours_at(&self, index: usize) -> Option<f64> {
        self.calls().get(index).and_then(|call| call.hours)
    }

    /// A working account: one project with one active task, plus writable timers.
    pub fn standard_account() -> Arc<Self> {
        Self::account(AccountOptions::default())
    }

    pub fn account(options: AccountOptions) -> Arc<Self> {
        let entries = options
            .entries
            .unwrap_or_else(|| vec![fixture::time_entry()]);
        Self::new(vec![
            Route::new(Method::Get, "reports/project_budget", &options.budgets)
                .status(options.budget_status),
            Route::new(Method::Get, "v2/clients", &options.clients).status(options.client_status),
            Route::new(
                Method::Get,
                "users/me/project_assignments",
                &options.assignments,
            ),
            Route::new(Method::Get, "users/me", &options.user),
            Route::new(Method::Get, "company", fixture::COMPANY),
            Route::new(Method::Patch, "/stop", &fixture::time_entry()),
            Route::new(Method::Patch, "/restart", &fixture::running_time_entry()),
            Route::new(Method::Post, "time_entries", &fixture::running_time_entry()).status(201),
            Route::new(Method::Get, "is_running=true", &fixture::entries_page(&[])),
            Route::new(
                Method::Get,
                "time_entries",
                &fixture::entries_page(&entries),
            ),
        ])
    }

    /// The same account, but with a timer already running.
    pub fn account_with_running_timer() -> Arc<Self> {
        let running = fixture::entries_page(&[fixture::running_time_entry()]);
        Self::new(vec![
            Route::new(
                Method::Get,
                "reports/project_budget",
                fixture::PROJECT_BUDGET_PAGE,
            ),
            Route::new(Method::Get, "v2/clients", fixture::CLIENTS_PAGE),
            Route::new(
                Method::Get,
                "users/me/project_assignments",
                fixture::PROJECT_ASSIGNMENTS_PAGE,
            ),
            Route::new(Method::Get, "users/me", fixture::CURRENT_USER),
            Route::new(Method::Get, "company", fixture::COMPANY),
            Route::new(Method::Get, "is_running=true", &running),
            Route::new(Method::Get, "time_entries", &running),
        ])
    }
}

/// What [`RoutingTransport::account`] answers with, where a test needs other than
/// the standard account.
pub struct AccountOptions {
    pub entries: Option<Vec<String>>,
    pub budgets: String,
    pub budget_status: u16,
    pub assignments: String,
    pub clients: String,
    pub client_status: u16,
    pub user: String,
}

impl Default for AccountOptions {
    fn default() -> Self {
        Self {
            entries: None,
            budgets: fixture::PROJECT_BUDGET_PAGE.to_string(),
            budget_status: 200,
            assignments: fixture::PROJECT_ASSIGNMENTS_PAGE.to_string(),
            clients: fixture::CLIENTS_PAGE.to_string(),
            client_status: 200,
            user: fixture::CURRENT_USER.to_string(),
        }
    }
}

#[async_trait]
impl Transport for RoutingTransport {
    async fn send(&self, request: Request) -> Result<Response, HarvestError> {
        let body = request.json();
        self.calls.lock().unwrap().push(Call {
            method: request.method,
            path: request.url.path().to_string(),
            hours: body.get("hours").and_then(|hours| hours.as_f64()),
            project_id: body.get("project_id").and_then(|id| id.as_i64()),
            spent_date: body
                .get("spent_date")
                .and_then(|date| date.as_str())
                .map(str::to_string),
            account_id: request.header("Harvest-Account-Id").map(str::to_string),
        });
        if self.offline.load(Ordering::SeqCst) {
            return Err(HarvestError::Network {
                reason: "The Internet connection appears to be offline.".to_string(),
            });
        }
        let route = self
            .routes
            .iter()
            .find(|route| route.matches(&request))
            .ok_or(HarvestError::NotFound)?;
        Ok(Response {
            status: route.status,
            headers: Vec::new(),
            body: route.body.clone().into_bytes(),
        })
    }
}

/// Passes requests to another transport, but parks those whose URL contains
/// `fragment` until the test releases them.
pub struct Held {
    inner: Arc<dyn Transport>,
    fragment: String,
    held: AtomicUsize,
    released: tokio::sync::watch::Sender<bool>,
}

impl Held {
    pub fn new(inner: Arc<dyn Transport>, fragment: &str) -> Arc<Self> {
        Arc::new(Self {
            inner,
            fragment: fragment.to_string(),
            held: AtomicUsize::new(0),
            released: tokio::sync::watch::channel(true).0,
        })
    }

    /// From now on, matching requests wait for `release`.
    pub fn hold(&self) {
        self.released.send_replace(false);
    }

    pub fn held(&self) -> usize {
        self.held.load(Ordering::SeqCst)
    }

    pub fn release(&self) {
        self.released.send_replace(true);
    }
}

#[async_trait]
impl Transport for Held {
    async fn send(&self, request: Request) -> Result<Response, HarvestError> {
        if request.url.as_str().contains(&self.fragment) && !*self.released.borrow() {
            self.held.fetch_add(1, Ordering::SeqCst);
            let mut released = self.released.subscribe();
            released
                .wait_for(|released| *released)
                .await
                .expect("the transport outlives its requests");
        }
        self.inner.send(request).await
    }
}

/// Parks every request until the test releases it, so a drain can be caught with a
/// request in flight, which is when the queue is free to be changed under it.
pub struct GatedTransport {
    arrived: AtomicUsize,
    released: tokio::sync::watch::Sender<bool>,
    body: String,
}

impl GatedTransport {
    pub fn new(body: &str) -> Arc<Self> {
        Arc::new(Self {
            arrived: AtomicUsize::new(0),
            released: tokio::sync::watch::channel(false).0,
            body: body.to_string(),
        })
    }

    pub fn arrived(&self) -> usize {
        self.arrived.load(Ordering::SeqCst)
    }

    pub fn release_all(&self) {
        self.released.send_replace(true);
    }
}

#[async_trait]
impl Transport for GatedTransport {
    async fn send(&self, _request: Request) -> Result<Response, HarvestError> {
        self.arrived.fetch_add(1, Ordering::SeqCst);
        let mut released = self.released.subscribe();
        released
            .wait_for(|released| *released)
            .await
            .expect("the transport outlives its requests");
        Ok(Response {
            status: 201,
            headers: Vec::new(),
            body: self.body.clone().into_bytes(),
        })
    }
}
