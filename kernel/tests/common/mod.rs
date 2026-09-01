#![allow(dead_code)]
use hearth_sync_kernel::keys::{HouseholdRoot, SoftSigner};
use hearth_sync_kernel::op::Value;
use hearth_sync_kernel::replica::{Config, Replica};

pub const APP: &str = "lullaby";
pub const T0: u64 = 1_727_000_000_000;
pub const DAY: u64 = 24 * 60 * 60 * 1000;

pub fn seed() -> [u8; 64] {
    core::array::from_fn(|i| i as u8)
}

pub fn root() -> HouseholdRoot {
    HouseholdRoot::from_seed(seed())
}

/// A device replica with device secret `[n; 32]`, not yet enrolled.
pub fn device(n: u8, cfg: Config) -> Replica {
    Replica::new(APP, root(), Box::new(SoftSigner::from_secret([n; 32])), cfg)
}

/// A device that has enrolled itself at `now`.
pub fn joined(n: u8, now: u64) -> Replica {
    joined_with(n, now, Config::default())
}

pub fn joined_with(n: u8, now: u64, cfg: Config) -> Replica {
    let mut r = device(n, cfg);
    r.enroll_self(&format!("device {n}"), now).unwrap();
    r
}

pub fn observer() -> Replica {
    Replica::observer(APP, root().enroll_public(APP), Config::default())
}

pub fn v(n: i64) -> Value {
    Value::Int(n)
}

pub fn t(s: &str) -> Value {
    Value::Text(s.to_string())
}
