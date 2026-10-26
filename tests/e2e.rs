//! End-to-end tests: the real catsed binary driven by the real `curl` and `wget` binaries.

use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Output, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

struct Server {
    child: Child,
    base: String,
    dir: PathBuf,
}

impl Server {
    fn start(test: &str, env: &[(&str, &str)]) -> Self {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let base = format!("http://127.0.0.1:{port}");
        let child = Command::new(env!("CARGO_BIN_EXE_catsed"))
            .env("CATSED_LISTEN", format!("127.0.0.1:{port}"))
            .env("CATSED_METRICS_LISTEN", "off")
            .env("CATSED_PUBLIC_URL", &base)
            .envs(env.iter().copied())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(Instant::now() < deadline, "catsed did not start");
            thread::sleep(Duration::from_millis(20));
        }
        let dir = std::env::temp_dir().join(format!("catsed-e2e-{}-{test}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Server { child, base, dir }
    }

    fn file(&self, name: &str, data: &[u8]) -> PathBuf {
        let path = self.dir.join(name);
        std::fs::write(&path, data).unwrap();
        path
    }

    fn sigterm(&self) {
        let ok = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status()
            .unwrap();
        assert!(ok.success());
    }

    fn wait_exit(&mut self, within: Duration) -> ExitStatus {
        let deadline = Instant::now() + within;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "catsed did not exit");
            thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A running Sender: `curl -N` with its status lines collected as they stream in.
struct Sending {
    child: Child,
    lines: Arc<Mutex<Vec<String>>>,
    new_line: mpsc::Receiver<()>,
}

impl Sending {
    fn file(path: &Path, url: &str, extra: &[&str]) -> Self {
        let mut cmd = Command::new("curl");
        cmd.args(["-sS", "-N"])
            .args(extra)
            .arg("-T")
            .arg(path)
            .arg(url);
        Self::spawn(cmd, None)
    }

    fn stdin(data: Vec<u8>, url: &str) -> Self {
        let mut cmd = Command::new("curl");
        cmd.args(["-sS", "-N", "-T", "-", url]);
        Self::spawn(cmd, Some(data))
    }

    fn spawn(mut cmd: Command, stdin: Option<Vec<u8>>) -> Self {
        let mut child = cmd
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        if let Some(data) = stdin {
            let mut pipe = child.stdin.take().unwrap();
            thread::spawn(move || {
                let _ = pipe.write_all(&data);
            });
        }
        let lines = Arc::new(Mutex::new(Vec::new()));
        let (tx, new_line) = mpsc::channel();
        let stdout = child.stdout.take().unwrap();
        let sink = lines.clone();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                sink.lock().unwrap().push(line);
                let _ = tx.send(());
            }
        });
        Sending {
            child,
            lines,
            new_line,
        }
    }

    /// Waits for a status line matching `pred` and returns it.
    fn wait_line(&self, pred: impl Fn(&str) -> bool) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(l) = self.lines.lock().unwrap().iter().find(|l| pred(l)) {
                return l.clone();
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if self.new_line.recv_timeout(left).is_err() {
                panic!("no matching line; got {:?}", self.lines.lock().unwrap());
            }
        }
    }

    fn url(&self) -> String {
        self.wait_line(|l| l.starts_with("http"))
    }

    fn finish(mut self) -> (ExitStatus, Vec<String>) {
        let status = self.child.wait().unwrap();
        thread::sleep(Duration::from_millis(50));
        (status, self.lines.lock().unwrap().clone())
    }
}

