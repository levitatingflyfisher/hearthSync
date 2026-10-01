//! `hearth-relay`: serve the hearthSync relay protocol over HTTP/1.1.
//! House CLI rules: results to stdout (only --help and --version print any), errors and
//! logs to stderr; exit 0 success, 1 failure, 2 usage.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use hearth_sync_relay::config::DAY_MS;
use hearth_sync_relay::http::{serve, HttpLimits};
use hearth_sync_relay::{log, Config, Relay};

const HELP: &str = "\
hearth-relay: the hearthSync relay (data-blind store-and-forward of sealed ops)

Usage: hearth-relay --data <DIR> [options]

Options:
  --data <DIR>               Directory for the database (must exist and be writable)
  --listen <ADDR>            Address to listen on [default: 127.0.0.1:8080]
  --channel-quota <BYTES>    Storage per household channel [default: 67108864]
  --max-total-bytes <BYTES>  Storage for the whole relay [default: 8589934592]
  --max-channels <N>         Household channels the relay accepts [default: 1000]
  --retain-days <N>          Keep covered log entries this long [default: 120]
  --idle-days <N>            Expire a channel after this long with no write [default: 400]
  --max-devices <N>          Devices per household channel [default: 32]
  --max-reader-nonces <N>    Live read nonces per device and channel [default: 16]
  --channel-burst <N>        Requests a channel may burst to [default: 600]
  --channel-interval-ms <N>  One more channel request allowed every N ms [default: 100]
  --test-hooks               Serve POST /test/sweep, a sweep as if N ms (the body) had
                             passed. For tests only: never on a relay that serves households
  -h, --help                 Print this help
  --version                  Print the version

It prints nothing while serving except structured JSON logs on stderr, which never
carry channel or device ids. Put a TLS proxy in front (see relay/deploy/README.md).
Stops on SIGTERM or SIGINT.
";

struct Args {
    data: PathBuf,
    listen: SocketAddr,
    cfg: Config,
    test_hooks: bool,
}

enum Parsed {
    Run(Box<Args>),
    Help,
    Version,
}

fn parse(args: Vec<String>) -> Result<Parsed, String> {
    let mut data = None;
    let mut listen: SocketAddr = "127.0.0.1:8080".parse().expect("valid default");
    let mut cfg = Config::default();
    let mut test_hooks = false;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut val = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        let num = |name: &str, v: String| v.parse::<u64>().map_err(|_| format!("{name}: not a number: {v}"));
        match a.as_str() {
            "-h" | "--help" => return Ok(Parsed::Help),
            "--version" => return Ok(Parsed::Version),
            "--data" => data = Some(PathBuf::from(val("--data")?)),
            "--listen" => {
                let v = val("--listen")?;
                listen = v.parse().map_err(|_| format!("--listen: not an address: {v}"))?;
            }
            "--channel-quota" => cfg.channel_quota = num("--channel-quota", val("--channel-quota")?)?,
            "--max-total-bytes" => cfg.max_total_bytes = num("--max-total-bytes", val("--max-total-bytes")?)?,
            "--max-channels" => cfg.max_channels = num("--max-channels", val("--max-channels")?)?,
            "--max-devices" => cfg.max_devices = num("--max-devices", val("--max-devices")?)?,
            "--max-reader-nonces" => cfg.max_reader_nonces = num("--max-reader-nonces", val("--max-reader-nonces")?)?,
            "--channel-burst" => cfg.channel_burst = num("--channel-burst", val("--channel-burst")?)?,
            "--channel-interval-ms" => {
                cfg.channel_interval_ms = num("--channel-interval-ms", val("--channel-interval-ms")?)?
            }
            "--retain-days" => {
                let d = num("--retain-days", val("--retain-days")?)?;
                cfg.retain_ms = d.checked_mul(DAY_MS).ok_or("--retain-days: too large")?;
            }
            "--idle-days" => {
                let d = num("--idle-days", val("--idle-days")?)?;
                cfg.idle_ms = d.checked_mul(DAY_MS).ok_or("--idle-days: too large")?;
            }
            "--test-hooks" => test_hooks = true,
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    let data = data.ok_or("--data <DIR> is required")?;
    Ok(Parsed::Run(Box::new(Args { data, listen, cfg, test_hooks })))
}

fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1).collect()) {
        Ok(Parsed::Help) => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Ok(Parsed::Version) => {
            println!("hearth-relay {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Ok(Parsed::Run(a)) => a,
        Err(e) => {
            eprintln!("hearth-relay: {e}\nTry 'hearth-relay --help'.");
            return ExitCode::from(2);
        }
    };
    if !args.data.is_dir() {
        eprintln!("hearth-relay: --data {}: not a directory", args.data.display());
        return ExitCode::from(1);
    }
    let relay = match Relay::open(Some(&args.data), args.cfg.clone()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("hearth-relay: cannot open the database in {}: {e}", args.data.display());
            return ExitCode::from(1);
        }
    };
    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().thread_stack_size(8 << 20).build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("hearth-relay: cannot start the runtime: {e}");
            return ExitCode::from(1);
        }
    };
    rt.block_on(async move {
        let listener = match tokio::net::TcpListener::bind(args.listen).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("hearth-relay: cannot listen on {}: {e}", args.listen);
                return ExitCode::from(1);
            }
        };
        let addr = listener.local_addr().map(|a| a.to_string()).unwrap_or_default();
        log::event("info", &[("event", "start"), ("listen", &addr), ("version", env!("CARGO_PKG_VERSION"))]);
        if args.test_hooks {
            log::event("warn", &[("event", "test_hooks"), ("path", "/test/sweep")]);
        }
        let lim = HttpLimits { test_hooks: args.test_hooks, ..HttpLimits::for_config(&args.cfg) };
        serve(listener, Arc::new(Mutex::new(relay)), lim, shutdown()).await;
        log::event("info", &[("event", "stop")]);
        ExitCode::SUCCESS
    })
}

async fn shutdown() {
    use tokio::signal::unix::{signal, SignalKind};
    let n = Arc::new(tokio::sync::Notify::new());
    let mut any = false;
    for kind in [SignalKind::terminate(), SignalKind::interrupt()] {
        if let Ok(mut s) = signal(kind) {
            any = true;
            let n = n.clone();
            tokio::spawn(async move {
                s.recv().await;
                n.notify_one();
            });
        }
    }
    if !any {
        log::event("warn", &[("event", "no_signal_handlers")]);
    }
    n.notified().await;
}
