//! DaemonHall — a web control room for your own systemd services.
//!
//! Serves on 127.0.0.1 only (default :8767). Watches the same units as
//! cyberwatch (shared config, shared discovery), shows live state,
//! resources, logs and history, and lets you start/stop/restart,
//! enable/disable, run timers now, add/remove services and edit units.

mod cybergrid;
mod journal;
mod monitor;
mod ops;
mod systemd;
mod views;
mod web;

use std::io::Read;
use std::sync::Arc;

fn token() -> String {
    let mut bytes = [0u8; 24];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .expect("read /dev/urandom");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn usage() -> ! {
    println!(
        "daemonhall — web control room for your own systemd services\n\n\
         USAGE: daemonhall [--port N]\n\n\
         Serves http://127.0.0.1:<port>/ (default {}; env DAEMONHALL_PORT).\n\
         Watch list: ~/.config/cyberwatch/config.json (shared with cyberwatch).",
        daemonhall::DEFAULT_PORT
    );
    std::process::exit(0)
}

#[tokio::main]
async fn main() {
    let mut port: u16 = std::env::var("DAEMONHALL_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(daemonhall::DEFAULT_PORT);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => {
                port = args
                    .next()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or_else(|| usage())
            }
            "-h" | "--help" => usage(),
            other => {
                eprintln!("daemonhall: unknown argument {other:?} (try --help)");
                std::process::exit(2);
            }
        }
    }

    let monitor = monitor::Monitor::new();
    tokio::spawn(monitor::run(monitor.clone()));

    let state = Arc::new(web::AppState {
        token: token(),
        port,
        monitor,
    });
    let app = web::router(state);
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("daemonhall: can't bind 127.0.0.1:{port}: {e}");
            std::process::exit(1);
        }
    };
    println!("[DaemonHall] listening on http://127.0.0.1:{port}/");
    let shutdown = async {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("signal");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    };
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .expect("server");
}
