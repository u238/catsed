//! Configuration from `CATSED_*` environment variables.

use std::{net::IpAddr, net::SocketAddr, time::Duration};

use crate::units::{parse_duration, parse_size};

#[derive(Debug, Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub metrics_listen: Option<SocketAddr>,
    /// Base of every Transfer URL, without a trailing slash.
    pub public_url: String,
    /// Peers that must open each connection with a PROXY protocol v1 header.
    pub trusted_proxies: Vec<IpAddr>,
    pub max_transfers: usize,
    pub max_waiting_per_ip: usize,
    pub max_failed_claims_per_min: u32,
    pub default_wait: Duration,
    pub max_wait: Duration,
    pub idle_timeout: Duration,
    pub heartbeat: Duration,
    /// Global relay speed limit in bytes per second, shared by all Transfers.
    pub max_rate: Option<u64>,
    pub shutdown_grace: Duration,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let var = |k: &str| get(k).filter(|v| !v.trim().is_empty());
        let duration = |k: &str, default: &str| {
            let raw = var(k).unwrap_or_else(|| default.to_owned());
            parse_duration(&raw).ok_or_else(|| format!("{k}: invalid duration {raw:?}"))
        };
        let number = |k: &str, default: u64| -> Result<u64, String> {
            var(k).map_or(Ok(default), |raw| {
                raw.trim()
                    .parse()
                    .map_err(|_| format!("{k}: invalid number {raw:?}"))
            })
        };
        let addr = |k: &str, raw: String| {
            raw.parse::<SocketAddr>()
                .map_err(|_| format!("{k}: invalid address {raw:?}"))
        };

        let listen = addr(
            "CATSED_LISTEN",
            var("CATSED_LISTEN").unwrap_or_else(|| "0.0.0.0:8080".into()),
        )?;
        let metrics_listen = match var("CATSED_METRICS_LISTEN").as_deref() {
            Some("off") => None,
            Some(raw) => Some(addr("CATSED_METRICS_LISTEN", raw.to_owned())?),
            None => Some("0.0.0.0:9090".parse().unwrap()),
        };
        let public_url = var("CATSED_PUBLIC_URL")
            .unwrap_or_else(|| format!("http://{listen}"))
            .trim_end_matches('/')
            .to_owned();
        if !public_url.starts_with("http://") && !public_url.starts_with("https://") {
            return Err(format!(
                "CATSED_PUBLIC_URL: must start with http:// or https://, got {public_url:?}"
            ));
        }
        let trusted_proxies = var("CATSED_TRUSTED_PROXY")
            .map(|raw| {
                raw.split(',')
                    .map(|ip| {
                        ip.trim()
                            .parse()
                            .map_err(|_| format!("CATSED_TRUSTED_PROXY: invalid IP {ip:?}"))
                    })
                    .collect::<Result<Vec<IpAddr>, _>>()
            })
            .transpose()?
            .unwrap_or_default();
        let max_rate = var("CATSED_MAX_RATE")
            .map(|raw| match parse_size(&raw) {
                Some(0) | None => Err(format!("CATSED_MAX_RATE: invalid size {raw:?}")),
                Some(n) => Ok(n),
            })
            .transpose()?;

        let cfg = Config {
            listen,
            metrics_listen,
            public_url,
            trusted_proxies,
            max_transfers: number("CATSED_MAX_TRANSFERS", 5)? as usize,
            max_waiting_per_ip: number("CATSED_MAX_WAITING_PER_IP", 2)? as usize,
            max_failed_claims_per_min: number("CATSED_MAX_FAILED_CLAIMS_PER_MIN", 10)? as u32,
            default_wait: duration("CATSED_DEFAULT_WAIT", "10m")?,
            max_wait: duration("CATSED_MAX_WAIT", "24h")?,
            idle_timeout: duration("CATSED_IDLE_TIMEOUT", "60s")?,
            heartbeat: duration("CATSED_HEARTBEAT", "30s")?,
            max_rate,
            shutdown_grace: duration("CATSED_SHUTDOWN_GRACE", "115s")?,
        };
        if cfg.default_wait > cfg.max_wait {
            return Err("CATSED_DEFAULT_WAIT must not exceed CATSED_MAX_WAIT".into());
        }
        if cfg.heartbeat.is_zero() || cfg.idle_timeout.is_zero() || cfg.default_wait.is_zero() {
            return Err(
                "CATSED_HEARTBEAT, CATSED_IDLE_TIMEOUT and CATSED_DEFAULT_WAIT must be positive"
                    .into(),
            );
        }
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(pairs: &[(&str, &str)]) -> Result<Config, String> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_lookup(|k| pairs.iter().find(|(pk, _)| pk == k).map(|(_, v)| v.clone()))
    }

    #[test]
    fn defaults() {
        let cfg = with(&[]).unwrap();
        assert_eq!(cfg.max_transfers, 5);
        assert_eq!(cfg.max_waiting_per_ip, 2);
        assert_eq!(cfg.default_wait, Duration::from_secs(600));
        assert_eq!(cfg.max_wait, Duration::from_secs(86400));
        assert_eq!(cfg.max_rate, None);
        assert_eq!(cfg.public_url, "http://0.0.0.0:8080");
    }

    #[test]
    fn overrides_and_errors() {
        let cfg = with(&[
            ("CATSED_PUBLIC_URL", "https://catsed.example/"),
            ("CATSED_TRUSTED_PROXY", "192.168.1.1, 10.0.0.2"),
            ("CATSED_MAX_RATE", "10MiB"),
            ("CATSED_METRICS_LISTEN", "off"),
        ])
        .unwrap();
        assert_eq!(cfg.public_url, "https://catsed.example");
        assert_eq!(cfg.trusted_proxies.len(), 2);
        assert_eq!(cfg.max_rate, Some(10 << 20));
        assert_eq!(cfg.metrics_listen, None);
        assert!(with(&[("CATSED_MAX_RATE", "0")]).is_err());
        assert!(with(&[("CATSED_TRUSTED_PROXY", "nope")]).is_err());
        assert!(with(&[("CATSED_DEFAULT_WAIT", "2d")]).is_err());
        assert!(with(&[("CATSED_PUBLIC_URL", "catsed.example")]).is_err());
    }
}
