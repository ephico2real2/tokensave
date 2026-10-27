//! The dashboard's HTTP server: the API under `/api/` and the page embedded
//! in the binary, on a loopback address.
//!
//! Only `GET` is served. A request whose `Host` is not this server's own
//! loopback name is refused (403): a web page cannot point a name it owns at
//! 127.0.0.1 and read the API from the browser (DNS rebinding). Every
//! response carries `Content-Security-Policy: default-src 'self'`, so the
//! page loads nothing from another origin.

use std::convert::Infallible;
use std::future::Future;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::header::{HeaderValue, ALLOW, CACHE_CONTROL, CONTENT_TYPE, HOST};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde::Serialize;

use super::api::{self, ApiError, Context};
use super::params;

/// The policy every response carries: nothing from another origin.
pub const CSP: &str = "default-src 'self'";

/// How many requests read `global.db` at once. SQLite runs on the runtime's
/// worker threads, so without a bound a burst of API requests occupies every
/// worker: `/api/status` and the page wait behind it, and each open
/// connection holds its own page cache. Two keep a page load as fast as
/// with no bound, since the page's requests contend for the CPU anyway.
/// Requests beyond the bound wait without holding a thread.
const DB_READS: usize = 2;

/// What the server needs: the API's context and the page's files.
pub struct Server {
    pub api: Context,
    /// The page's files by path, `index.html` among them once it is built.
    pub assets: &'static [(&'static str, &'static [u8])],
    /// [`DB_READS`] permits, one per request reading the database.
    db_reads: tokio::sync::Semaphore,
}

impl Server {
    pub fn new(api: Context, assets: &'static [(&'static str, &'static [u8])]) -> Self {
        Self {
            api,
            assets,
            db_reads: tokio::sync::Semaphore::new(DB_READS),
        }
    }
}

