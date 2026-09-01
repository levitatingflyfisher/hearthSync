//! The wire format (docs/reference/relay-protocol.md): request parsing, the signed
//! messages, response encoding, and builders for clients.
//!
//! Parsing never trusts the body's own idea of what was signed: the server rebuilds each
//! signed message from the parsed fields with the same function a client uses to build it.

use dcbor::{CBORCase, CBOR};
use hearth_sync_kernel::keys::DeviceSigner;
use hearth_sync_kernel::{sha256, DeviceId, Id, MAX_NAME_BYTES, MAX_PARENTS};

pub type Channel = [u8; 32];
pub type Nonce = [u8; 16];
/// `(device, seq)` pairs: cursors and covers.
pub type Pairs = Vec<(DeviceId, u64)>;

/// Deepest nesting a request (or an envelope) may have. Requests nest at most four deep;
/// the limit exists because the dCBOR decoder recurses once per level.
pub const MAX_DEPTH: usize = 8;

/// The channel id of `app` and `household` (the enroll public key).
pub fn channel_id(app: &str, household: &[u8; 32]) -> Channel {
    sha256(
        &CBOR::from(vec![CBOR::from("oh-relay-channel/v1"), CBOR::from(app), CBOR::to_byte_string(household)])
            .to_cbor_data(),
    )
}

// ---------------------------------------------------------------- nesting guard

/// True if `data` is one well-formed CBOR item nested at most `max` deep, without
/// recursing. The kernel holds the one implementation (it guards its own decoders with
/// it); the relay re-exports it so `wire::nesting_ok` keeps working.
pub use hearth_sync_kernel::cbor::nesting_ok;

// ---------------------------------------------------------------- parsing

/// Why a body was refused before any check ran.
#[derive(Debug, PartialEq, Eq)]
pub struct BadRequest;

type P<T> = Result<T, BadRequest>;

/// Decode one canonical dCBOR item (nesting-guarded).
pub fn decode(body: &[u8]) -> P<CBOR> {
    if !nesting_ok(body, MAX_DEPTH) {
        return Err(BadRequest);
    }
    let c = CBOR::try_from_data(body).map_err(|_| BadRequest)?;
    if c.to_cbor_data() != body {
        return Err(BadRequest);
    }
    Ok(c)
}

fn array(c: &CBOR, n: usize) -> P<&[CBOR]> {
    match c.as_case() {
        CBORCase::Array(a) if a.len() == n => Ok(a),
        _ => Err(BadRequest),
    }
}

fn list(c: &CBOR) -> P<&[CBOR]> {
    match c.as_case() {
        CBORCase::Array(a) => Ok(a),
        _ => Err(BadRequest),
    }
}

fn uint(c: &CBOR) -> P<u64> {
    match c.as_case() {
        CBORCase::Unsigned(n) => Ok(*n),
        _ => Err(BadRequest),
    }
}

fn text(c: &CBOR) -> P<String> {
    match c.as_case() {
        CBORCase::Text(s) => Ok(s.clone()),
        _ => Err(BadRequest),
    }
}

fn bytes(c: &CBOR) -> P<Vec<u8>> {
    match c.as_case() {
        CBORCase::ByteString(b) => Ok(b.data().to_vec()),
        _ => Err(BadRequest),
    }
}

fn fixed<const N: usize>(c: &CBOR) -> P<[u8; N]> {
    <[u8; N]>::try_from(bytes(c)?.as_slice()).map_err(|_| BadRequest)
}

fn ids(c: &CBOR) -> P<Vec<Id>> {
    let v = list(c)?.iter().map(fixed::<32>).collect::<P<Vec<Id>>>()?;
    if v.windows(2).any(|w| w[0] >= w[1]) {
        return Err(BadRequest);
    }
    Ok(v)
}

