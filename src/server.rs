//! HTTP routes: `PUT` opens a Transfer, `GET /<code>/<name>` claims one, `GET /` explains usage.

use std::{convert::Infallible, io, net::IpAddr, sync::Arc, time::Duration};

use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Full, combinators::BoxBody};
use hyper::{
    Method, Request, Response, StatusCode,
    body::Incoming,
    header::{self, HeaderValue},
};
use tokio::time::timeout;

use crate::{
    app::{App, ClaimOutcome, OpenError},
    body, code, filename,
    metrics::Metrics,
    transfer::{self, Opened},
    units::{format_duration, parse_duration},
};

pub type Body = BoxBody<Bytes, io::Error>;

pub async fn handle(
    app: Arc<App>,
    req: Request<Incoming>,
    client_ip: IpAddr,
) -> Result<Response<Body>, Infallible> {
    let resp = match *req.method() {
        Method::PUT => open(&app, req, client_ip).await,
        Method::GET => get(&app, &req, client_ip),
        Method::HEAD => text(StatusCode::OK, ""),
        Method::POST => text(
            StatusCode::METHOD_NOT_ALLOWED,
            "send with curl --upload-file (-T), not POST; see GET / for usage\n",
        ),
        _ => text(StatusCode::METHOD_NOT_ALLOWED, "method not allowed\n"),
    };
    Ok(resp)
}

async fn open(app: &Arc<App>, req: Request<Incoming>, sender_ip: IpAddr) -> Response<Body> {
    if app.is_shutting_down() {
        return text(
            StatusCode::SERVICE_UNAVAILABLE,
            "server restarting, try again shortly\n",
        );
    }
    let name = match filename::from_put_path(req.uri().path()) {
        Ok(name) => name,
        Err(e) => return text(StatusCode::BAD_REQUEST, &format!("{}\n", e.message())),
    };
    let wait = match requested_wait(app, req.uri().query()) {
        Ok(wait) => wait,
        Err(msg) => return text(StatusCode::BAD_REQUEST, &msg),
    };
    let len = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok()?.parse().ok());
    let (code, claim) = match app.open(name.clone(), len, sender_ip) {
        Ok(opened) => opened,
        Err(e) => {
            Metrics::inc(&app.metrics.rejected_busy);
            return match e {
                OpenError::Busy => text(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "server busy, try again later\n",
                ),
                OpenError::TooManyWaiting => text(
                    StatusCode::TOO_MANY_REQUESTS,
                    "too many of your Transfers are already waiting for a receiver\n",
                ),
            };
        }
    };
    let id = app.next_log_id();
    Metrics::inc(&app.metrics.opened);
    eprintln!(
        "event=opened id={id} sender={sender_ip} bytes={}",
        len.map_or("unknown".into(), |n: u64| n.to_string())
    );

    // Polling the upload once makes hyper answer `Expect: 100-continue` before our 200;
    // otherwise curl holds back the body for a second. A frame that is already there is kept.
    let mut upload = req.into_body();
    let first = timeout(Duration::ZERO, upload.frame()).await.ok().flatten();

    let (status, inlet) = body::channel(16);
    let opened = Opened {
        id,
        code,
        name,
        wait,
        claim,
        first,
    };
    tokio::spawn(transfer::run(app.clone(), opened, upload, status));
    let mut resp = Response::new(inlet.boxed());
    plain_text_headers(&mut resp);
    resp
}

fn requested_wait(app: &App, query: Option<&str>) -> Result<Duration, String> {
    let Some(raw) = query
        .into_iter()
        .flat_map(|q| q.split('&'))
        .find_map(|kv| kv.strip_prefix("wait="))
    else {
        return Ok(app.cfg.default_wait);
    };
    let max = format_duration(app.cfg.max_wait);
    match parse_duration(raw) {
        Some(d) if !d.is_zero() && d <= app.cfg.max_wait => Ok(d),
        Some(d) if !d.is_zero() => Err(format!("wait={raw} is longer than the maximum of {max}\n")),
        _ => Err(format!(
            "wait={raw} is not a duration; use e.g. wait=30m or wait=2h (maximum {max})\n"
        )),
    }
}

