//! One Transfer, from the Sender's side: announce the Transfer URL, wait for a Claim,
//! then relay the request body to the Receiver.

use std::{sync::Arc, time::Duration};

use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::body::{Frame, Incoming};
use tokio::{
    sync::oneshot,
    time::{Instant, interval_at, sleep_until, timeout},
};

use crate::{
    app::{App, Claim},
    body::Outlet,
    filename,
    metrics::Metrics,
    units::{format_duration, format_size},
};

pub struct Opened {
    pub id: u64,
    pub code: String,
    pub name: String,
    pub wait: Duration,
    pub claim: oneshot::Receiver<Claim>,
    /// An upload frame that was already read while answering `Expect: 100-continue`.
    pub first: Option<Result<Frame<Bytes>, hyper::Error>>,
}

/// Drives the Transfer; `status` is the body of the Sender's streamed `200`.
pub async fn run(app: Arc<App>, t: Opened, mut upload: Incoming, status: Outlet) {
    let Opened {
        id,
        code,
        name,
        wait,
        claim,
        first,
    } = t;
    let Some(Claim {
        receiver_ip,
        outlet,
    }) = wait_for_claim(&app, id, &code, &name, wait, claim, &status).await
    else {
        return;
    };
    Metrics::inc(&app.metrics.claimed);
    eprintln!("event=claimed id={id} receiver={receiver_ip}");
    status.line(format!("# claimed by {receiver_ip}")).await;

    let started = Instant::now();
    let idle = app.cfg.idle_timeout;
    let mut relayed: u64 = 0;
    let mut pending = first;
    let outcome = loop {
        let next = match pending.take() {
            Some(frame) => Some(frame),
            None => match timeout(idle, upload.frame()).await {
                Err(_) => break Err("sender stalled"),
                Ok(next) => next,
            },
        };
        let frame = match next {
            None => break Ok(()),
            Some(Err(_)) => break Err("sender disconnected"),
            Some(Ok(frame)) => frame,
        };
        let Ok(data) = frame.into_data() else {
            continue;
        };
        if let Some(rate) = &app.rate {
            rate.acquire(data.len()).await;
        }
        let len = data.len() as u64;
        match timeout(idle, outlet.send(data)).await {
            Err(_) => break Err("receiver stalled"),
            Ok(false) => break Err("receiver disconnected"),
            Ok(true) => {
                relayed += len;
                app.metrics
                    .bytes_relayed
                    .fetch_add(len, std::sync::atomic::Ordering::Relaxed);
            }
        }
    };
    app.finish(&code);

    let elapsed = started.elapsed();
    match outcome {
        Ok(()) => {
            outlet.finish();
            Metrics::inc(&app.metrics.completed);
            eprintln!(
                "event=completed id={id} bytes={relayed} secs={:.1}",
                elapsed.as_secs_f64()
            );
            let speed = relayed as f64 / elapsed.as_secs_f64().max(0.001);
            status
                .line(format!(
                    "# done: {} in {}, {}/s",
                    format_size(relayed as f64),
                    format_duration(elapsed),
                    format_size(speed)
                ))
                .await;
            status.finish();
        }
        Err(reason) => {
            // Dropping `outlet` unfinished aborts the Receiver's download.
            drop(outlet);
            Metrics::inc(&app.metrics.failed);
            eprintln!("event=failed id={id} reason={reason:?} bytes={relayed}");
            status
                .line(format!(
                    "# failed: {reason} after {}",
                    format_size(relayed as f64)
                ))
                .await;
        }
    }
}

/// Announces the Transfer URL and heartbeats until a Claim, expiry, shutdown, or the Sender leaving.
async fn wait_for_claim(
    app: &App,
    id: u64,
    code: &str,
    name: &str,
    wait: Duration,
    mut claim: oneshot::Receiver<Claim>,
    status: &Outlet,
) -> Option<Claim> {
    let url = app.transfer_url(code, name);
    let (curl, wget) = if filename::needs_no_encoding(name) {
        (format!("curl -O {url}"), format!("wget {url}"))
    } else {
        let quoted = filename::shell_quote(name);
        (
            format!("curl -o {quoted} {url}"),
            format!("wget -O {quoted} {url}"),
        )
    };
    for line in [
        url.clone(),
        "# receive with one of:".into(),
        format!("#   {curl}"),
        format!("#   {wget}"),
        format!(
            "# waiting for receiver (expires in {})…",
            format_duration(wait)
        ),
    ] {
        status.line(line).await;
    }

    let deadline = Instant::now() + wait;
    let heartbeat = app.cfg.heartbeat;
    let mut beats = interval_at(Instant::now() + heartbeat, heartbeat);
    let mut shutdown = app.shutdown_signal();
    loop {
        // A Claim that races with any other event wins: `withdraw` then returns false
        // and the loop comes back round to the ready `claim`.
        tokio::select! {
            biased;
            c = &mut claim => return c.ok(),
            _ = sleep_until(deadline) => {
                if app.withdraw(code) {
                    Metrics::inc(&app.metrics.expired);
                    eprintln!("event=expired id={id}");
                    status.line(format!("# expired: no receiver within {}", format_duration(wait))).await;
                    return None;
                }
            }
            // The borrow guard from `wait_for` is not Send, so drop it inside the block.
            _ = async { drop(shutdown.wait_for(|stopping| *stopping).await) } => {
                if app.withdraw(code) {
                    Metrics::inc(&app.metrics.expired);
                    eprintln!("event=closed_for_shutdown id={id}");
                    status.line("# server restarting, please resend").await;
                    return None;
                }
            }
            _ = status.closed() => {
                if app.withdraw(code) {
                    eprintln!("event=sender_left id={id}");
                    return None;
                }
            }
            _ = beats.tick() => {
                let left = deadline.saturating_duration_since(Instant::now());
                status.line(format!("# waiting for receiver ({} left)…", format_duration(left))).await;
            }
        }
    }
}