fn data(len: usize) -> Vec<u8> {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

fn curl(args: &[&str]) -> Output {
    Command::new("curl").arg("-sS").args(args).output().unwrap()
}

fn status_of(url: &str, extra: &[&str]) -> String {
    let out = curl(&[extra, &["-o", "/dev/null", "-w", "%{http_code}", url]].concat());
    String::from_utf8(out.stdout).unwrap()
}

fn receive_curl(url: &str, out: &Path) -> ExitStatus {
    Command::new("curl")
        .args(["-sS", "-o"])
        .arg(out)
        .arg(url)
        .status()
        .unwrap()
}

#[test]
fn file_via_curl() {
    let srv = Server::start("file_via_curl", &[]);
    let content = data(3 << 20);
    let src = srv.file("report.pdf", &content);
    let sending = Sending::file(&src, &format!("{}/", srv.base), &[]);

    let url = sending.url();
    let code = url
        .strip_prefix(&format!("{}/", srv.base))
        .unwrap()
        .strip_suffix("/report.pdf")
        .unwrap();
    assert_eq!(code.len(), 5);
    sending.wait_line(|l| l == format!("#   curl -O {url}"));
    sending.wait_line(|l| l == format!("#   wget {url}"));
    sending.wait_line(|l| l == "# waiting for receiver (expires in 10m)…");

    let out = srv.dir.join("out");
    assert!(receive_curl(&url, &out).success());
    assert_eq!(std::fs::read(&out).unwrap(), content);

    let (status, lines) = sending.finish();
    assert!(status.success());
    assert!(
        lines.iter().any(|l| l == "# claimed by 127.0.0.1"),
        "{lines:?}"
    );
    assert!(
        lines.last().unwrap().starts_with("# done: 3.0 MiB in "),
        "{lines:?}"
    );
}

#[test]
fn stdin_via_wget() {
    let srv = Server::start("stdin_via_wget", &[]);
    let content = data(2 << 20);
    let sending = Sending::stdin(content.clone(), &format!("{}/", srv.base));
    let url = sending.url();
    assert!(url.ends_with("/data"), "{url}");

    let out = srv.dir.join("out");
    let ok = Command::new("wget")
        .args(["-q", "-O"])
        .arg(&out)
        .arg(&url)
        .status()
        .unwrap();
    assert!(ok.success());
    assert_eq!(std::fs::read(&out).unwrap(), content);
    assert!(sending.finish().0.success());
}

#[test]
fn names_needing_encoding() {
    let srv = Server::start("names_needing_encoding", &[]);
    let sending = Sending::stdin(
        b"hello".to_vec(),
        &format!("{}/my%20report%20(1).txt", srv.base),
    );
    let url = sending.url();
    assert!(url.ends_with("/my%20report%20%281%29.txt"), "{url}");
    sending.wait_line(|l| l == format!("#   curl -o 'my report (1).txt' {url}"));

    let out = curl(&["-D", "-", "-o", "/dev/null", &url]);
    let headers = String::from_utf8_lossy(&out.stdout).to_ascii_lowercase();
    assert!(
        headers.contains("content-type: application/octet-stream"),
        "{headers}"
    );
    assert!(
        headers.contains("filename*=utf-8''my%20report%20%281%29.txt"),
        "{headers}"
    );
    assert!(sending.finish().0.success());
}

#[test]
fn url_is_single_use_and_name_must_match() {
    let srv = Server::start("single_use", &[]);
    let src = srv.file("a.txt", b"secret");
    let sending = Sending::file(&src, &format!("{}/", srv.base), &[]);
    let url = sending.url();
    let wrong_name = url.replace("/a.txt", "/b.txt");
    let no_name = url.trim_end_matches("/a.txt").to_owned();

    assert_eq!(status_of(&wrong_name, &[]), "404");
    assert_eq!(status_of(&no_name, &[]), "404");
    let first = curl(&[&url]);
    assert_eq!(first.stdout, b"secret");
    assert_eq!(status_of(&url, &[]), "404");
    assert!(sending.finish().0.success());
}

#[test]
fn only_curl_and_wget_can_claim() {
    let srv = Server::start("only_curl_and_wget", &[]);
    let src = srv.file("a.txt", b"payload");
    let sending = Sending::file(&src, &format!("{}/", srv.base), &[]);
    let url = sending.url();

    for ua in [
        "Mozilla/5.0 (X11; Linux x86_64)",
        "Slackbot-LinkExpanding 1.0",
        "Discordbot/2.0",
    ] {
        let out = curl(&["-A", ua, &url]);
        let body = String::from_utf8(out.stdout).unwrap();
        assert!(
            body.contains("use curl or wget to receive this file"),
            "{ua}: {body}"
        );
    }
    assert_eq!(status_of(&url, &["-I"]), "200");
    assert_eq!(curl(&[&url]).stdout, b"payload");
    assert!(sending.finish().0.success());
}

#[test]
fn wait_parameter_and_expiry() {
    let srv = Server::start("wait_parameter", &[("CATSED_HEARTBEAT", "1s")]);
    let src = srv.file("a.txt", b"x");
    for bad in ["25h", "soon", "0"] {
        let out = curl(&[
            "-T",
            src.to_str().unwrap(),
            &format!("{}/?wait={bad}", srv.base),
            "-w",
            "%{http_code}",
        ]);
        assert!(
            String::from_utf8_lossy(&out.stdout).ends_with("400"),
            "{bad}"
        );
    }

    let sending = Sending::file(&src, &format!("{}/?wait=3s", srv.base), &[]);
    sending.wait_line(|l| l == "# waiting for receiver (expires in 3s)…");
    sending.wait_line(|l| l.starts_with("# waiting for receiver (") && l.ends_with(" left)…"));
    let url = sending.url();
    let (status, lines) = sending.finish();
    assert!(!status.success(), "an expired Transfer must make curl fail");
    assert!(
        lines
            .iter()
            .any(|l| l == "# expired: no receiver within 3s"),
        "{lines:?}"
    );
    assert_eq!(status_of(&url, &[]), "404");
}

#[test]
fn concurrency_limits() {
    let put = |srv: &Server, src: &Path| {
        let out = curl(&[
            "-T",
            src.to_str().unwrap(),
            &format!("{}/", srv.base),
            "-w",
            "%{http_code}",
        ]);
        String::from_utf8(out.stdout).unwrap()
    };

    let srv = Server::start("limits_per_ip", &[("CATSED_MAX_WAITING_PER_IP", "2")]);
    let src = srv.file("a.txt", b"x");
    let waiting: Vec<_> = (0..2)
        .map(|_| Sending::file(&src, &format!("{}/", srv.base), &[]))
        .collect();
    waiting.iter().for_each(|s| drop(s.url()));
    let third = put(&srv, &src);
    assert!(
        third.contains("already waiting") && third.ends_with("429"),
        "{third}"
    );

    let srv = Server::start("limits_total", &[("CATSED_MAX_TRANSFERS", "1")]);
    let src = srv.file("a.txt", b"x");
    let only = Sending::file(&src, &format!("{}/", srv.base), &[]);
    only.url();
    let second = put(&srv, &src);
    assert!(
        second.contains("server busy") && second.ends_with("503"),
        "{second}"
    );
}

#[test]
fn failed_claims_are_throttled() {
    let srv = Server::start(
        "failed_claims",
        &[("CATSED_MAX_FAILED_CLAIMS_PER_MIN", "3")],
    );
    let src = srv.file("a.txt", b"x");
    let sending = Sending::file(&src, &format!("{}/", srv.base), &[]);
    let url = sending.url();
    for guess in ["AAAAA", "BBBBB", "CCCCC"] {
        assert_eq!(
            status_of(&format!("{}/{guess}/a.txt", srv.base), &[]),
            "404"
        );
    }
    assert_eq!(
        status_of(&url, &[]),
        "429",
        "even a correct URL is refused while throttled"
    );
}

#[test]
fn receiver_disconnect_fails_the_sender() {
    let srv = Server::start("receiver_disconnect", &[("CATSED_MAX_RATE", "1MiB")]);
    let src = srv.file("big.bin", &data(8 << 20));
    let sending = Sending::file(&src, &format!("{}/", srv.base), &[]);
    let url = sending.url();
    let mut receiver = Command::new("curl")
        .args(["-sS", "-o", "/dev/null", &url])
        .spawn()
        .unwrap();
    sending.wait_line(|l| l.starts_with("# claimed"));
    thread::sleep(Duration::from_millis(700));
    receiver.kill().unwrap();
    receiver.wait().unwrap();

    let (status, lines) = sending.finish();
    assert!(!status.success());
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("# failed: receiver disconnected")),
        "{lines:?}"
    );
}

