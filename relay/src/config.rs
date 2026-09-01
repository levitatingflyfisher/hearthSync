//! Limits (docs/reference/relay-protocol.md, "Limits" and "Rate limits").

/// Every limit the handler enforces. The defaults are the protocol's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub window_ms: u64,
    pub max_envelope: u64,
    pub max_batch: u64,
    pub max_snapshot: u64,
    pub channel_quota: u64,
    pub max_total_bytes: u64,
    pub max_devices: u64,
    pub max_channels: u64,
    pub max_pull_entries: u64,
    pub max_pull_bytes: u64,
    pub retain_ms: u64,
    pub device_burst: u64,
    pub device_interval_ms: u64,
    pub enroll_burst: u64,
    pub enroll_interval_ms: u64,
    pub create_burst: u64,
    pub create_interval_ms: u64,
    pub channel_burst: u64,
    pub channel_interval_ms: u64,
    /// A channel with no write for this long is expired by the sweep.
    pub idle_ms: u64,
    /// Live read nonces one reader may hold in one channel.
    pub max_reader_nonces: u64,
}

pub const DAY_MS: u64 = 24 * 60 * 60 * 1000;

impl Default for Config {
    fn default() -> Self {
        Config {
            window_ms: 300_000,
            // An op's 64 KiB plus the envelope's framing, nonce, ref and tag.
            max_envelope: hearth_sync_kernel::MAX_OP_BYTES as u64 + 1024,
            max_batch: 64,
            max_snapshot: 8 << 20,
            channel_quota: 64 << 20,
            max_total_bytes: 8 << 30,
            max_devices: 32,
            max_channels: 1_000,
            max_pull_entries: 512,
            max_pull_bytes: 4 << 20,
            // The kernel's default horizon plus 30 days' grace.
            retain_ms: hearth_sync_kernel::DEFAULT_HORIZON_MS + 30 * DAY_MS,
            device_burst: 120,
            device_interval_ms: 500,
            enroll_burst: 16,
            enroll_interval_ms: 60_000,
            create_burst: 32,
            create_interval_ms: 60_000,
            channel_burst: 600,
            channel_interval_ms: 100,
            idle_ms: 400 * DAY_MS,
            max_reader_nonces: 16,
        }
    }
}

impl Config {
    /// The HTTP body limit: the largest request (a snapshot) plus room for its framing.
    pub fn max_body(&self) -> u64 {
        self.max_snapshot.max(self.max_batch.saturating_mul(self.max_envelope + 8)) + 64 * 1024
    }

    /// Set one limit by its protocol name (the vectors' config keys).
    pub fn set(&mut self, name: &str, v: u64) -> bool {
        let f = match name {
            "window_ms" => &mut self.window_ms,
            "max_envelope" => &mut self.max_envelope,
            "max_batch" => &mut self.max_batch,
            "max_snapshot" => &mut self.max_snapshot,
            "channel_quota" => &mut self.channel_quota,
            "max_total_bytes" => &mut self.max_total_bytes,
            "max_devices" => &mut self.max_devices,
            "max_channels" => &mut self.max_channels,
            "max_pull_entries" => &mut self.max_pull_entries,
            "max_pull_bytes" => &mut self.max_pull_bytes,
            "retain_ms" => &mut self.retain_ms,
            "device_burst" => &mut self.device_burst,
            "device_interval_ms" => &mut self.device_interval_ms,
            "enroll_burst" => &mut self.enroll_burst,
            "enroll_interval_ms" => &mut self.enroll_interval_ms,
            "create_burst" => &mut self.create_burst,
            "create_interval_ms" => &mut self.create_interval_ms,
            "channel_burst" => &mut self.channel_burst,
            "channel_interval_ms" => &mut self.channel_interval_ms,
            "idle_ms" => &mut self.idle_ms,
            "max_reader_nonces" => &mut self.max_reader_nonces,
            _ => return false,
        };
        *f = v;
        true
    }
}