fn get(app: &App, req: &Request<Incoming>, ip: IpAddr) -> Response<Body> {
    let path = req.uri().path();
    if path == "/" {
        return text(StatusCode::OK, &usage(app));
    }
    let Some((code, name)) = path.strip_prefix('/').and_then(|p| p.split_once('/')) else {
        return text(StatusCode::NOT_FOUND, "not found\n");
    };
    // Link-preview bots and browsers must never Claim; they get the same answer whether or
    // not the Transfer URL is live, so this reveals nothing.
    if !is_allowed_receiver(req) {
        return text(
            StatusCode::OK,
            &format!("{}\nuse curl or wget to receive this file\n", usage(app)),
        );
    }
    if app.failed_claims.is_blocked(ip) {
        return text(
            StatusCode::TOO_MANY_REQUESTS,
            "too many failed attempts, try again in a minute\n",
        );
    }
    let claimed = match filename::url_decode(name) {
        Some(name) if code::is_well_formed(code) => app.claim(code, &name, ip),
        _ => ClaimOutcome::NotFound,
    };
    match claimed {
        ClaimOutcome::NotFound => {
            app.failed_claims.record(ip);
            Metrics::inc(&app.metrics.failed_claims);
            text(StatusCode::NOT_FOUND, "not found\n")
        }
        ClaimOutcome::Claimed { name, len, inlet } => {
            let mut resp = Response::new(inlet.boxed());
            let h = resp.headers_mut();
            h.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            );
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            if let Ok(v) = HeaderValue::from_str(&filename::content_disposition(&name)) {
                h.insert(header::CONTENT_DISPOSITION, v);
            }
            if let Some(len) = len {
                h.insert(header::CONTENT_LENGTH, len.into());
            }
            resp
        }
    }
}

fn is_allowed_receiver(req: &Request<Incoming>) -> bool {
    let ua = req
        .headers()
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let ua = ua.to_ascii_lowercase();
    ["curl/", "wget/", "wget2/"]
        .iter()
        .any(|p| ua.starts_with(p))
}

fn usage(app: &App) -> String {
    let base = &app.cfg.public_url;
    let default_wait = format_duration(app.cfg.default_wait);
    let max_wait = format_duration(app.cfg.max_wait);
    format!(
        "catsed: relay a file from one machine to another with curl. Nothing is stored.

SEND
  curl -T report.pdf {base}/
  tar cz dir | curl -T - {base}/dir.tar.gz      (stdin; name it in the URL, default \"data\")
  curl -T report.pdf '{base}/?wait=2h'          (wait up to {max_wait} for a receiver; default {default_wait})

  The first line printed is the Transfer URL. Keep the command running: the file
  streams straight to the receiver once they start downloading.
  In scripts, add -N so curl prints the URL immediately:  curl -N -T f {base}/ | head -1

RECEIVE (single use: the first download wins)
  curl -O {base}/<code>/report.pdf
  wget {base}/<code>/report.pdf

NOTES
  - Only curl and wget can receive; browsers and link previews just see this page.
  - Either side disconnecting ends the Transfer; there is no resume.
  - catsed sees your bytes in transit. For end-to-end secrecy, encrypt first:
      age -p report.pdf | curl -T - {base}/report.pdf.age
  - A server restart closes Transfers still waiting for a receiver; just resend.
"
    )
}

fn text(status: StatusCode, body: &str) -> Response<Body> {
    let body: Body = if body.is_empty() {
        Empty::new().map_err(|e: Infallible| match e {}).boxed()
    } else {
        Full::new(Bytes::from(body.to_owned()))
            .map_err(|e: Infallible| match e {})
            .boxed()
    };
    let mut resp = Response::new(body);
    *resp.status_mut() = status;
    plain_text_headers(&mut resp);
    resp
}

fn plain_text_headers(resp: &mut Response<Body>) {
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
}

pub async fn metrics(app: Arc<App>, req: Request<Incoming>) -> Result<Response<Body>, Infallible> {
    if req.uri().path() != "/metrics" {
        return Ok(text(StatusCode::NOT_FOUND, "not found\n"));
    }
    let (waiting, active) = app.counts();
    Ok(text(StatusCode::OK, &app.metrics.render(waiting, active)))
}