#[test]
fn sender_disconnect_fails_the_receiver() {
    let srv = Server::start("sender_disconnect", &[("CATSED_MAX_RATE", "1MiB")]);
    // Both a known Content-Length (file) and a chunked upload (stdin).
    let src = srv.file("big.bin", &data(8 << 20));
    for mut sending in [
        Sending::file(&src, &format!("{}/", srv.base), &[]),
        Sending::stdin(data(8 << 20), &format!("{}/", srv.base)),
    ] {
        let url = sending.url();
        let receiver = Command::new("curl")
            .args(["-sS", "-o", "/dev/null", &url])
            .spawn()
            .unwrap();
        sending.wait_line(|l| l.starts_with("# claimed"));
        thread::sleep(Duration::from_millis(700));
        sending.child.kill().unwrap();
        let status = receiver.wait_with_output().unwrap().status;
        assert!(
            !status.success(),
            "a broken Transfer must make the receiving curl fail ({url})"
        );
    }
}

#[test]
fn global_speed_limit() {
    let srv = Server::start("speed_limit", &[("CATSED_MAX_RATE", "1MiB")]);
    let content = data(3 << 20);
    let src = srv.file("a.bin", &content);
    let sending = Sending::file(&src, &format!("{}/", srv.base), &[]);
    let url = sending.url();
    let out = srv.dir.join("out");
    let start = Instant::now();
    assert!(receive_curl(&url, &out).success());
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(2500),
        "3 MiB at 1 MiB/s took only {elapsed:?}"
    );
    assert_eq!(std::fs::read(&out).unwrap(), content);
    let (_, lines) = sending.finish();
    assert!(lines.last().unwrap().contains("1.0 MiB/s"), "{lines:?}");
}

