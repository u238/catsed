//! Prometheus counters, served on the internal metrics port only.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

#[derive(Default)]
pub struct Metrics {
    pub opened: AtomicU64,
    pub claimed: AtomicU64,
    pub completed: AtomicU64,
    pub failed: AtomicU64,
    pub expired: AtomicU64,
    pub bytes_relayed: AtomicU64,
    pub rejected_busy: AtomicU64,
    pub failed_claims: AtomicU64,
}

impl Metrics {
    pub fn inc(counter: &AtomicU64) {
        counter.fetch_add(1, Relaxed);
    }

    pub fn render(&self, waiting: usize, active: usize) -> String {
        let counters = [
            (
                "catsed_transfers_opened_total",
                "Transfers opened by a Sender",
                &self.opened,
            ),
            (
                "catsed_transfers_claimed_total",
                "Transfers claimed by a Receiver",
                &self.claimed,
            ),
            (
                "catsed_transfers_completed_total",
                "Transfers fully delivered",
                &self.completed,
            ),
            (
                "catsed_transfers_failed_total",
                "Claimed Transfers that broke mid-stream",
                &self.failed,
            ),
            (
                "catsed_transfers_expired_total",
                "Transfers closed before any Claim",
                &self.expired,
            ),
            (
                "catsed_bytes_relayed_total",
                "Bytes relayed from Senders to Receivers",
                &self.bytes_relayed,
            ),
            (
                "catsed_rejected_busy_total",
                "Senders turned away by concurrency limits",
                &self.rejected_busy,
            ),
            (
                "catsed_failed_claims_total",
                "Claims for unknown or already-claimed Transfer URLs",
                &self.failed_claims,
            ),
        ];
        let mut out = String::new();
        for (name, help, value) in counters {
            out += &format!(
                "# HELP {name} {help}\n# TYPE {name} counter\n{name} {}\n",
                value.load(Relaxed)
            );
        }
        for (name, help, value) in [
            (
                "catsed_transfers_waiting",
                "Transfers waiting for a Receiver",
                waiting,
            ),
            (
                "catsed_transfers_active",
                "Claimed Transfers currently relaying",
                active,
            ),
        ] {
            out += &format!("# HELP {name} {help}\n# TYPE {name} gauge\n{name} {value}\n");
        }
        out
    }
}
