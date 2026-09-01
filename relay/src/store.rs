//! Persistence: one SQLite database under the data directory (or in memory for tests).
//!
//! The relay holds only what the protocol names: per channel its app and household key,
//! device records, per-uploader logs of sealed envelopes and one snapshot per device.
//! Every request runs in one transaction, so a crash leaves a request wholly applied or
//! not at all.

use std::path::Path;

use dcbor::CBOR;
use hearth_sync_kernel::{sha256, DeviceId};
use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::wire::{pairs_cbor, Channel, Pairs};

pub use rusqlite::Error as StoreError;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY, value INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS channels (
    id BLOB PRIMARY KEY, app TEXT NOT NULL, household BLOB NOT NULL,
    next_ord INTEGER NOT NULL, bytes INTEGER NOT NULL, last_write INTEGER NOT NULL,
    generation INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS devices (
    channel BLOB NOT NULL, device BLOB NOT NULL, label TEXT, auth BLOB,
    cut_seq INTEGER, freeze_ord INTEGER,
    PRIMARY KEY (channel, device)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS logs (
    channel BLOB NOT NULL, uploader BLOB NOT NULL, last INTEGER NOT NULL,
    PRIMARY KEY (channel, uploader)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS entries (
    channel BLOB NOT NULL, uploader BLOB NOT NULL, seq INTEGER NOT NULL,
    ord INTEGER NOT NULL, stored_at INTEGER NOT NULL, env BLOB NOT NULL,
    PRIMARY KEY (channel, uploader, seq)
);
CREATE TABLE IF NOT EXISTS snapshots (
    channel BLOB NOT NULL, device BLOB NOT NULL, stored_at INTEGER NOT NULL,
    env BLOB NOT NULL,
    PRIMARY KEY (channel, device)
);
CREATE TABLE IF NOT EXISTS covers (
    channel BLOB NOT NULL, device BLOB NOT NULL, uploader BLOB NOT NULL, seq INTEGER NOT NULL,
    PRIMARY KEY (channel, device, uploader)
) WITHOUT ROWID;
";

/// A channel's row.
#[derive(Clone, Debug)]
pub struct ChannelRow {
    pub app: String,
    pub household: [u8; 32],
    pub next_ord: u64,
    pub bytes: u64,
    pub last_write: u64,
    /// Changes whenever the channel's logs are wiped: created, or expired and kept.
    pub generation: u64,
}

/// A device's record in a channel.
#[derive(Clone, Debug, Default)]
pub struct DeviceRow {
    pub label: Option<String>,
    pub auth: Option<Vec<u8>>,
    pub cut_seq: Option<u64>,
    pub freeze_ord: Option<u64>,
}

impl DeviceRow {
    pub fn enrolled(&self) -> bool {
        self.auth.is_some()
    }
    pub fn forgotten(&self) -> bool {
        self.cut_seq.is_some()
    }
}

pub struct Store {
    conn: Connection,
    epoch: u64,
}

// SQLite integers are i64. Seqs and ords are bounded by storage, times by the clock and
// sizes by the quotas, so they fit. A request's cut_seq and covers are any u64, so they
// (and freeze_ord beside them) are stored as their bits and read back whole: a value
// from 2^63 up reads as negative in SQL, which the prune query treats as covering all.
fn bits(n: u64) -> i64 {
    n as i64
}
fn unbits(n: i64) -> u64 {
    n as u64
}
fn i(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}
fn u(n: i64) -> u64 {
    u64::try_from(n).unwrap_or(0)
}
fn b32(v: Vec<u8>) -> [u8; 32] {
    v.try_into().unwrap_or([0; 32])
}

impl Store {
    /// Open (creating if needed) the database in `dir`, or an in-memory one.
    pub fn open(dir: Option<&Path>) -> Result<Store, StoreError> {
        Store::open_with(dir, true)
    }

    /// As [`Store::open`]; `durable = false` skips fsync (tests and the differential
    /// harness only: a crash may then lose committed requests).
    pub fn open_with(dir: Option<&Path>, durable: bool) -> Result<Store, StoreError> {
        let conn = match dir {
            Some(d) => Connection::open(d.join("relay.sqlite3"))?,
            None => Connection::open_in_memory()?,
        };
        // WAL for concurrent readers of the file (backups); temp_store in memory so a
        // read-only root filesystem never matters; foreign-free schema.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", if durable { "FULL" } else { "OFF" })?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        conn.execute_batch(SCHEMA)?;
        // Every start moves the epoch on (a new store starts at 1), so a read signed
        // before this start never passes after it (docs/reference/relay-protocol.md).
        conn.execute(
            "INSERT INTO meta (key, value) VALUES ('epoch', 1)
             ON CONFLICT (key) DO UPDATE SET value = value + 1",
            [],
        )?;
        let epoch = conn.query_row("SELECT value FROM meta WHERE key = 'epoch'", [], |r| r.get::<_, i64>(0)).map(u)?;
        Ok(Store { conn, epoch })
    }

    /// The epoch this start of the relay answers to.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn tx(&mut self) -> Result<Tx<'_>, StoreError> {
        Ok(Tx(self.conn.transaction()?))
    }

    /// Run read-only queries (no transaction needed beyond SQLite's implicit one).
    pub fn read(&mut self) -> Result<Tx<'_>, StoreError> {
        self.tx()
    }
}

/// One transaction. Dropped without [`Tx::commit`], it rolls back.
pub struct Tx<'a>(Transaction<'a>);

