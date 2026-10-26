//! Shared state: the in-memory registry of live Transfers (see ADR 0001) and the limits on it.

use std::{
    collections::{HashMap, HashSet},
    net::IpAddr,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
};

use tokio::sync::{oneshot, watch};

use crate::{
    body::{self, Inlet, Outlet},
    code,
    config::Config,
    filename,
    limits::{FailedClaims, RateLimiter},
    metrics::Metrics,
};

/// Handed from the Receiver's request to the Sender's task at the moment of Claim.
pub struct Claim {
    pub receiver_ip: IpAddr,
    pub outlet: Outlet,
}

struct Waiting {
    name: String,
    len: Option<u64>,
    sender_ip: IpAddr,
    claim: oneshot::Sender<Claim>,
}

#[derive(Default)]
struct Registry {
    waiting: HashMap<String, Waiting>,
    /// Codes of Claimed Transfers still relaying, kept reserved so no new Transfer reuses them.
    active: HashSet<String>,
}

pub enum OpenError {
    Busy,
    TooManyWaiting,
}

pub enum ClaimOutcome {
    Claimed {
        name: String,
        len: Option<u64>,
        inlet: Inlet,
    },
    NotFound,
}

pub struct App {
    pub cfg: Config,
    pub metrics: Metrics,
    pub failed_claims: FailedClaims,
    pub rate: Option<RateLimiter>,
    registry: Mutex<Registry>,
    shutdown: watch::Sender<bool>,
    next_id: AtomicU64,
}

impl App {
    pub fn new(cfg: Config) -> Self {
        Self {
            metrics: Metrics::default(),
            failed_claims: FailedClaims::new(cfg.max_failed_claims_per_min),
            rate: cfg.max_rate.map(RateLimiter::new),
            registry: Mutex::default(),
            shutdown: watch::Sender::new(false),
            next_id: AtomicU64::new(1),
            cfg,
        }
    }

    pub fn transfer_url(&self, code: &str, name: &str) -> String {
        format!(
            "{}/{code}/{}",
            self.cfg.public_url,
            filename::url_encode(name)
        )
    }

    /// An id for correlating log lines without logging the Transfer Code.
    pub fn next_log_id(&self) -> u64 {
        self.next_id.fetch_add(1, Relaxed)
    }

    /// Registers a new waiting Transfer, enforcing the concurrency limits.
    pub fn open(
        &self,
        name: String,
        len: Option<u64>,
        sender_ip: IpAddr,
    ) -> Result<(String, oneshot::Receiver<Claim>), OpenError> {
        let mut reg = self.registry.lock().unwrap();
        if reg.waiting.len() + reg.active.len() >= self.cfg.max_transfers {
            return Err(OpenError::Busy);
        }
        if reg
            .waiting
            .values()
            .filter(|w| w.sender_ip == sender_ip)
            .count()
            >= self.cfg.max_waiting_per_ip
        {
            return Err(OpenError::TooManyWaiting);
        }
        let code = loop {
            let c = code::generate();
            if !reg.waiting.contains_key(&c) && !reg.active.contains(&c) {
                break c;
            }
        };
        let (claim, claim_rx) = oneshot::channel();
        reg.waiting.insert(
            code.clone(),
            Waiting {
                name,
                len,
                sender_ip,
                claim,
            },
        );
        Ok((code, claim_rx))
    }

    /// Claims a waiting Transfer if both the code and the name match.
    pub fn claim(&self, code: &str, name: &str, receiver_ip: IpAddr) -> ClaimOutcome {
        let mut reg = self.registry.lock().unwrap();
        if reg.waiting.get(code).is_none_or(|w| w.name != name) {
            return ClaimOutcome::NotFound;
        }
        let w = reg.waiting.remove(code).unwrap();
        let (outlet, inlet) = body::channel(8);
        if w.claim
            .send(Claim {
                receiver_ip,
                outlet,
            })
            .is_err()
        {
            return ClaimOutcome::NotFound;
        }
        reg.active.insert(code.to_owned());
        ClaimOutcome::Claimed {
            name: w.name,
            len: w.len,
            inlet,
        }
    }

    /// Withdraws a waiting Transfer. `false` means a Receiver claimed it first.
    pub fn withdraw(&self, code: &str) -> bool {
        self.registry.lock().unwrap().waiting.remove(code).is_some()
    }

    pub fn finish(&self, code: &str) {
        self.registry.lock().unwrap().active.remove(code);
    }

    pub fn counts(&self) -> (usize, usize) {
        let reg = self.registry.lock().unwrap();
        (reg.waiting.len(), reg.active.len())
    }

    pub fn begin_shutdown(&self) {
        self.shutdown.send_replace(true);
    }

    pub fn is_shutting_down(&self) -> bool {
        *self.shutdown.borrow()
    }

    pub fn shutdown_signal(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }
}
