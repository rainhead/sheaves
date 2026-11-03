//! Answering peers: `GET /presence` and `POST /touched`, on this machine's tailnet
//! addresses only.

use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use jiff::Timestamp;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use super::{Peers, Refusal, Touched, is_tailnet, tailnet_addresses};

/// The largest request body read. A notice of changed entries is a few dozen bytes.
const BODY_LIMIT: usize = 16 * 1024;

/// How often the listener looks for tailnet addresses that appeared or went:
/// Tailscale started after the app, or a new address after a reconnect.
const RECHECK: Duration = Duration::from_secs(60);

/// Listens on every tailnet address this machine has, for as long as it lives.
/// Dropping it stops listening.
pub struct Listener {
    task: JoinHandle<()>,
    bound: Arc<std::sync::Mutex<Vec<SocketAddr>>>,
}

impl Listener {
    /// Starts listening on `port` at each of this machine's tailnet addresses, now
    /// and as they change. Must be called inside a Tokio runtime.
    pub fn start(peers: Arc<Peers>, port: u16) -> Self {
        let bound = Arc::new(std::sync::Mutex::new(Vec::new()));
        let task = tokio::spawn(keep_listening(peers, port, bound.clone()));
        Self { task, bound }
    }

    /// The addresses being listened on now.
    pub fn bound(&self) -> Vec<SocketAddr> {
        super::lock(&self.bound).clone()
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn keep_listening(
    peers: Arc<Peers>,
    port: u16,
    bound: Arc<std::sync::Mutex<Vec<SocketAddr>>>,
) {
    let mut serving: Vec<(IpAddr, ServeTask)> = Vec::new();
    let mut announced_none = false;
    loop {
        let addresses = tailnet_addresses();
        if addresses.is_empty() && !announced_none {
            log::info!("not listening for peers: this machine has no tailnet address");
        }
        announced_none = addresses.is_empty();
        serving.retain(|(address, _)| addresses.contains(address));
        for address in addresses {
            if serving.iter().any(|(serving, _)| *serving == address) {
                continue;
            }
            match TcpListener::bind((address, port)).await {
                Ok(listener) => {
                    log::info!("listening for peers on {address} port {port}");
                    let peers = peers.clone();
                    serving.push((address, ServeTask(tokio::spawn(serve(listener, peers)))));
                }
                Err(error) => log::error!("could not listen for peers on {address}: {error}"),
            }
        }
        *super::lock(&bound) = serving
            .iter()
            .map(|(address, _)| SocketAddr::new(*address, port))
            .collect();
        tokio::time::sleep(RECHECK).await;
    }
}

/// A serving task that stops when it is dropped.
struct ServeTask(JoinHandle<()>);

impl Drop for ServeTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Serves peers on `listener` until the task is dropped. A connection from outside
/// the tailnet is refused, whatever address the listener is bound to.
pub async fn serve(listener: TcpListener, peers: Arc<Peers>) {
    loop {
        let (stream, remote) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                log::error!("could not accept a peer: {error}");
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        let peers = peers.clone();
        tokio::spawn(async move {
            let service = service_fn(move |request: Request<Incoming>| {
                let peers = peers.clone();
                async move { Ok::<_, Infallible>(handle(&peers, request, remote.ip()).await) }
            });
            let served = http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(Duration::from_secs(5))
                .serve_connection(TokioIo::new(stream), service)
                .await;
            if let Err(error) = served {
                log::info!("a connection from {remote} ended badly: {error}");
            }
        });
    }
}

async fn handle(
    peers: &Peers,
    request: Request<Incoming>,
    remote: IpAddr,
) -> Response<Full<Bytes>> {
    let (parts, body) = request.into_parts();
    let body = if parts.method == Method::POST {
        match Limited::new(body, BODY_LIMIT).collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => return reply(StatusCode::PAYLOAD_TOO_LARGE, "too large"),
        }
    } else {
        Bytes::new()
    };
    let (status, text) = respond(
        peers,
        &parts.method,
        parts.uri.path(),
        parts.uri.query(),
        &body,
        remote,
    );
    reply(status, &text)
}

fn reply(status: StatusCode, body: &str) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from(body.to_string())));
    *response.status_mut() = status;
    if status.is_success() {
        response.headers_mut().insert(
            hyper::header::CONTENT_TYPE,
            hyper::header::HeaderValue::from_static("application/json"),
        );
    }
    response
}

/// What to answer a request from `remote`, as a status and a body.
pub fn respond(
    peers: &Peers,
    method: &Method,
    path: &str,
    query: Option<&str>,
    body: &[u8],
    remote: IpAddr,
) -> (StatusCode, String) {
    if !is_tailnet(remote) {
        log::warn!("refused a peer request from {remote}, which is not on the tailnet");
        return (
            StatusCode::FORBIDDEN,
            "only machines on the tailnet may ask".into(),
        );
    }
    let refused = |refusal: Refusal| match refusal {
        Refusal::NotSignedIn => (
            StatusCode::SERVICE_UNAVAILABLE,
            "not signed in to Harvest yet".to_string(),
        ),
        Refusal::OtherUser => (
            StatusCode::FORBIDDEN,
            "this machine belongs to another Harvest user".to_string(),
        ),
    };
    match (method, path) {
        (&Method::GET, "/presence") => {
            let pairs: Vec<(String, String)> =
                url::form_urlencoded::parse(query.unwrap_or_default().as_bytes())
                    .into_owned()
                    .collect();
            let field = |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value.as_str())
            };
            let since = field("since").and_then(|text| text.parse::<Timestamp>().ok());
            let user_id = field("user_id").and_then(|text| text.parse::<i64>().ok());
            let (Some(since), Some(user_id)) = (since, user_id) else {
                return (
                    StatusCode::BAD_REQUEST,
                    "needs since and user_id".to_string(),
                );
            };
            match peers.answer_presence(user_id, since) {
                Ok(answer) => (
                    StatusCode::OK,
                    serde_json::to_string(&answer).expect("an answer encodes"),
                ),
                Err(refusal) => refused(refusal),
            }
        }
        (&Method::POST, "/touched") => match serde_json::from_slice::<Touched>(body) {
            Ok(touched) => match peers.receive_touched(touched) {
                Ok(()) => (StatusCode::OK, "{}".to_string()),
                Err(refusal) => refused(refusal),
            },
            Err(error) => (StatusCode::BAD_REQUEST, error.to_string()),
        },
        _ => (StatusCode::NOT_FOUND, "no such request".to_string()),
    }
}
