//! PROXY protocol v1, sent by the trusted L4 proxy in front of catsed (see ADR 0002).

use std::{io, net::IpAddr};

use tokio::io::{AsyncRead, AsyncReadExt};

/// The v1 header is at most 107 bytes including the trailing CRLF.
const MAX_LEN: usize = 107;

/// Reads the header byte by byte so no application bytes are consumed.
/// Returns the client address, or `None` for `PROXY UNKNOWN`.
pub async fn read_v1<S: AsyncRead + Unpin>(stream: &mut S) -> io::Result<Option<IpAddr>> {
    let mut line = Vec::with_capacity(MAX_LEN);
    while !line.ends_with(b"\r\n") {
        if line.len() == MAX_LEN {
            return Err(invalid("PROXY header too long"));
        }
        line.push(stream.read_u8().await?);
    }
    let line = std::str::from_utf8(&line[..line.len() - 2])
        .map_err(|_| invalid("PROXY header not ASCII"))?;
    parse_v1(line).ok_or_else(|| invalid("malformed PROXY header"))
}

fn parse_v1(line: &str) -> Option<Option<IpAddr>> {
    let mut parts = line.split(' ');
    if parts.next()? != "PROXY" {
        return None;
    }
    let family = parts.next()?;
    if family == "UNKNOWN" {
        return Some(None);
    }
    let src: IpAddr = parts.next()?.parse().ok()?;
    let _dst: IpAddr = parts.next()?.parse().ok()?;
    let _sport: u16 = parts.next()?.parse().ok()?;
    let _dport: u16 = parts.next()?.parse().ok()?;
    let family_matches = matches!(
        (family, src),
        ("TCP4", IpAddr::V4(_)) | ("TCP6", IpAddr::V6(_))
    );
    (family_matches && parts.next().is_none()).then_some(Some(src))
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_headers() {
        assert_eq!(
            parse_v1("PROXY TCP4 203.0.113.7 192.168.1.10 51234 8080"),
            Some(Some("203.0.113.7".parse().unwrap()))
        );
        assert_eq!(
            parse_v1("PROXY TCP6 2001:db8::1 2001:db8::2 1 2"),
            Some(Some("2001:db8::1".parse().unwrap()))
        );
        assert_eq!(parse_v1("PROXY UNKNOWN"), Some(None));
        assert_eq!(parse_v1("PROXY TCP4 2001:db8::1 2001:db8::2 1 2"), None);
        assert_eq!(parse_v1("PROXY TCP4 1.2.3.4 5.6.7.8 1"), None);
        assert_eq!(parse_v1("GET / HTTP/1.1"), None);
    }

    #[tokio::test]
    async fn leaves_application_bytes_unread() {
        let mut input: &[u8] = b"PROXY TCP4 203.0.113.7 10.0.0.1 1 2\r\nGET / HTTP/1.1\r\n";
        let ip = read_v1(&mut input).await.unwrap();
        assert_eq!(ip, Some("203.0.113.7".parse().unwrap()));
        assert_eq!(input, b"GET / HTTP/1.1\r\n");
    }

    #[tokio::test]
    async fn rejects_missing_header() {
        let mut input: &[u8] = &[b'G'; 200];
        assert!(read_v1(&mut input).await.is_err());
    }
}
