//! Keeping a Dynamic Link preview live.
//!
//! A linked composition is not a copy: it is rendered by the app that owns it, and it changes when
//! that app is edited. The monitor caches rendered frames under a key that includes the project
//! revision, so an edit on the other side would go unnoticed — nothing here was edited. The watcher
//! below polls each linked source app's revision and moves the link epoch when it changes
//! ([`filmcraft_engine::media_pool::link_epoch`]); the epoch is part of every frame key, so frames
//! drawn from a link go stale at once and the monitor renders the composition as it is now.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, PoisonError};

use filmcraft_engine::media_pool;
use filmcraft_project::{ItemKind, MediaRef};

use crate::FilmcraftApp;

/// The ports of the source apps this project links to, once each.
pub(crate) fn source_ports(app: &FilmcraftApp) -> Vec<u16> {
    let mut ports = Vec::new();
    for it in app.session.project.items.values() {
        if let ItemKind::Media(m) = &it.kind
            && let MediaRef::DynamicLink(t) = &m.media
            && !ports.contains(&t.port)
        {
            ports.push(t.port);
        }
    }
    ports
}

impl FilmcraftApp {
    /// Start the watcher that keeps linked previews live, once. [`Self::refresh_link_ports`] keeps
    /// the set of watched apps current as links come and go.
    pub(crate) fn start_link_watch(&mut self, ctx: &egui::Context) {
        if self.link_watch_started {
            return;
        }
        self.link_watch_started = true;
        let ports = self.link_ports.clone();
        // Never cleared: the thread sleeps out the life of the app and exits with the process.
        let run = Arc::new(AtomicBool::new(true));
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            media_pool::watch_links(ports, run, std::thread::sleep, move || repaint.request_repaint());
        });
    }

    /// Hand the watcher the current set of linked source apps.
    pub(crate) fn refresh_link_ports(&mut self) {
        let ports = source_ports(self);
        let mut held = self.link_ports.lock().unwrap_or_else(PoisonError::into_inner);
        if *held != ports {
            *held = ports;
        }
    }

    /// A project revision for frame keys, which also moves when a linked source project changes:
    /// the epoch sits in the high bits, so the lookback arithmetic of playback frames (which
    /// subtracts small numbers from the revision) never borrows into it.
    pub(crate) fn frame_revision(&self) -> u64 {
        self.session.revision ^ (media_pool::link_epoch() << 40)
    }
}
