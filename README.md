# catsed

Relay a file from one machine to another with nothing but `curl` (or `wget`) on either end.
catsed stores nothing: bytes stream from the Sender straight to the Receiver while both are connected.

```console
sender$ curl -T report.pdf https://catsed.example/
https://catsed.example/Ak9zL/report.pdf
# receive with one of:
#   curl -O https://catsed.example/Ak9zL/report.pdf
#   wget https://catsed.example/Ak9zL/report.pdf
# waiting for receiver (expires in 10m)…
# claimed by 203.0.113.7
# done: 1.2 GiB in 2m3s, 9.8 MiB/s

receiver$ curl -O https://catsed.example/Ak9zL/report.pdf
```

- **Send stdin:** `tar cz dir | curl -T - https://catsed.example/dir.tar.gz` (unnamed stdin is called `data`).
- **Wait longer:** `curl -T f 'https://catsed.example/?wait=2h'`, up to 24h.
- **Scripts:** add `-N` so curl prints the URL immediately when stdout is a pipe: `curl -N -T f https://catsed.example/ | head -1`.
- **Single use:** the first Receiver wins; afterwards the URL returns `404`.
- **No resume:** if either side disconnects, the Transfer fails and both curls exit non-zero.
- **Encryption:** catsed sees bytes in transit; encrypt first for end-to-end secrecy, e.g. `age -p f | curl -T - https://catsed.example/f.age`.

Only curl and wget can Claim a Transfer. Browsers and link-preview bots (Slack, Discord, …) get the usage page, so pasting a Transfer URL into a chat does not burn it.

Terms are defined in [GLOSSARY.md](GLOSSARY.md); design decisions are in [docs/adr](docs/adr).

## Deployment

```
internet ──443──▶ router: nginx `stream` (TLS, PROXY protocol) ──▶ NAS: catsed container :8080
```

catsed must sit behind an **L4 (`stream`) proxy, not an HTTP reverse proxy**: nginx's `http` proxy stops forwarding the upload once catsed has started its streamed response ([ADR 0002](docs/adr/0002-l4-proxy-in-front.md)).

1. **Image.** GitHub Actions tests every push and publishes `ghcr.io/<owner>/catsed` (`latest`, `sha-…`, and `X.Y.Z` for `vX.Y.Z` tags). GHCR packages start out private: set the package's visibility to public once, so the NAS can pull without credentials.
2. **NAS.** Copy [`deploy/compose.yaml`](deploy/compose.yaml) and [`deploy/.env.example`](deploy/.env.example) (renamed to `.env`) to the NAS, fill in `.env`, then `docker compose up -d`. In the Synology firewall, allow port 8080 only from the nginx container's IP.
3. **Router.** Add [`deploy/nginx/catsed-stream.conf`](deploy/nginx/catsed-stream.conf) at the top level of nginx.conf (outside `http {}`); it needs the stream module (OpenWrt: `nginx-mod-stream`). If 443 is shared with other HTTPS sites, use [`catsed-stream-sni.conf`](deploy/nginx/catsed-stream-sni.conf) instead, which also needs `--with-stream_realip_module`. Forward only 443 (and 80 if the router renews Let's Encrypt itself) to it.

Deploying restarts the single instance ([ADR 0001](docs/adr/0001-single-replica.md)): Transfers still waiting for a Receiver are closed with "please resend", and Claimed ones get up to 120s to finish.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `CATSED_PUBLIC_URL` | `http://<listen>` | Base of every Transfer URL, e.g. `https://catsed-you.synology.me` |
| `CATSED_TRUSTED_PROXY` | none | Comma-separated proxy IPs; connections from them must start with a PROXY v1 header |
| `CATSED_LISTEN` | `0.0.0.0:8080` | Public listener |
| `CATSED_METRICS_LISTEN` | `0.0.0.0:9090` | Prometheus `/metrics` (never publish it); `off` disables |
| `CATSED_MAX_RATE` | unlimited | Global speed limit shared by all Transfers, e.g. `10MiB` (per second) |
| `CATSED_MAX_TRANSFERS` | `5` | Concurrent Transfers, waiting plus Claimed |
| `CATSED_MAX_WAITING_PER_IP` | `2` | Waiting Transfers per Sender IP |
| `CATSED_MAX_FAILED_CLAIMS_PER_MIN` | `10` | Unknown-URL lookups per IP per minute before `429` |
| `CATSED_DEFAULT_WAIT` / `CATSED_MAX_WAIT` | `10m` / `24h` | How long a Sender waits for a Receiver, default and `?wait=` ceiling |
| `CATSED_IDLE_TIMEOUT` | `60s` | Abort a Claimed Transfer when no bytes move this long |
| `CATSED_HEARTBEAT` | `30s` | Status line interval while waiting; keeps proxies from timing out |
| `CATSED_SHUTDOWN_GRACE` | `115s` | How long Claimed Transfers may finish after SIGTERM |

Logs record Transfer events, sizes and IPs, never Transfer Codes or file names.

## Development

```sh
cargo test    # unit tests plus end-to-end tests that drive the real curl and wget
```
