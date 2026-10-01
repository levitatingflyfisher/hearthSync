//! The HTTP/1.1 server around [`Relay::handle`]: body limits, timeouts, a connection
//! cap and CORS. TLS is the job of a proxy in front (relay/deploy/README.md).

use std::convert::Infallible;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::header::{
    HeaderValue, ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
    ACCESS_CONTROL_MAX_AGE, CONTENT_LENGTH, CONTENT_TYPE,
};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

use crate::log;
use crate::relay::{error_body, Relay};

/// Transport limits (the handler's own limits live in [`crate::Config`]).
#[derive(Clone, Debug)]
pub struct HttpLimits {
    pub max_body: u64,
    /// Time allowed to send a request's headers (and to start the next one on a
    /// kept-alive connection).
    pub header_timeout: Duration,
    /// Time allowed to send a request's body.
    pub body_timeout: Duration,
    /// Longest life of one connection.
    pub conn_timeout: Duration,
    pub max_connections: usize,
    /// Serve the test hook `POST /test/sweep` (the CLI's `--test-hooks`). Off by
    /// default; never for a relay that serves households.
    pub test_hooks: bool,
}

impl HttpLimits {
    pub fn for_config(cfg: &crate::Config) -> Self {
        HttpLimits {
            max_body: cfg.max_body(),
            header_timeout: Duration::from_secs(10),
            body_timeout: Duration::from_secs(60),
            conn_timeout: Duration::from_secs(300),
            max_connections: 512,
            test_hooks: false,
        }
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

type Resp = Response<Full<Bytes>>;

fn reply(status: u16, content_type: &str, body: Vec<u8>) -> Resp {
    let mut r = Response::new(Full::new(Bytes::from(body)));
    *r.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let h = r.headers_mut();
    h.insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    if let Ok(v) = HeaderValue::from_str(content_type) {
        h.insert(CONTENT_TYPE, v);
    }
    r
}

fn log_request(verb: &str, status: u16, started: Instant) {
    let ms = started.elapsed().as_millis().to_string();
    log::event("info", &[("event", "request"), ("verb", verb), ("status", &status.to_string()), ("ms", &ms)]);
}

async fn handle(req: Request<Incoming>, relay: Arc<Mutex<Relay>>, lim: HttpLimits) -> Result<Resp, Infallible> {
    let started = Instant::now();
    if req.method() == hyper::Method::OPTIONS {
        let mut r = reply(204, "text/plain", Vec::new());
        let h = r.headers_mut();
        h.insert(ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("POST"));
        h.insert(ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("content-type"));
        h.insert(ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("86400"));
        return Ok(r);
    }
    let method = req.method().as_str().to_owned();
    let path = req.uri().path().to_owned();
    let declared = req.headers().get(CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|n| n > lim.max_body) {
        log_request("-", 413, started);
        return Ok(reply(413, "application/cbor", error_body("too_large")));
    }
    let body =
        match tokio::time::timeout(lim.body_timeout, Limited::new(req.into_body(), lim.max_body as usize).collect())
            .await
        {
            Ok(Ok(c)) => c.to_bytes(),
            Ok(Err(e)) if e.downcast_ref::<LengthLimitError>().is_some() => {
                log_request("-", 413, started);
                return Ok(reply(413, "application/cbor", error_body("too_large")));
            }
            Ok(Err(_)) => {
                log_request("-", 400, started);
                return Ok(reply(400, "application/cbor", error_body("bad_request")));
            }
            Err(_) => {
                log_request("-", 408, started);
                return Ok(reply(408, "application/cbor", error_body("timeout")));
            }
        };
    let now = now_ms();
    if lim.test_hooks && path == "/test/sweep" && method == "POST" {
        return Ok(test_sweep(&relay, &body, now, started).await);
    }
    let res = tokio::task::spawn_blocking(move || {
        let mut r = relay.lock().unwrap_or_else(|p| p.into_inner());
        r.handle(&method, &path, &body, now)
    })
    .await;
    Ok(match res {
        Ok(r) => {
            log_request(r.verb, r.status, started);
            reply(r.status, r.content_type, r.body)
        }
        Err(_) => {
            log::event("error", &[("event", "panic")]);
            reply(500, "application/cbor", error_body("internal"))
        }
    })
}

/// The test hook: run the sweep as if `body` (decimal milliseconds, empty for 0)
/// had passed, so idle channels expire without a restart. The relay's clock does
/// not move, so requests signed with the wall clock stay fresh.
async fn test_sweep(relay: &Arc<Mutex<Relay>>, body: &[u8], now: u64, started: Instant) -> Resp {
    let ahead = match std::str::from_utf8(body).ok().map(str::trim) {
        Some("") => Some(0),
        Some(v) => v.parse::<u64>().ok(),
        None => None,
    };
    let Some(ahead) = ahead else {
        log_request("test_sweep", 400, started);
        return reply(400, "application/cbor", error_body("bad_request"));
    };
    let relay = relay.clone();
    let res = tokio::task::spawn_blocking(move || {
        let mut r = relay.lock().unwrap_or_else(|p| p.into_inner());
        r.sweep_ahead(now, ahead)
    })
    .await;
    match res {
        Ok(Ok(n)) => {
            log::event("info", &[("event", "sweep"), ("pruned", &n.to_string()), ("ahead_ms", &ahead.to_string())]);
            log_request("test_sweep", 200, started);
            reply(200, "text/plain", b"ok".to_vec())
        }
        _ => {
            log_request("test_sweep", 500, started);
            reply(500, "application/cbor", error_body("internal"))
        }
    }
}

/// Serve until `shutdown` resolves. Connections past the cap are closed at once.
pub async fn serve(
    listener: TcpListener,
    relay: Arc<Mutex<Relay>>,
    lim: HttpLimits,
    shutdown: impl Future<Output = ()>,
) {
    let sweeper = {
        let relay = relay.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(3600));
            loop {
                tick.tick().await;
                let relay = relay.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    let mut r = relay.lock().unwrap_or_else(|p| p.into_inner());
                    match r.sweep(now_ms()) {
                        Ok(n) => log::event("info", &[("event", "sweep"), ("pruned", &n.to_string())]),
                        Err(e) => log::event("error", &[("event", "sweep"), ("error", &e.to_string())]),
                    }
                })
                .await;
            }
        })
    };
    let accept = tokio::spawn(async move {
        let sem = Arc::new(Semaphore::new(lim.max_connections));
        loop {
            let stream = match listener.accept().await {
                Ok((s, _)) => s,
                Err(e) => {
                    log::event("warn", &[("event", "accept"), ("error", &e.to_string())]);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            let Ok(permit) = sem.clone().try_acquire_owned() else {
                drop(stream);
                continue;
            };
            let (relay, lim) = (relay.clone(), lim.clone());
            tokio::spawn(async move {
                let _permit = permit;
                let svc_lim = lim.clone();
                let svc = service_fn(move |req| handle(req, relay.clone(), svc_lim.clone()));
                let conn = http1::Builder::new()
                    .timer(TokioTimer::new())
                    .header_read_timeout(lim.header_timeout)
                    .serve_connection(TokioIo::new(stream), svc);
                let _ = tokio::time::timeout(lim.conn_timeout, conn).await;
            });
        }
    });
    shutdown.await;
    accept.abort();
    sweeper.abort();
}
