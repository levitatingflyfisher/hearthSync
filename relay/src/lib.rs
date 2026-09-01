//! # hearth_sync_relay
//!
//! The hearthSync relay: data-blind store-and-forward of sealed ops per household
//! channel (docs/reference/relay-protocol.md, ADR 0013). It cannot open what it stores.
//! It checks household signatures on enrolments and Forgets, and device signatures on
//! everything else, with the kernel's own functions.
//!
//! - [`wire`]: request parsing, the signed messages, client builders.
//! - [`relay`]: the handler, pure apart from its store (time comes in as `now`).
//! - [`store`]: SQLite persistence and the canonical dump the conformance suite digests.
//! - [`http`]: the hyper server around the handler.
//! - [`config`], [`log`]: limits and structured logs.

#![forbid(unsafe_code)]

pub mod config;
pub mod http;
pub mod log;
pub mod relay;
pub mod store;
pub mod wire;

pub use config::Config;
pub use relay::{Relay, Response};
