//! Human-friendly durations (`10m`, `1h30m`) and byte sizes (`10MiB`).

use std::time::Duration;

/// Parses `90`, `90s`, `10m`, `24h`, `1h30m`. A bare number means seconds.
pub fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(secs) = s.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let mut total: u64 = 0;
    let mut num = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            num.push(c);
            continue;
        }
        let n: u64 = num.parse().ok()?;
        num.clear();
        let unit = match c {
            's' => 1,
            'm' => 60,
            'h' => 3600,
            'd' => 86400,
            _ => return None,
        };
        total = total.checked_add(n.checked_mul(unit)?)?;
    }
    num.is_empty().then(|| Duration::from_secs(total))
}

/// Formats as `1h2m3s`, dropping zero parts; sub-second durations show as `0s`.
pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    let mut out = String::new();
    if h > 0 {
        out += &format!("{h}h");
    }
    if m > 0 {
        out += &format!("{m}m");
    }
    if s > 0 || out.is_empty() {
        out += &format!("{s}s");
    }
    out
}

/// Parses `1048576`, `500K`, `10MB`, `10MiB`, `1G`. `K/M/G` are decimal, `KiB/MiB/GiB` binary.
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: u64 = num.parse().ok()?;
    let mult: u64 = match unit.trim() {
        "" | "B" => 1,
        "K" | "KB" => 1_000,
        "M" | "MB" => 1_000_000,
        "G" | "GB" => 1_000_000_000,
        "KiB" => 1 << 10,
        "MiB" => 1 << 20,
        "GiB" => 1 << 30,
        _ => return None,
    };
    n.checked_mul(mult)
}

/// Formats bytes as `512 B`, `1.5 KiB`, `9.8 MiB`, `1.2 GiB`.
pub fn format_size(n: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{v:.0} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("90"), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration("10m"), Some(Duration::from_secs(600)));
        assert_eq!(parse_duration("1h30m"), Some(Duration::from_secs(5400)));
        assert_eq!(parse_duration("24h"), Some(Duration::from_secs(86400)));
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("10x"), None);
        assert_eq!(parse_duration("m"), None);
        assert_eq!(parse_duration("10m5"), None);
        assert_eq!(parse_duration("-5m"), None);
        assert_eq!(format_duration(Duration::from_secs(570)), "9m30s");
        assert_eq!(format_duration(Duration::from_secs(3600)), "1h");
        assert_eq!(format_duration(Duration::from_millis(300)), "0s");
    }

    #[test]
    fn sizes() {
        assert_eq!(parse_size("1024"), Some(1024));
        assert_eq!(parse_size("10MiB"), Some(10 << 20));
        assert_eq!(parse_size("10M"), Some(10_000_000));
        assert_eq!(parse_size("1x"), None);
        assert_eq!(parse_size("MiB"), None);
        assert_eq!(format_size(512.0), "512 B");
        assert_eq!(format_size(1536.0), "1.5 KiB");
        assert_eq!(format_size(1.2 * (1u64 << 30) as f64), "1.2 GiB");
    }
}