/// Serves `listener` until `shutdown` completes. Connections in progress
/// when it does are dropped with the runtime.
pub async fn serve(
    listener: tokio::net::TcpListener,
    server: Arc<Server>,
    shutdown: impl Future<Output = ()>,
) {
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => break,
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else {
                    // A failed accept (too many open files, a reset in the
                    // backlog) concerns that connection only; pause so a
                    // lasting one does not spin.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                };
                let server = Arc::clone(&server);
                tokio::spawn(async move {
                    let service = hyper::service::service_fn(move |request| {
                        let server = Arc::clone(&server);
                        async move { Ok::<_, Infallible>(handle(&server, &request).await) }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .timer(TokioTimer::new())
                        .header_read_timeout(Duration::from_secs(10))
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        }
    }
}

/// Answers one request.
async fn handle(server: &Server, request: &Request<Incoming>) -> Response<Full<Bytes>> {
    if !names_this_server(request, server.api.bind, server.api.port) {
        return plain(
            StatusCode::FORBIDDEN,
            "this server answers only on its loopback address\n",
        );
    }
    if request.method() != Method::GET {
        let mut response = plain(StatusCode::METHOD_NOT_ALLOWED, "only GET is served\n");
        response
            .headers_mut()
            .insert(ALLOW, HeaderValue::from_static("GET"));
        return response;
    }
    let path = request.uri().path();
    match path.strip_prefix("/api/") {
        Some(endpoint) => match api_response(server, endpoint, request.uri().query()).await {
            Ok(body) => json(StatusCode::OK, body),
            Err(error) => error_response(&error),
        },
        None => asset(server, path),
    }
}

/// Whether `host` (a `Host` header) names this server: `127.0.0.1`,
/// `localhost`, `[::1]` or the bind address, with its port.
pub fn host_allowed(host: &str, bind: IpAddr, port: u16) -> bool {
    let bind = match bind {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    };
    let host = host.to_ascii_lowercase();
    ["127.0.0.1", "localhost", "[::1]", bind.as_str()]
        .iter()
        .any(|name| host == format!("{name}:{port}"))
}

/// Whether `request` is addressed to this server: exactly one `Host`, which
/// [`host_allowed`] accepts, and for an absolute-form target
/// (`GET http://name/ HTTP/1.1`) a name it accepts too, since that name then
/// wins over `Host` (RFC 9112, section 3.2.2). Two `Host` lines are refused,
/// as RFC 9112 section 3.2 requires.
fn names_this_server<B>(request: &Request<B>, bind: IpAddr, port: u16) -> bool {
    let mut hosts = request.headers().get_all(HOST).iter();
    let (Some(host), None) = (hosts.next(), hosts.next()) else {
        return false;
    };
    let target_ok = request
        .uri()
        .authority()
        .is_none_or(|authority| host_allowed(authority.as_str(), bind, port));
    target_ok
        && host
            .to_str()
            .is_ok_and(|host| host_allowed(host, bind, port))
}

/// The JSON of one API endpoint.
async fn api_response(
    server: &Server,
    endpoint: &str,
    query: Option<&str>,
) -> Result<Vec<u8>, ApiError> {
    const FILTERS: &[&str] = &["range", "from", "to", "repo", "tool", "model", "effort"];
    const TOP: &[&str] = &[
        "range", "from", "to", "repo", "tool", "model", "effort", "limit",
    ];
    const NO_MODEL: &[&str] = &["range", "from", "to", "repo", "tool"];
    const REPOS: &[&str] = &["workspace", "range", "from", "to", "tool"];
    const SAVINGS: &[&str] = &["range", "from", "to", "repo"];

    let ctx = &server.api;
    let allowed: &[&str] = match endpoint {
        "status" | "sources" => &[],
        "summary" | "daily" | "mix" | "runs" | "roles" => FILTERS,
        "runs/top" => TOP,
        "models" | "thinking" | "fallbacks" => NO_MODEL,
        "repos" => REPOS,
        "savings" => SAVINGS,
        _ => {
            return Err(ApiError {
                status: 404,
                message: format!("no endpoint /api/{endpoint}"),
                parameter: None,
            })
        }
    };
    let today = params::day_of((ctx.now)());
    let params = params::parse(query, allowed, today, ctx.default_range_days)?;

    if endpoint == "status" {
        return to_json(&api::status(ctx));
    }
    let _permit = server.db_reads.acquire().await.map_err(|_| ApiError {
        // Only a closed semaphore refuses, and this one is never closed.
        status: 500,
        message: "no database read permit".to_string(),
        parameter: None,
    })?;
    let db = api::open_read_only(&ctx.db_path).await?;
    match endpoint {
        "summary" => to_json(&api::summary(&db, &params).await?),
        "daily" => to_json(&api::daily(&db, &params).await?),
        "mix" => to_json(&api::mix(&db, &params).await?),
        "runs/top" => to_json(&api::top_runs(&db, &params, ctx).await?),
        "runs" => to_json(&api::runs(&db, &params, ctx).await?),
        "roles" => to_json(&api::roles(&db, &params, ctx).await?),
        "models" => to_json(&api::models(&db, &params).await?),
        "thinking" => to_json(&api::thinking(&db, &params).await?),
        "fallbacks" => to_json(&api::fallbacks(&db, &params).await?),
        "repos" => to_json(&api::repos(&db, &params).await?),
        "savings" => to_json(&api::savings(&db, &params).await?),
        _ => to_json(&api::sources(&db, ctx).await?),
    }
}

fn to_json(value: &impl Serialize) -> Result<Vec<u8>, ApiError> {
    serde_json::to_vec(value).map_err(|error| ApiError {
        status: 500,
        message: format!("cannot encode the answer: {error}"),
        parameter: None,
    })
}

/// The page's file at `path`, `/` being `index.html`.
fn asset(server: &Server, path: &str) -> Response<Full<Bytes>> {
    let name = match path.trim_start_matches('/') {
        "" => "index.html",
        name => name,
    };
    match server.assets.iter().find(|(asset, _)| *asset == name) {
        Some((_, bytes)) => {
            let mut response = respond(
                StatusCode::OK,
                content_type(name),
                Bytes::from_static(bytes),
            );
            response
                .headers_mut()
                .insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
            response
        }
        None if name == "index.html" => plain(
            StatusCode::SERVICE_UNAVAILABLE,
            "dashboard page not built: this binary was built without dashboard/dist/\n",
        ),
        None => plain(StatusCode::NOT_FOUND, "not found\n"),
    }
}

/// The media type of a page file, by its extension.
pub fn content_type(name: &str) -> &'static str {
    let extension = name.rsplit_once('.').map_or("", |(_, ext)| ext);
    match extension.to_ascii_lowercase().as_str() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn respond(status: StatusCode, content_type: &'static str, body: Bytes) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(body));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        hyper::header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    headers.insert(
        hyper::header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        hyper::header::X_FRAME_OPTIONS,
        HeaderValue::from_static("DENY"),
    );
    headers.insert(
        hyper::header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn plain(status: StatusCode, text: &'static str) -> Response<Full<Bytes>> {
    respond(
        status,
        "text/plain; charset=utf-8",
        Bytes::from_static(text.as_bytes()),
    )
}

fn json(status: StatusCode, body: Vec<u8>) -> Response<Full<Bytes>> {
    respond(status, "application/json", Bytes::from(body))
}

/// The JSON of an error: `{"error": "...", "parameter": "range" | null}`.
fn error_response(error: &ApiError) -> Response<Full<Bytes>> {
    let body = serde_json::json!({ "error": error.message, "parameter": error.parameter });
    json(
        StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        body.to_string().into_bytes(),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn only_this_servers_loopback_names_are_hosts() {
        let v4 = IpAddr::from([127, 0, 0, 1]);
        assert!(host_allowed("127.0.0.1:7311", v4, 7311));
        assert!(host_allowed("LOCALHOST:7311", v4, 7311));
        assert!(host_allowed("[::1]:7311", v4, 7311));
        assert!(!host_allowed("127.0.0.1:7312", v4, 7311));
        assert!(!host_allowed("evil.example:7311", v4, 7311));
        assert!(!host_allowed("127.0.0.1", v4, 7311));
        assert!(host_allowed(
            "127.0.0.2:80",
            IpAddr::from([127, 0, 0, 2]),
            80
        ));
        let v6: IpAddr = "::1".parse().unwrap_or(v4);
        assert!(host_allowed("[::1]:7311", v6, 7311));
    }

    #[test]
    fn a_request_names_this_server_only_with_one_allowed_host_and_target() {
        let v4 = IpAddr::from([127, 0, 0, 1]);
        let request = |target: &str, hosts: &[&str]| {
            let mut builder = Request::builder().uri(target);
            for host in hosts {
                builder = builder.header(HOST, *host);
            }
            builder.body(()).unwrap()
        };
        assert!(names_this_server(
            &request("/api/status", &["127.0.0.1:7311"]),
            v4,
            7311
        ));
        assert!(names_this_server(
            &request("http://localhost:7311/api/status", &["127.0.0.1:7311"]),
            v4,
            7311
        ));
        assert!(!names_this_server(&request("/api/status", &[]), v4, 7311));
        assert!(!names_this_server(
            &request("/api/status", &["127.0.0.1:7311", "evil.example"]),
            v4,
            7311
        ));
        assert!(!names_this_server(
            &request("http://evil.example/api/status", &["127.0.0.1:7311"]),
            v4,
            7311
        ));
    }

    #[tokio::test]
    async fn database_reads_wait_for_a_permit_and_status_does_not() {
        let server = Server::new(
            Context {
                db_path: std::path::PathBuf::from("/nonexistent/ob3/global.db"),
                bind: IpAddr::from([127, 0, 0, 1]),
                port: 7311,
                default_range_days: 30,
                home: None,
                codex_home: None,
                now: || 1_793_000_000,
            },
            &[],
        );
        let held = server.db_reads.acquire_many(DB_READS as u32).await.unwrap();
        let wait = Duration::from_millis(200);
        assert!(
            tokio::time::timeout(wait, api_response(&server, "status", None))
                .await
                .is_ok(),
            "status needs no permit"
        );
        assert!(
            tokio::time::timeout(wait, api_response(&server, "summary", None))
                .await
                .is_err(),
            "a database read waits while every permit is held"
        );
        drop(held);
        let answer = tokio::time::timeout(wait, api_response(&server, "summary", None))
            .await
            .expect("a free permit lets the read run");
        assert_eq!(answer.unwrap_err().status, 503);
    }

    #[test]
    fn page_files_are_served_with_their_media_type() {
        assert_eq!(content_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(
            content_type("assets/index-abc.js"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(content_type("tokens.css"), "text/css; charset=utf-8");
        assert_eq!(content_type("logo.SVG"), "image/svg+xml");
        assert_eq!(content_type("README"), "application/octet-stream");
    }
}