impl Tx<'_> {
    pub fn commit(self) -> Result<(), StoreError> {
        self.0.commit()
    }

    pub fn channel(&self, ch: &Channel) -> Result<Option<ChannelRow>, StoreError> {
        self.0
            .query_row(
                "SELECT app, household, next_ord, bytes, last_write, generation FROM channels WHERE id = ?1",
                [ch.as_slice()],
                |r| {
                    Ok(ChannelRow {
                        app: r.get(0)?,
                        household: b32(r.get(1)?),
                        next_ord: u(r.get(2)?),
                        bytes: u(r.get(3)?),
                        last_write: u(r.get(4)?),
                        generation: u(r.get(5)?),
                    })
                },
            )
            .optional()
    }

    pub fn channel_count(&self) -> Result<u64, StoreError> {
        self.0.query_row("SELECT COUNT(*) FROM channels", [], |r| r.get::<_, i64>(0)).map(u)
    }

    pub fn total_bytes(&self) -> Result<u64, StoreError> {
        self.0.query_row("SELECT COALESCE(SUM(bytes), 0) FROM channels", [], |r| r.get::<_, i64>(0)).map(u)
    }

    /// The next generation from the store's counter (never handed out twice).
    fn next_generation(&self) -> Result<u64, StoreError> {
        self.0.execute(
            "INSERT INTO meta (key, value) VALUES ('generation', 1)
             ON CONFLICT (key) DO UPDATE SET value = value + 1",
            [],
        )?;
        self.0.query_row("SELECT value FROM meta WHERE key = 'generation'", [], |r| r.get::<_, i64>(0)).map(u)
    }

    pub fn create_channel(&self, ch: &Channel, app: &str, household: &[u8; 32], now: u64) -> Result<(), StoreError> {
        let generation = self.next_generation()?;
        self.0.execute(
            "INSERT INTO channels (id, app, household, next_ord, bytes, last_write, generation)
             VALUES (?1, ?2, ?3, 1, 0, ?4, ?5)",
            params![ch.as_slice(), app, household.as_slice(), i(now), i(generation)],
        )?;
        Ok(())
    }

    /// Record a write to the channel at `now` (its `last_write`, for idle expiry).
    pub fn touch(&self, ch: &Channel, now: u64) -> Result<(), StoreError> {
        self.0.execute("UPDATE channels SET last_write = ?2 WHERE id = ?1", params![ch.as_slice(), i(now)])?;
        Ok(())
    }

    /// Expire every channel with no write for `idle` ms: drop its entries, snapshots,
    /// the records of devices not forgotten and their logs, and the channel itself if no
    /// record is left. Forgotten records stay, so a forgotten device cannot enrol again,
    /// and so do their logs' `last`, so it cannot store entries at used seqs again.
    /// Returns the number of channels expired.
    pub fn expire_idle(&self, now: u64, idle: u64) -> Result<u64, StoreError> {
        // last_write + idle <= now  <=>  last_write <= now - idle (for now >= idle).
        if now < idle {
            return Ok(0);
        }
        let cutoff = i(now - idle);
        let mut st = self.0.prepare("SELECT id FROM channels WHERE last_write <= ?1 ORDER BY id")?;
        let idle: Vec<Channel> = st.query_map([cutoff], |r| Ok(b32(r.get(0)?)))?.collect::<Result<_, _>>()?;
        for ch in &idle {
            let c = ch.as_slice();
            for table in ["entries", "snapshots", "covers"] {
                self.0.execute(&format!("DELETE FROM {table} WHERE channel = ?1"), [c])?;
            }
            self.0.execute("DELETE FROM devices WHERE channel = ?1 AND cut_seq IS NULL", [c])?;
            self.0.execute(
                "DELETE FROM logs WHERE channel = ?1
                 AND NOT EXISTS (SELECT 1 FROM devices d WHERE d.channel = ?1 AND d.device = logs.uploader)",
                [c],
            )?;
            self.0.execute("UPDATE channels SET bytes = 0 WHERE id = ?1", [c])?;
            let kept = self.0.execute(
                "DELETE FROM channels WHERE id = ?1 AND NOT EXISTS (SELECT 1 FROM devices WHERE channel = ?1)",
                [c],
            )? == 0;
            if kept {
                // Its logs were wiped: clients' positions in them are void.
                let g = self.next_generation()?;
                self.0.execute("UPDATE channels SET generation = ?2 WHERE id = ?1", params![c, i(g)])?;
            }
        }
        Ok(idle.len() as u64)
    }

    fn add_bytes(&self, ch: &Channel, delta: i64) -> Result<(), StoreError> {
        self.0.execute("UPDATE channels SET bytes = bytes + ?2 WHERE id = ?1", params![ch.as_slice(), delta])?;
        Ok(())
    }

    pub fn device(&self, ch: &Channel, d: &DeviceId) -> Result<Option<DeviceRow>, StoreError> {
        self.0
            .query_row(
                "SELECT label, auth, cut_seq, freeze_ord FROM devices WHERE channel = ?1 AND device = ?2",
                [ch.as_slice(), d.as_slice()],
                |r| {
                    Ok(DeviceRow {
                        label: r.get(0)?,
                        auth: r.get(1)?,
                        cut_seq: r.get::<_, Option<i64>>(2)?.map(unbits),
                        freeze_ord: r.get::<_, Option<i64>>(3)?.map(unbits),
                    })
                },
            )
            .optional()
    }

    pub fn device_count(&self, ch: &Channel) -> Result<u64, StoreError> {
        self.0
            .query_row("SELECT COUNT(*) FROM devices WHERE channel = ?1", [ch.as_slice()], |r| r.get::<_, i64>(0))
            .map(u)
    }

    pub fn put_device(&self, ch: &Channel, d: &DeviceId, row: &DeviceRow) -> Result<(), StoreError> {
        self.0.execute(
            "INSERT OR REPLACE INTO devices (channel, device, label, auth, cut_seq, freeze_ord)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![ch.as_slice(), d.as_slice(), row.label, row.auth, row.cut_seq.map(bits), row.freeze_ord.map(bits)],
        )?;
        Ok(())
    }

    pub fn last(&self, ch: &Channel, up: &DeviceId) -> Result<u64, StoreError> {
        Ok(self
            .0
            .query_row(
                "SELECT last FROM logs WHERE channel = ?1 AND uploader = ?2",
                [ch.as_slice(), up.as_slice()],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .map(u)
            .unwrap_or(0))
    }

    pub fn entry(&self, ch: &Channel, up: &DeviceId, seq: u64) -> Result<Option<Vec<u8>>, StoreError> {
        self.0
            .query_row(
                "SELECT env FROM entries WHERE channel = ?1 AND uploader = ?2 AND seq = ?3",
                params![ch.as_slice(), up.as_slice(), i(seq)],
                |r| r.get(0),
            )
            .optional()
    }

    /// Append entries `(seq, env)` (seqs above `last`, ascending) with consecutive ords.
    pub fn append(&self, ch: &Channel, up: &DeviceId, entries: &[(u64, &[u8])], now: u64) -> Result<u64, StoreError> {
        let c = self.channel(ch)?.expect("append to an existing channel");
        let mut ord = c.next_ord;
        let mut added = 0i64;
        for (seq, env) in entries {
            self.0.execute(
                "INSERT INTO entries (channel, uploader, seq, ord, stored_at, env) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![ch.as_slice(), up.as_slice(), i(*seq), i(ord), i(now), env],
            )?;
            ord += 1;
            added += env.len() as i64;
        }
        let last = entries.last().map(|e| e.0).unwrap_or(0);
        self.0.execute(
            "INSERT INTO logs (channel, uploader, last) VALUES (?1, ?2, ?3)
             ON CONFLICT (channel, uploader) DO UPDATE SET last = excluded.last",
            params![ch.as_slice(), up.as_slice(), i(last)],
        )?;
        self.0.execute("UPDATE channels SET next_ord = ?2 WHERE id = ?1", params![ch.as_slice(), i(ord)])?;
        self.add_bytes(ch, added)?;
        Ok(last)
    }

    /// Drop every entry some snapshot covers that was stored at least `retain` ms ago.
    pub fn prune(&self, ch: &Channel, now: u64, retain: u64) -> Result<u64, StoreError> {
        let cutoff = i(now.saturating_sub(retain));
        // stored_at + retain <= now  <=>  stored_at <= now - retain (for now >= retain).
        if now < retain {
            return Ok(0);
        }
        let freed: i64 = self.0.query_row(
            "SELECT COALESCE(SUM(length(env)), 0) FROM entries e WHERE e.channel = ?1 AND e.stored_at <= ?2
             AND EXISTS (SELECT 1 FROM covers c WHERE c.channel = e.channel AND c.uploader = e.uploader AND (c.seq < 0 OR c.seq >= e.seq))",
            params![ch.as_slice(), cutoff],
            |r| r.get(0),
        )?;
        let n = self.0.execute(
            "DELETE FROM entries WHERE channel = ?1 AND stored_at <= ?2
             AND EXISTS (SELECT 1 FROM covers c WHERE c.channel = entries.channel AND c.uploader = entries.uploader AND (c.seq < 0 OR c.seq >= entries.seq))",
            params![ch.as_slice(), cutoff],
        )?;
        self.add_bytes(ch, -freed)?;
        Ok(n as u64)
    }

    /// Every channel id (for the periodic sweep).
    pub fn channel_ids(&self) -> Result<Vec<Channel>, StoreError> {
        let mut st = self.0.prepare("SELECT id FROM channels ORDER BY id")?;
        let v = st.query_map([], |r| Ok(b32(r.get(0)?)))?.collect::<Result<Vec<_>, _>>()?;
        Ok(v)
    }

    pub fn snapshot(&self, ch: &Channel, d: &DeviceId) -> Result<Option<(Vec<u8>, Pairs)>, StoreError> {
        let env: Option<Vec<u8>> = self
            .0
            .query_row(
                "SELECT env FROM snapshots WHERE channel = ?1 AND device = ?2",
                [ch.as_slice(), d.as_slice()],
                |r| r.get(0),
            )
            .optional()?;
        match env {
            None => Ok(None),
            Some(env) => Ok(Some((env, self.covers(ch, d)?))),
        }
    }

    fn covers(&self, ch: &Channel, d: &DeviceId) -> Result<Vec<(DeviceId, u64)>, StoreError> {
        let mut st =
            self.0.prepare("SELECT uploader, seq FROM covers WHERE channel = ?1 AND device = ?2 ORDER BY uploader")?;
        let v = st
            .query_map([ch.as_slice(), d.as_slice()], |r| Ok((b32(r.get(0)?), unbits(r.get(1)?))))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(v)
    }

    pub fn put_snapshot(
        &self,
        ch: &Channel,
        d: &DeviceId,
        env: &[u8],
        covers: &[(DeviceId, u64)],
        now: u64,
    ) -> Result<(), StoreError> {
        let old = self.snapshot(ch, d)?.map(|(e, _)| e.len() as i64).unwrap_or(0);
        self.0.execute("DELETE FROM covers WHERE channel = ?1 AND device = ?2", [ch.as_slice(), d.as_slice()])?;
        self.0.execute(
            "INSERT OR REPLACE INTO snapshots (channel, device, stored_at, env) VALUES (?1, ?2, ?3, ?4)",
            params![ch.as_slice(), d.as_slice(), i(now), env],
        )?;
        for (up, seq) in covers {
            self.0.execute(
                "INSERT INTO covers (channel, device, uploader, seq) VALUES (?1, ?2, ?3, ?4)",
                params![ch.as_slice(), d.as_slice(), up.as_slice(), bits(*seq)],
            )?;
        }
        self.add_bytes(ch, env.len() as i64 - old)?;
        Ok(())
    }

    /// `(device, envelope, covers)` for every snapshot in the channel, sorted by device.
    pub fn snapshots(&self, ch: &Channel) -> Result<Vec<(DeviceId, Vec<u8>, Pairs)>, StoreError> {
        let mut st = self.0.prepare("SELECT device, env FROM snapshots WHERE channel = ?1 ORDER BY device")?;
        let rows = st
            .query_map([ch.as_slice()], |r| Ok((b32(r.get(0)?), r.get::<_, Vec<u8>>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter().map(|(d, e)| Ok((d, e, self.covers(ch, &d)?))).collect()
    }

    /// `(uploader, last)` for every log in the channel, sorted by uploader.
    pub fn logs(&self, ch: &Channel) -> Result<Vec<(DeviceId, u64)>, StoreError> {
        let mut st = self.0.prepare("SELECT uploader, last FROM logs WHERE channel = ?1 ORDER BY uploader")?;
        let v =
            st.query_map([ch.as_slice()], |r| Ok((b32(r.get(0)?), u(r.get(1)?))))?.collect::<Result<Vec<_>, _>>()?;
        Ok(v)
    }

    /// The lowest held seq of a log, if it holds any.
    pub fn first_held(&self, ch: &Channel, up: &DeviceId) -> Result<Option<u64>, StoreError> {
        self.0
            .query_row(
                "SELECT MIN(seq) FROM entries WHERE channel = ?1 AND uploader = ?2",
                [ch.as_slice(), up.as_slice()],
                |r| r.get::<_, Option<i64>>(0),
            )
            .map(|v| v.map(u))
    }

    /// Visit a log's entries above `after`, ascending, as `(seq, ord, env)`, until `f`
    /// returns false.
    pub fn scan(
        &self,
        ch: &Channel,
        up: &DeviceId,
        after: u64,
        mut f: impl FnMut(u64, u64, Vec<u8>) -> bool,
    ) -> Result<(), StoreError> {
        let mut st = self.0.prepare(
            "SELECT seq, ord, env FROM entries WHERE channel = ?1 AND uploader = ?2 AND seq > ?3 ORDER BY seq",
        )?;
        let mut rows = st.query(params![ch.as_slice(), up.as_slice(), i(after)])?;
        while let Some(r) = rows.next()? {
            if !f(u(r.get(0)?), u(r.get(1)?), r.get(2)?) {
                break;
            }
        }
        Ok(())
    }

    /// The canonical dump (docs/reference/relay-protocol.md, "Store digest").
    pub fn dump(&self) -> Result<Vec<u8>, StoreError> {
        let mut out = Vec::new();
        for ch in self.channel_ids()? {
            let c = self.channel(&ch)?.expect("listed");
            let mut st = self.0.prepare(
                "SELECT device, label, auth, cut_seq, freeze_ord FROM devices WHERE channel = ?1 ORDER BY device",
            )?;
            let devices = st
                .query_map([ch.as_slice()], |r| {
                    let opt_u = |v: Option<i64>| v.map(|n| CBOR::from(unbits(n))).unwrap_or_else(CBOR::null);
                    Ok(CBOR::from(vec![
                        CBOR::to_byte_string(r.get::<_, Vec<u8>>(0)?),
                        r.get::<_, Option<String>>(1)?.map(|s| CBOR::from(s.as_str())).unwrap_or_else(CBOR::null),
                        r.get::<_, Option<Vec<u8>>>(2)?.map(CBOR::to_byte_string).unwrap_or_else(CBOR::null),
                        opt_u(r.get(3)?),
                        opt_u(r.get(4)?),
                    ]))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let mut logs = Vec::new();
            for (up, last) in self.logs(&ch)? {
                let mut st = self.0.prepare(
                    "SELECT seq, ord, stored_at, env FROM entries WHERE channel = ?1 AND uploader = ?2 ORDER BY seq",
                )?;
                let entries = st
                    .query_map([ch.as_slice(), up.as_slice()], |r| {
                        Ok(CBOR::from(vec![
                            CBOR::from(u(r.get(0)?)),
                            CBOR::from(u(r.get(1)?)),
                            CBOR::from(u(r.get(2)?)),
                            CBOR::to_byte_string(sha256(&r.get::<_, Vec<u8>>(3)?)),
                        ]))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                logs.push(CBOR::from(vec![CBOR::to_byte_string(up), CBOR::from(last), CBOR::from(entries)]));
            }
            let mut st =
                self.0.prepare("SELECT device, stored_at, env FROM snapshots WHERE channel = ?1 ORDER BY device")?;
            let snaps = st
                .query_map([ch.as_slice()], |r| Ok((b32(r.get(0)?), u(r.get(1)?), r.get::<_, Vec<u8>>(2)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            let snaps = snaps
                .into_iter()
                .map(|(d, at, env)| {
                    Ok(CBOR::from(vec![
                        CBOR::to_byte_string(d),
                        CBOR::from(at),
                        CBOR::to_byte_string(sha256(&env)),
                        pairs_cbor(&self.covers(&ch, &d)?),
                    ]))
                })
                .collect::<Result<Vec<_>, StoreError>>()?;
            out.push(CBOR::from(vec![
                CBOR::to_byte_string(ch),
                CBOR::from(c.app.as_str()),
                CBOR::to_byte_string(c.household),
                CBOR::from(c.generation),
                CBOR::from(c.next_ord),
                CBOR::from(c.last_write),
                CBOR::from(devices),
                CBOR::from(logs),
                CBOR::from(snaps),
            ]));
        }
        Ok(CBOR::from(out).to_cbor_data())
    }
}