#[test]
fn proxy_protocol_from_trusted_peer() {
    let srv = Server::start("proxy_protocol", &[("CATSED_TRUSTED_PROXY", "127.0.0.1")]);
    let src = srv.file("a.txt", b"via proxy");
    let sending = Sending::file(
        &src,
        &format!("{}/", srv.base),
        &["--haproxy-clientip", "198.51.100.1"],
    );
    let url = sending.url();
    let got = curl(&["--haproxy-clientip", "203.0.113.7", &url]);
    assert_eq!(got.stdout, b"via proxy");
    let (_, lines) = sending.finish();
    assert!(
        lines.iter().any(|l| l == "# claimed by 203.0.113.7"),
        "{lines:?}"
    );

    // A trusted peer that skips the PROXY header is cut off.
    assert!(!curl(&[&format!("{}/", srv.base)]).status.success());
}

#[test]
fn shutdown_closes_waiting_and_drains_claimed() {
    let mut srv = Server::start("shutdown", &[("CATSED_MAX_RATE", "1MiB")]);
    let small = srv.file("w.txt", b"x");
    let waiting = Sending::file(&small, &format!("{}/", srv.base), &[]);
    waiting.url();

    let content = data(2 << 20);
    let big = srv.file("big.bin", &content);
    let active = Sending::file(&big, &format!("{}/", srv.base), &[]);
    let url = active.url();
    let out = srv.dir.join("out");
    let receiver = Command::new("curl")
        .args(["-sS", "-o"])
        .arg(&out)
        .arg(&url)
        .spawn()
        .unwrap();
    active.wait_line(|l| l.starts_with("# claimed"));

    srv.sigterm();
    let (status, lines) = waiting.finish();
    assert!(!status.success());
    assert!(
        lines
            .iter()
            .any(|l| l == "# server restarting, please resend"),
        "{lines:?}"
    );

    assert!(receiver.wait_with_output().unwrap().status.success());
    assert_eq!(std::fs::read(&out).unwrap(), content);
    assert!(active.finish().0.success());
    assert!(srv.wait_exit(Duration::from_secs(5)).success());
}

#[test]
fn usage_and_methods() {
    let srv = Server::start("usage", &[]);
    let body = String::from_utf8(curl(&[&format!("{}/", srv.base)]).stdout).unwrap();
    assert!(
        body.contains(&format!("curl -T report.pdf {}/", srv.base)),
        "{body}"
    );
    assert_eq!(status_of(&format!("{}/", srv.base), &["-X", "POST"]), "405");
    assert_eq!(status_of(&format!("{}/favicon.ico", srv.base), &[]), "404");
}
