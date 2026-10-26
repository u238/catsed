# Put an L4 (TCP) proxy in front of catsed, not an HTTP reverse proxy

A Transfer needs full-duplex HTTP/1.1: catsed answers the Sender's `PUT` with a streamed `200` (carrying the Transfer URL) long before it reads the request body. A spike showed that nginx's HTTP proxy (`proxy_pass` in an `http {}` block, even with `proxy_request_buffering off` and `proxy_http_version 1.1`) delivers the URL but stops forwarding the upload once the response has started: only the ~2.6 MB already sitting in socket buffers reached catsed, then the Transfer stalled. We therefore run nginx's `stream` module on the router. It terminates TLS and passes raw TCP to catsed, with PROXY protocol v1 carrying the client IP, which catsed parses only from trusted proxy addresses.

## Considered Options

- **nginx HTTP proxy**: rejected for the reason above. Without `proxy_http_version 1.1` it also buffers chunked (stdin) uploads to disk, and with default settings it rejects bodies over 1 MB.
- **A full-duplex HTTP proxy** (HAProxy, or Caddy with full duplex enabled): plausible, but untested, and it would replace the nginx already on the router.
- **TLS terminated inside catsed**, with nginx `stream` only passing TLS through by SNI: works, but catsed would have to manage certificates.

## Consequences

- catsed never sees `X-Forwarded-For`; the client IP comes from the PROXY protocol header.
- The router's nginx must include the `stream` module. If port 443 also serves other sites, `ssl_preread` routes by SNI.
- Clients talk HTTP/1.1 to catsed end to end (no HTTP/2), which is what the Sender flow needs anyway.
