//! The file's name, taken from the last segment of the Sender's `PUT` path.

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};

pub const DEFAULT: &str = "data";
const MAX_BYTES: usize = 255;

/// Characters left as-is in a Transfer URL: RFC 3986 unreserved.
const URL_SAFE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

#[derive(Debug, PartialEq)]
pub enum Invalid {
    NotUtf8,
    ControlCharacter,
    TooLong,
}

impl Invalid {
    pub fn message(&self) -> &'static str {
        match self {
            Invalid::NotUtf8 => "filename is not valid UTF-8",
            Invalid::ControlCharacter => "filename contains control characters",
            Invalid::TooLong => "filename is longer than 255 bytes",
        }
    }
}

/// `/report.pdf` -> `report.pdf`, `/` or `/-` -> `data`, `/x/C%3A%5Cy%5Cz.txt` -> `z.txt`.
pub fn from_put_path(path: &str) -> Result<String, Invalid> {
    let segment = path.rsplit('/').next().unwrap_or("");
    let decoded = percent_decode_str(segment)
        .decode_utf8()
        .map_err(|_| Invalid::NotUtf8)?;
    let base = decoded.rsplit(['/', '\\']).next().unwrap_or("").trim();
    if base.chars().any(char::is_control) {
        return Err(Invalid::ControlCharacter);
    }
    if matches!(base, "" | "-" | "." | "..") {
        return Ok(DEFAULT.to_owned());
    }
    if base.len() > MAX_BYTES {
        return Err(Invalid::TooLong);
    }
    Ok(base.to_owned())
}

pub fn url_encode(name: &str) -> String {
    utf8_percent_encode(name, URL_SAFE).to_string()
}

pub fn url_decode(segment: &str) -> Option<String> {
    percent_decode_str(segment)
        .decode_utf8()
        .ok()
        .map(|s| s.into_owned())
}

/// `Content-Disposition` with an ASCII fallback plus the exact UTF-8 name (RFC 6266).
pub fn content_disposition(name: &str) -> String {
    let ascii: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!(
        "attachment; filename=\"{ascii}\"; filename*=UTF-8''{}",
        url_encode(name)
    )
}

/// Whether `curl -O` / `wget` would save the file under exactly this name.
pub fn needs_no_encoding(name: &str) -> bool {
    url_encode(name) == name
}

/// Single-quotes a name for a POSIX shell.
pub fn shell_quote(name: &str) -> String {
    format!("'{}'", name.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_paths() {
        assert_eq!(from_put_path("/report.pdf").unwrap(), "report.pdf");
        assert_eq!(from_put_path("/").unwrap(), "data");
        assert_eq!(from_put_path("/-").unwrap(), "data");
        assert_eq!(from_put_path("/..").unwrap(), "data");
        assert_eq!(
            from_put_path("/dir/my%20report.pdf").unwrap(),
            "my report.pdf"
        );
        assert_eq!(from_put_path("/..%2F..%2Fetc%2Fpasswd").unwrap(), "passwd");
        assert_eq!(
            from_put_path("/C%3A%5Cx%5Creport.pdf").unwrap(),
            "report.pdf"
        );
        assert_eq!(
            from_put_path("/r%C3%A9sum%C3%A9.txt").unwrap(),
            "résumé.txt"
        );
        assert_eq!(from_put_path("/a%0Ab"), Err(Invalid::ControlCharacter));
        assert_eq!(from_put_path("/%FF"), Err(Invalid::NotUtf8));
        assert_eq!(
            from_put_path(&format!("/{}", "a".repeat(256))),
            Err(Invalid::TooLong)
        );
        assert!(from_put_path(&format!("/{}", "a".repeat(255))).is_ok());
    }

    #[test]
    fn encoding_round_trips() {
        let name = "my report (1).pdf";
        assert_eq!(url_encode(name), "my%20report%20%281%29.pdf");
        assert_eq!(url_decode(&url_encode(name)).unwrap(), name);
        assert!(needs_no_encoding("report.pdf"));
        assert!(!needs_no_encoding(name));
    }

    #[test]
    fn disposition_and_quoting() {
        assert_eq!(
            content_disposition("résumé \"x\".txt"),
            "attachment; filename=\"r_sum_ _x_.txt\"; filename*=UTF-8''r%C3%A9sum%C3%A9%20%22x%22.txt"
        );
        assert_eq!(shell_quote("it's.txt"), r"'it'\''s.txt'");
    }
}
