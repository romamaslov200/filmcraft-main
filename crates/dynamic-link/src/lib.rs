//! # filmcraft-dynamic-link
//!
//! Dynamic Link: a live reference to a composition (EffectCraft) or sequence (FilmCraft) living in
//! another running Craft app, resolved over that app's control channel (TCP loopback JSON-lines).
//!
//! Frames are not rendered to disk ahead of time: a linked media item asks the source app for each
//! frame on demand. When the source app is closed the link goes offline (the offline slate shows
//! instead), and a watcher polls the source app's control port so a reopened app reconnects
//! automatically.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod client;
pub mod launch;
pub mod model;

pub use model::{CraftLinkFile, LinkApp, LinkItem, LinkTarget};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("bad link address `{0}`")]
    BadAddr(String),
    #[error("control channel: {0}")]
    Bridge(String),
    #[error("the source app replied with an error: {0}")]
    App(String),
    #[error("the source app is not reachable on port {0}")]
    Unreachable(u16),
    #[error("the linked item `{0}` is not in the source project")]
    NotFound(String),
    #[error("{0}")]
    Other(String),
}
