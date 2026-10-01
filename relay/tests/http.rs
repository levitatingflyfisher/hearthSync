//! The HTTP layer and the CLI: CORS, body limits, content types, and the house CLI
//! rules (help and version on stdout, errors on stderr, exit 2 usage / 1 failure,
//! nothing on stdout while serving, JSON logs on stderr, a clean stop on SIGTERM).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hearth_sync_relay::http::{now_ms, serve, HttpLimits};
use hearth_sync_relay::{Config, Relay};

const BIN: &str = env!("CARGO_BIN_EXE_hearth-relay");

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// One request over a fresh connection, by hand (no HTTP client dependency).
fn request(addr: SocketAddr, raw: &[u8]) -> Reply {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(raw).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let split = out.windows(4).position(|w| w == b"\r\n\r\n").expect("a full response");
    let head = String::from_utf8(out[..split].to_vec()).unwrap();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers =
        lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
    Reply { status, headers, body: out[split + 4..].to_vec() }
}

fn post(addr: SocketAddr, path: &str, body: &[u8]) -> Reply {
    let mut raw = format!(
        "POST {path} HTTP/1.1\r\nHost: relay\r\nContent-Type: application/cbor\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    raw.extend_from_slice(body);
    request(addr, &raw)
}

/// Run a server on an ephemeral port for the closure's duration.
fn with_server(cfg: Config, f: impl FnOnce(SocketAddr)) {
    with_server_hooks(cfg, false, f)
}

fn with_server_hooks(cfg: Config, test_hooks: bool, f: impl FnOnce(SocketAddr)) {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = listener.local_addr().unwrap();
    let lim = HttpLimits { test_hooks, ..HttpLimits::for_config(&cfg) };
    let relay = Arc::new(Mutex::new(Relay::open(None, cfg).unwrap()));
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let server = rt.spawn(serve(listener, relay, lim, async move {
        let _ = rx.await;
    }));
    f(addr);
    let _ = tx.send(());
    rt.block_on(server).unwrap();
}

const CH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[test]
fn cors_preflight_and_every_answer_allow_any_origin() {
    with_server(Config::default(), |addr| {
        let r = request(addr, format!("OPTIONS /v1/{CH}/pull HTTP/1.1\r\nHost: relay\r\nOrigin: https://app.example\r\nAccess-Control-Request-Method: POST\r\nConnection: close\r\n\r\n").as_bytes());
        assert_eq!(r.status, 204);
        assert_eq!(r.header("access-control-allow-origin"), Some("*"));
        assert_eq!(r.header("access-control-allow-methods"), Some("POST"));
        assert_eq!(r.header("access-control-allow-headers"), Some("content-type"));
        assert_eq!(r.header("access-control-max-age"), Some("86400"));
        let r = post(addr, &format!("/v1/{CH}/pull"), b"\x80");
        assert_eq!(r.status, 400);
        assert_eq!(r.header("access-control-allow-origin"), Some("*"));
        assert_eq!(r.header("content-type"), Some("application/cbor"));
        assert_eq!(r.body, b"\x82\x63err\x6bbad_request");
        let r = request(addr, b"GET /healthz HTTP/1.1\r\nHost: relay\r\nConnection: close\r\n\r\n");
        assert_eq!((r.status, r.body.as_slice()), (200, b"ok".as_slice()));
    });
}

#[test]
fn bodies_over_the_limit_are_refused_declared_or_streamed() {
    let cfg = Config { max_snapshot: 1000, max_batch: 1, max_envelope: 100, ..Config::default() };
    let limit = cfg.max_body();
    with_server(cfg, |addr| {
        // Declared too large: refused before reading.
        let raw = format!(
            "POST /v1/{CH}/append HTTP/1.1\r\nHost: relay\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            limit + 1
        );
        let r = request(addr, raw.as_bytes());
        assert_eq!(r.status, 413);
        // Streamed in chunks with no length: refused once it passes the limit.
        let mut raw = format!(
            "POST /v1/{CH}/append HTTP/1.1\r\nHost: relay\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
        )
        .into_bytes();
        let chunk = vec![0x41u8; 4096];
        for _ in 0..(limit / 4096 + 2) {
            raw.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
            raw.extend_from_slice(&chunk);
            raw.extend_from_slice(b"\r\n");
        }
        raw.extend_from_slice(b"0\r\n\r\n");
        let r = request(addr, &raw);
        assert_eq!(r.status, 413);
        assert_eq!(r.body, b"\x82\x63err\x69too_large");
    });
}

#[test]
fn the_handler_answers_the_same_over_http() {
    use hearth_sync_kernel::keys::{sign_enroll, DeviceSigner, HouseholdRoot, SoftSigner};
    use hearth_sync_relay::wire::{channel_id, client};
    let root = HouseholdRoot::from_seed([3; 64]);
    let hh = root.enroll_public("lullaby");
    let ch: String = channel_id("lullaby", &hh).iter().map(|b| format!("{b:02x}")).collect();
    let dev = SoftSigner::from_secret([4; 32]);
    with_server(Config::default(), |addr| {
        let body =
            client::enroll("lullaby", hh, dev.device(), "phone", sign_enroll(&root, "lullaby", &dev.device(), "phone"));
        let r = post(addr, &format!("/v1/{ch}/enroll"), &body);
        assert_eq!((r.status, client::parse_enroll(&r.body)), (200, Some(1)), "a new store's first channel");
        let body = client::pull(&channel_id("lullaby", &hh), &dev, 1, [1; 16], vec![], now_ms());
        let r = post(addr, &format!("/v1/{ch}/pull"), &body);
        assert_eq!(r.status, 200);
        assert!(client::parse_pull(&r.body).is_some());
        let r =
            request(addr, format!("GET /v1/{ch}/pull HTTP/1.1\r\nHost: relay\r\nConnection: close\r\n\r\n").as_bytes());
        assert_eq!(r.status, 405);
    });
}

#[test]
fn the_test_sweep_hook_expires_idle_channels_ahead_of_time_only_when_enabled() {
    use hearth_sync_kernel::keys::{sign_enroll, DeviceSigner, HouseholdRoot, SoftSigner};
    use hearth_sync_relay::wire::{channel_id, client};
    let root = HouseholdRoot::from_seed([3; 64]);
    let hh = root.enroll_public("lullaby");
    let ch: String = channel_id("lullaby", &hh).iter().map(|b| format!("{b:02x}")).collect();
    let dev = SoftSigner::from_secret([4; 32]);
    let enroll =
        client::enroll("lullaby", hh, dev.device(), "phone", sign_enroll(&root, "lullaby", &dev.device(), "phone"));
    let cfg = Config { idle_ms: 10_000, ..Config::default() };
    // Off by default: the path is unknown, like any other.
    with_server(cfg.clone(), |addr| {
        assert_eq!(post(addr, "/test/sweep", b"20000").status, 404);
    });
    with_server_hooks(cfg, true, |addr| {
        let r = post(addr, &format!("/v1/{ch}/enroll"), &enroll);
        assert_eq!(client::parse_enroll(&r.body), Some(1));
        // A sweep as of now: the channel is not idle yet.
        let r = post(addr, "/test/sweep", b"");
        assert_eq!((r.status, r.body.as_slice()), (200, b"ok".as_slice()));
        assert_eq!(client::parse_enroll(&post(addr, &format!("/v1/{ch}/enroll"), &enroll).body), Some(1));
        // As of 20 s from now: it expired, so enrolling again makes it anew.
        assert_eq!(post(addr, "/test/sweep", b"20000").status, 200);
        assert_eq!(client::parse_enroll(&post(addr, &format!("/v1/{ch}/enroll"), &enroll).body), Some(2));
        // The relay's clock did not move: a read signed now is fresh, not stale.
        let body = client::pull(&channel_id("lullaby", &hh), &dev, 1, [1; 16], vec![], now_ms());
        assert_eq!(post(addr, &format!("/v1/{ch}/pull"), &body).status, 200);
        assert_eq!(post(addr, "/test/sweep", b"soon").status, 400);
        assert_eq!(request(addr, b"GET /test/sweep HTTP/1.1\r\nHost: relay\r\nConnection: close\r\n\r\n").status, 404);
    });
}

// ---------------------------------------------------------------- the CLI

fn run(args: &[&str]) -> (i32, String, String) {
    let o = Command::new(BIN).args(args).output().unwrap();
    (o.status.code().unwrap_or(-1), String::from_utf8(o.stdout).unwrap(), String::from_utf8(o.stderr).unwrap())
}

#[test]
fn help_and_version_go_to_stdout_and_usage_errors_exit_2() {
    for flag in ["-h", "--help"] {
        let (code, out, err) = run(&[flag]);
        assert_eq!((code, err.as_str()), (0, ""));
        assert!(out.starts_with("hearth-relay:") && out.contains("--data <DIR>") && out.contains("--version"));
        for flag in [
            "--idle-days <N>",
            "--max-devices <N>",
            "--max-reader-nonces <N>",
            "--channel-burst <N>",
            "--channel-interval-ms <N>",
        ] {
            assert!(out.contains(flag), "{flag}");
        }
    }
    let (code, out, _) = run(&["--version"]);
    assert_eq!(code, 0);
    assert_eq!(out, format!("hearth-relay {}\n", env!("CARGO_PKG_VERSION")));
    for args in [
        &[][..],
        &["--bogus"],
        &["--data"],
        &["--data", ".", "--listen", "nowhere"],
        &["--data", ".", "--max-channels", "x"],
        &["--data", ".", "--idle-days", "x"],
        &["--data", ".", "--idle-days", "213503982334602"],
        &["--data", ".", "--max-devices", "x"],
        &["--data", ".", "--max-reader-nonces", "-1"],
        &["--data", ".", "--channel-burst", ""],
        &["--data", ".", "--channel-interval-ms", "1.5"],
    ] {
        let (code, out, err) = run(args);
        assert_eq!(code, 2, "{args:?}");
        assert_eq!(out, "", "{args:?}");
        assert!(err.starts_with("hearth-relay: ") && err.contains("--help"), "{args:?}: {err}");
    }
}

#[test]
fn test_hooks_are_a_flag_that_warns_at_start() {
    let (_, out, _) = run(&["--help"]);
    assert!(out.contains("--test-hooks"));
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("relay-cli-hooks");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut child = Command::new(BIN)
        .args(["--data", dir.to_str().unwrap(), "--listen", "127.0.0.1:0", "--test-hooks"])
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut err = BufReader::new(child.stderr.take().unwrap());
    let mut first = String::new();
    err.read_line(&mut first).unwrap();
    let listen = first.split("\"listen\":\"").nth(1).unwrap().split('"').next().unwrap();
    let addr: SocketAddr = listen.parse().unwrap();
    let mut second = String::new();
    err.read_line(&mut second).unwrap();
    assert!(second.contains("\"event\":\"test_hooks\""), "{second}");
    assert_eq!(post(addr, "/test/sweep", b"1000").status, 200);
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn failures_exit_1() {
    let (code, out, err) = run(&["--data", "/nonexistent/hearth-relay-test"]);
    assert_eq!((code, out.as_str()), (1, ""));
    assert!(err.contains("not a directory"));
}

#[test]
fn serving_prints_only_json_logs_to_stderr_and_stops_cleanly_on_sigterm() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("relay-cli-serve");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut child = Command::new(BIN)
        .args(["--data", dir.to_str().unwrap(), "--listen", "127.0.0.1:0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut err = BufReader::new(child.stderr.take().unwrap());
    let mut first = String::new();
    err.read_line(&mut first).unwrap();
    assert!(first.starts_with("{\"ts\":") && first.contains("\"event\":\"start\""), "{first}");
    let listen = first.split("\"listen\":\"").nth(1).unwrap().split('"').next().unwrap();
    let addr: SocketAddr = listen.parse().unwrap();
    let r = request(addr, b"GET /healthz HTTP/1.1\r\nHost: relay\r\nConnection: close\r\n\r\n");
    assert_eq!(r.status, 200);
    let r = post(addr, &format!("/v1/{CH}/pull"), b"\x80");
    assert_eq!(r.status, 400);
    let st = Command::new("kill").args(["-TERM", &child.id().to_string()]).status().unwrap();
    assert!(st.success());
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(0));
    let mut out = String::new();
    child.stdout.take().unwrap().read_to_string(&mut out).unwrap();
    assert_eq!(out, "", "nothing on stdout while serving");
    let mut rest = String::new();
    err.read_to_string(&mut rest).unwrap();
    for line in rest.lines() {
        assert!(line.starts_with("{\"ts\":") && line.ends_with('}'), "not a JSON log line: {line}");
        assert!(!line.contains(CH), "a log line carries a channel id: {line}");
    }
    assert!(rest.contains("\"event\":\"stop\""));
    assert!(dir.join("relay.sqlite3").exists());
    let _ = std::fs::remove_dir_all(&dir);
}
