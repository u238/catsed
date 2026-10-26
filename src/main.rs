mod app;
mod body;
mod code;
mod config;
mod filename;
mod limits;
mod metrics;
mod proxy_protocol;
mod server;
mod transfer;
mod units;

use std::{net::IpAddr, process::ExitCode, sync::Arc, time::Duration};

use hyper::{server::conn::http1, service::service_fn};
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::{
    net::{TcpListener, TcpStream},
    signal::unix::{SignalKind, signal},
    time::{Instant, sleep, timeout},
};

use crate::{app::App, config::Config};

#[tokio::main]
async fn main() -> ExitCode {
    let cfg = match Config::from_env() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("configuration error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let listener = match TcpListener::bind(cfg.listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot listen on {}: {e}", cfg.listen);
            return ExitCode::FAILURE;
        }
    };
    eprintln!(
        "event=listening addr={} public_url={}",
        cfg.listen, cfg.public_url
    );
    let app = Arc::new(App::new(cfg));

    if let Some(addr) = app.cfg.metrics_listen {
        match TcpListener::bind(addr).await {
            Ok(l) => {
                eprintln!("event=metrics_listening addr={addr}");
                tokio::spawn(serve_metrics(app.clone(), l));
            }
            Err(e) => {
                eprintln!("cannot listen on {addr} for metrics: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    let accept = tokio::spawn(accept_loop(app.clone(), listener));
    wait_for_signal().await;
    drain(&app).await;
    accept.abort();
    ExitCode::SUCCESS
}

async fn accept_loop(app: Arc<App>, listener: TcpListener) {
    loop {
        let (sock, peer) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                eprintln!("event=accept_error error={e:?}");
                sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let app = app.clone();
        tokio::spawn(async move {
            let peer = peer.ip().to_canonical();
            let Some((sock, client_ip)) = client_address(&app, sock, peer).await else {
                return;
            };
            let svc = service_fn(move |req| server::handle(app.clone(), req, client_ip));
            let conn = http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(Duration::from_secs(30))
                .serve_connection(TokioIo::new(sock), svc);
            if let Err(e) = conn.await {
                eprintln!("event=connection_error client={client_ip} error={e}");
            }
        });
    }
}

/// Connections from a trusted proxy must start with a PROXY header naming the real client.
async fn client_address(
    app: &App,
    mut sock: TcpStream,
    peer: IpAddr,
) -> Option<(TcpStream, IpAddr)> {
    if !app.cfg.trusted_proxies.contains(&peer) {
        return Some((sock, peer));
    }
    match timeout(Duration::from_secs(5), proxy_protocol::read_v1(&mut sock)).await {
        Ok(Ok(client)) => Some((sock, client.map_or(peer, |ip| ip.to_canonical()))),
        Ok(Err(e)) => {
            eprintln!("event=proxy_header_error peer={peer} error={e}");
            None
        }
        Err(_) => {
            eprintln!("event=proxy_header_timeout peer={peer}");
            None
        }
    }
}

async fn serve_metrics(app: Arc<App>, listener: TcpListener) {
    while let Ok((sock, _)) = listener.accept().await {
        let app = app.clone();
        tokio::spawn(async move {
            let svc = service_fn(move |req| server::metrics(app.clone(), req));
            let _ = http1::Builder::new()
                .serve_connection(TokioIo::new(sock), svc)
                .await;
        });
    }
}

async fn wait_for_signal() {
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
}

/// Closes waiting Transfers at once and gives Claimed ones until the grace period ends.
async fn drain(app: &App) {
    app.begin_shutdown();
    let (_, active) = app.counts();
    eprintln!(
        "event=shutdown active={active} grace_secs={}",
        app.cfg.shutdown_grace.as_secs()
    );
    let deadline = Instant::now() + app.cfg.shutdown_grace;
    // Let waiting Senders receive their "please resend" line.
    sleep(Duration::from_millis(200)).await;
    while app.counts().1 > 0 && Instant::now() < deadline {
        sleep(Duration::from_millis(200)).await;
    }
    let (_, active) = app.counts();
    eprintln!("event=stopped unfinished={active}");
}