fn pairs(c: &CBOR) -> P<Vec<(DeviceId, u64)>> {
    let v = list(c)?
        .iter()
        .map(|p| {
            let a = array(p, 2)?;
            Ok((fixed::<32>(&a[0])?, uint(&a[1])?))
        })
        .collect::<P<Vec<_>>>()?;
    if v.windows(2).any(|w| w[0].0 >= w[1].0) {
        return Err(BadRequest);
    }
    Ok(v)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Enroll {
    pub app: String,
    pub household: [u8; 32],
    pub device: DeviceId,
    pub label: String,
    pub auth: [u8; 64],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Forget {
    pub target: DeviceId,
    pub cut: Vec<Id>,
    pub auth: [u8; 64],
    pub cut_seq: u64,
    pub poster: DeviceId,
    pub ts: u64,
    pub sig: [u8; 64],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Append {
    pub uploader: DeviceId,
    pub first_seq: u64,
    pub envelopes: Vec<Vec<u8>>,
    pub ts: u64,
    pub sig: [u8; 64],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub device: DeviceId,
    pub envelope: Vec<u8>,
    pub covers: Vec<(DeviceId, u64)>,
    pub ts: u64,
    pub sig: [u8; 64],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pull {
    pub reader: DeviceId,
    pub ts: u64,
    pub epoch: u64,
    pub nonce: Nonce,
    pub cursors: Vec<(DeviceId, u64)>,
    pub sig: [u8; 64],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchSnapshot {
    pub reader: DeviceId,
    pub ts: u64,
    pub epoch: u64,
    pub nonce: Nonce,
    pub device: DeviceId,
    pub sig: [u8; 64],
}

impl Enroll {
    pub fn parse(body: &[u8]) -> P<Self> {
        let c = decode(body)?;
        let a = array(&c, 5)?;
        let r = Enroll {
            app: text(&a[0])?,
            household: fixed(&a[1])?,
            device: fixed(&a[2])?,
            label: text(&a[3])?,
            auth: fixed(&a[4])?,
        };
        if !hearth_sync_kernel::keys::app_domain_ok(&r.app) || r.label.is_empty() || r.label.len() > MAX_NAME_BYTES {
            return Err(BadRequest);
        }
        Ok(r)
    }
    pub fn encode(&self) -> Vec<u8> {
        CBOR::from(vec![
            CBOR::from(self.app.as_str()),
            CBOR::to_byte_string(self.household),
            CBOR::to_byte_string(self.device),
            CBOR::from(self.label.as_str()),
            CBOR::to_byte_string(self.auth),
        ])
        .to_cbor_data()
    }
}

impl Forget {
    pub fn parse(body: &[u8]) -> P<Self> {
        let c = decode(body)?;
        let a = array(&c, 7)?;
        let r = Forget {
            target: fixed(&a[0])?,
            cut: ids(&a[1])?,
            auth: fixed(&a[2])?,
            cut_seq: uint(&a[3])?,
            poster: fixed(&a[4])?,
            ts: uint(&a[5])?,
            sig: fixed(&a[6])?,
        };
        if r.cut.len() > MAX_PARENTS {
            return Err(BadRequest);
        }
        Ok(r)
    }
    pub fn signable(&self, ch: &Channel) -> Vec<u8> {
        forget_signable(ch, &self.poster, self.ts, &self.target, &self.cut, &self.auth, self.cut_seq)
    }
    pub fn encode(&self) -> Vec<u8> {
        CBOR::from(vec![
            CBOR::to_byte_string(self.target),
            ids_cbor(&self.cut),
            CBOR::to_byte_string(self.auth),
            CBOR::from(self.cut_seq),
            CBOR::to_byte_string(self.poster),
            CBOR::from(self.ts),
            CBOR::to_byte_string(self.sig),
        ])
        .to_cbor_data()
    }
}

impl Append {
    pub fn parse(body: &[u8]) -> P<Self> {
        let c = decode(body)?;
        let a = array(&c, 5)?;
        let r = Append {
            uploader: fixed(&a[0])?,
            first_seq: uint(&a[1])?,
            envelopes: list(&a[2])?.iter().map(bytes).collect::<P<_>>()?,
            ts: uint(&a[3])?,
            sig: fixed(&a[4])?,
        };
        if r.first_seq == 0 || r.envelopes.is_empty() {
            return Err(BadRequest);
        }
        Ok(r)
    }
    pub fn signable(&self, ch: &Channel) -> Vec<u8> {
        append_signable(ch, &self.uploader, self.ts, self.first_seq, &self.envelopes)
    }
    pub fn encode(&self) -> Vec<u8> {
        CBOR::from(vec![
            CBOR::to_byte_string(self.uploader),
            CBOR::from(self.first_seq),
            CBOR::from(self.envelopes.iter().map(CBOR::to_byte_string).collect::<Vec<_>>()),
            CBOR::from(self.ts),
            CBOR::to_byte_string(self.sig),
        ])
        .to_cbor_data()
    }
}

impl Snapshot {
    pub fn parse(body: &[u8]) -> P<Self> {
        let c = decode(body)?;
        let a = array(&c, 5)?;
        Ok(Snapshot {
            device: fixed(&a[0])?,
            envelope: bytes(&a[1])?,
            covers: pairs(&a[2])?,
            ts: uint(&a[3])?,
            sig: fixed(&a[4])?,
        })
    }
    pub fn signable(&self, ch: &Channel) -> Vec<u8> {
        snapshot_signable(ch, &self.device, self.ts, &self.envelope, &self.covers)
    }
    pub fn encode(&self) -> Vec<u8> {
        CBOR::from(vec![
            CBOR::to_byte_string(self.device),
            CBOR::to_byte_string(&self.envelope),
            pairs_cbor(&self.covers),
            CBOR::from(self.ts),
            CBOR::to_byte_string(self.sig),
        ])
        .to_cbor_data()
    }
}

impl Pull {
    pub fn parse(body: &[u8]) -> P<Self> {
        let c = decode(body)?;
        let a = array(&c, 6)?;
        Ok(Pull {
            reader: fixed(&a[0])?,
            ts: uint(&a[1])?,
            epoch: uint(&a[2])?,
            nonce: fixed(&a[3])?,
            cursors: pairs(&a[4])?,
            sig: fixed(&a[5])?,
        })
    }
    pub fn signable(&self, ch: &Channel) -> Vec<u8> {
        pull_signable(ch, &self.reader, self.ts, self.epoch, &self.nonce, &self.cursors)
    }
    pub fn encode(&self) -> Vec<u8> {
        CBOR::from(vec![
            CBOR::to_byte_string(self.reader),
            CBOR::from(self.ts),
            CBOR::from(self.epoch),
            CBOR::to_byte_string(self.nonce),
            pairs_cbor(&self.cursors),
            CBOR::to_byte_string(self.sig),
        ])
        .to_cbor_data()
    }
}

impl FetchSnapshot {
    pub fn parse(body: &[u8]) -> P<Self> {
        let c = decode(body)?;
        let a = array(&c, 6)?;
        Ok(FetchSnapshot {
            reader: fixed(&a[0])?,
            ts: uint(&a[1])?,
            epoch: uint(&a[2])?,
            nonce: fixed(&a[3])?,
            device: fixed(&a[4])?,
            sig: fixed(&a[5])?,
        })
    }
    pub fn signable(&self, ch: &Channel) -> Vec<u8> {
        fetch_signable(ch, &self.reader, self.ts, self.epoch, &self.nonce, &self.device)
    }
    pub fn encode(&self) -> Vec<u8> {
        CBOR::from(vec![
            CBOR::to_byte_string(self.reader),
            CBOR::from(self.ts),
            CBOR::from(self.epoch),
            CBOR::to_byte_string(self.nonce),
            CBOR::to_byte_string(self.device),
            CBOR::to_byte_string(self.sig),
        ])
        .to_cbor_data()
    }
}

// ---------------------------------------------------------------- signed messages

pub(crate) fn ids_cbor(ids: &[Id]) -> CBOR {
    CBOR::from(ids.iter().map(CBOR::to_byte_string).collect::<Vec<_>>())
}

pub(crate) fn pairs_cbor(p: &[(DeviceId, u64)]) -> CBOR {
    CBOR::from(p.iter().map(|(d, n)| CBOR::from(vec![CBOR::to_byte_string(d), CBOR::from(*n)])).collect::<Vec<_>>())
}

fn signed(tag: &str, ch: &Channel, signer: &DeviceId, ts: u64, rest: Vec<CBOR>) -> Vec<u8> {
    let mut v = vec![CBOR::from(tag), CBOR::to_byte_string(ch), CBOR::to_byte_string(signer), CBOR::from(ts)];
    v.extend(rest);
    CBOR::from(v).to_cbor_data()
}

pub fn forget_signable(
    ch: &Channel,
    poster: &DeviceId,
    ts: u64,
    target: &DeviceId,
    cut: &[Id],
    auth: &[u8; 64],
    cut_seq: u64,
) -> Vec<u8> {
    signed(
        "oh-relay-forget/v1",
        ch,
        poster,
        ts,
        vec![CBOR::to_byte_string(target), ids_cbor(cut), CBOR::to_byte_string(auth), CBOR::from(cut_seq)],
    )
}

pub fn append_signable(ch: &Channel, uploader: &DeviceId, ts: u64, first_seq: u64, envelopes: &[Vec<u8>]) -> Vec<u8> {
    let hashes = CBOR::from(envelopes.iter().map(|e| CBOR::to_byte_string(sha256(e))).collect::<Vec<_>>());
    signed("oh-relay-append/v1", ch, uploader, ts, vec![CBOR::from(first_seq), hashes])
}

pub fn snapshot_signable(
    ch: &Channel,
    device: &DeviceId,
    ts: u64,
    envelope: &[u8],
    covers: &[(DeviceId, u64)],
) -> Vec<u8> {
    signed("oh-relay-snapshot/v1", ch, device, ts, vec![CBOR::to_byte_string(sha256(envelope)), pairs_cbor(covers)])
}

pub fn pull_signable(
    ch: &Channel,
    reader: &DeviceId,
    ts: u64,
    epoch: u64,
    nonce: &Nonce,
    cursors: &[(DeviceId, u64)],
) -> Vec<u8> {
    signed(
        "oh-relay-pull/v1",
        ch,
        reader,
        ts,
        vec![CBOR::from(epoch), CBOR::to_byte_string(nonce), pairs_cbor(cursors)],
    )
}

pub fn fetch_signable(
    ch: &Channel,
    reader: &DeviceId,
    ts: u64,
    epoch: u64,
    nonce: &Nonce,
    device: &DeviceId,
) -> Vec<u8> {
    signed(
        "oh-relay-fetch-snapshot/v1",
        ch,
        reader,
        ts,
        vec![CBOR::from(epoch), CBOR::to_byte_string(nonce), CBOR::to_byte_string(device)],
    )
}

// ---------------------------------------------------------------- client builders

/// Request bodies as a device builds them. `signer` is the device key (the platform key
/// store behind [`DeviceSigner`]); lists are sorted here.
pub mod client {
    use super::*;

    pub fn enroll(app: &str, household: [u8; 32], device: DeviceId, label: &str, auth: [u8; 64]) -> Vec<u8> {
        Enroll { app: app.into(), household, device, label: label.into(), auth }.encode()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn forget(
        ch: &Channel,
        poster: &dyn DeviceSigner,
        target: DeviceId,
        cut: &[Id],
        auth: [u8; 64],
        cut_seq: u64,
        ts: u64,
    ) -> Vec<u8> {
        let mut cut = cut.to_vec();
        cut.sort();
        cut.dedup();
        let p = poster.device();
        let sig = poster.sign(&forget_signable(ch, &p, ts, &target, &cut, &auth, cut_seq));
        Forget { target, cut, auth, cut_seq, poster: p, ts, sig }.encode()
    }

    pub fn append(
        ch: &Channel,
        uploader: &dyn DeviceSigner,
        first_seq: u64,
        envelopes: Vec<Vec<u8>>,
        ts: u64,
    ) -> Vec<u8> {
        let u = uploader.device();
        let sig = uploader.sign(&append_signable(ch, &u, ts, first_seq, &envelopes));
        Append { uploader: u, first_seq, envelopes, ts, sig }.encode()
    }

    pub fn snapshot(
        ch: &Channel,
        device: &dyn DeviceSigner,
        envelope: Vec<u8>,
        mut covers: Vec<(DeviceId, u64)>,
        ts: u64,
    ) -> Vec<u8> {
        covers.sort();
        let d = device.device();
        let sig = device.sign(&snapshot_signable(ch, &d, ts, &envelope, &covers));
        Snapshot { device: d, envelope, covers, ts, sig }.encode()
    }

    /// A pull. `epoch` is the relay's (0 if not known yet: the answer is then
    /// `["err", "epoch", current]`, see [`error_epoch`]).
    pub fn pull(
        ch: &Channel,
        reader: &dyn DeviceSigner,
        epoch: u64,
        nonce: Nonce,
        mut cursors: Vec<(DeviceId, u64)>,
        ts: u64,
    ) -> Vec<u8> {
        cursors.sort();
        let r = reader.device();
        let sig = reader.sign(&pull_signable(ch, &r, ts, epoch, &nonce, &cursors));
        Pull { reader: r, ts, epoch, nonce, cursors, sig }.encode()
    }

    pub fn fetch_snapshot(
        ch: &Channel,
        reader: &dyn DeviceSigner,
        epoch: u64,
        nonce: Nonce,
        device: DeviceId,
        ts: u64,
    ) -> Vec<u8> {
        let r = reader.device();
        let sig = reader.sign(&fetch_signable(ch, &r, ts, epoch, &nonce, &device));
        FetchSnapshot { reader: r, ts, epoch, nonce, device, sig }.encode()
    }

    /// One uploader's page of a pull answer.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct LogPage {
        pub uploader: DeviceId,
        /// The lowest seq the relay still holds.
        pub first: u64,
        pub entries: Vec<(u64, Vec<u8>)>,
    }

    /// A pull answer.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct PullAnswer {
        /// The channel's generation (it changes when an expired channel is made again).
        pub generation: u64,
        pub logs: Vec<LogPage>,
        /// `(device, checkpoint ref, covers)` for each stored snapshot.
        pub snapshots: Vec<(DeviceId, Id, Pairs)>,
        pub more: bool,
    }

    /// Decode a relay answer. The relay is untrusted, so the nesting guard runs first,
    /// as it does on the relay for requests (answers nest at most six deep).
    fn answer(body: &[u8]) -> Option<CBOR> {
        nesting_ok(body, MAX_DEPTH).then(|| CBOR::try_from_data(body).ok()).flatten()
    }

    /// Parse a successful pull answer.
    pub fn parse_pull(body: &[u8]) -> Option<PullAnswer> {
        let c = answer(body)?;
        let a = array(&c, 5).ok()?;
        if text(&a[0]).ok()? != "ok" {
            return None;
        }
        let generation = uint(&a[1]).ok()?;
        let a = &a[1..];
        let logs = list(&a[1])
            .ok()?
            .iter()
            .map(|l| {
                let l = array(l, 3).ok()?;
                let entries = list(&l[2])
                    .ok()?
                    .iter()
                    .map(|e| {
                        let e = array(e, 2).ok()?;
                        Some((uint(&e[0]).ok()?, bytes(&e[1]).ok()?))
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(LogPage { uploader: fixed(&l[0]).ok()?, first: uint(&l[1]).ok()?, entries })
            })
            .collect::<Option<Vec<_>>>()?;
        let snapshots = list(&a[2])
            .ok()?
            .iter()
            .map(|s| {
                let s = array(s, 3).ok()?;
                Some((fixed(&s[0]).ok()?, fixed(&s[1]).ok()?, pairs(&s[2]).ok()?))
            })
            .collect::<Option<Vec<_>>>()?;
        let more = match a[3].as_case() {
            CBORCase::Simple(dcbor::Simple::True) => true,
            CBORCase::Simple(dcbor::Simple::False) => false,
            _ => return None,
        };
        Some(PullAnswer { generation, logs, snapshots, more })
    }

    /// Parse a successful enroll answer: the channel's generation.
    pub fn parse_enroll(body: &[u8]) -> Option<u64> {
        let c = answer(body)?;
        let a = array(&c, 2).ok()?;
        (text(&a[0]).ok()? == "ok").then(|| uint(&a[1]).ok()).flatten()
    }

    /// Parse a successful fetch_snapshot answer: the envelope and its covers.
    pub fn parse_fetch(body: &[u8]) -> Option<(Vec<u8>, Pairs)> {
        let c = answer(body)?;
        let a = array(&c, 3).ok()?;
        (text(&a[0]).ok()? == "ok").then_some(())?;
        Some((bytes(&a[1]).ok()?, pairs(&a[2]).ok()?))
    }

    /// The error code of an error answer.
    pub fn error_code(body: &[u8]) -> Option<String> {
        let c = answer(body)?;
        let a = list(&c).ok()?;
        (a.len() >= 2 && text(&a[0]).ok()? == "err").then(|| text(&a[1]).ok()).flatten()
    }

    /// The relay's epoch, from an `["err", "epoch", current]` answer.
    pub fn error_epoch(body: &[u8]) -> Option<u64> {
        let c = answer(body)?;
        let a = array(&c, 3).ok()?;
        (text(&a[0]).ok()? == "err" && text(&a[1]).ok()? == "epoch").then(|| uint(&a[2]).ok()).flatten()
    }
}
