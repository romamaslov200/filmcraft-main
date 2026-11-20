//! Starting the source app on demand.
//!
//! The way Dynamic Link brings After Effects up when a linked composition has to be drawn: a link
//! whose source app is not running starts it in the background — with the project the link names,
//! so the composition resolves — and talks to it. A FilmCraft project full of links then opens,
//! previews and renders without anyone starting EffectCraft first.

use crate::client::SourceClient;
use crate::model::{LinkApp, LinkTarget};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long a request waits for a source that was just started: EffectCraft takes a few seconds to
/// open a project and listen.
const START_TIMEOUT: Duration = Duration::from_secs(15);
/// A source that did not come up is not started again for this long; requests meanwhile fall back
/// to the offline slate instead of starting an app per frame.
const RETRY_AFTER: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(250);

/// One start per port, so a second request does not open a second app while the first is opening.
struct Attempt {
    at: Instant,
    starting: bool,
}

fn exes() -> &'static Mutex<HashMap<LinkApp, PathBuf>> {
    static EXES: OnceLock<Mutex<HashMap<LinkApp, PathBuf>>> = OnceLock::new();
    EXES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn attempts() -> &'static Mutex<HashMap<u16, Attempt>> {
    static ATTEMPTS: OnceLock<Mutex<HashMap<u16, Attempt>>> = OnceLock::new();
    ATTEMPTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Where a source app's executable is. The preferences name it (Dynamic Link settings); an empty
/// path forgets it, and links then only work against an app that is already running.
pub fn set_exe(app: LinkApp, exe: Option<PathBuf>) {
    let mut held = exes().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    match exe {
        Some(p) if !p.as_os_str().is_empty() => {
            held.insert(app, p);
        }
        _ => {
            held.remove(&app);
        }
    }
}

/// The executable a source app starts from, when one was set.
pub fn exe(app: LinkApp) -> Option<PathBuf> {
    exes().lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&app).cloned()
}

/// Connect to the link's source app, starting it when it is not running.
///
/// A start is attempted at most once per [`RETRY_AFTER`]: the frames that ask while the app is
/// coming up wait for it, and the frames that ask after it failed take the slate rather than block.
pub fn connect(target: &LinkTarget) -> crate::Result<SourceClient> {
    match SourceClient::connect(target) {
        Ok(c) => {
            attempts().lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&target.port);
            Ok(c)
        }
        Err(e) => {
            if !start(target) {
                return Err(e);
            }
            let deadline = Instant::now() + START_TIMEOUT;
            loop {
                match SourceClient::connect(target) {
                    Ok(c) => {
                        attempts().lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&target.port);
                        return Ok(c);
                    }
                    Err(e) if Instant::now() >= deadline => {
                        attempts()
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .insert(target.port, Attempt { at: Instant::now(), starting: false });
                        return Err(e);
                    }
                    Err(_) => std::thread::sleep(POLL),
                }
            }
        }
    }
}

/// Start the app when it should be started now, and say whether the caller should wait for it: an
/// app that is already starting is waited for, one that just failed is not started again.
fn start(target: &LinkTarget) -> bool {
    let Some(exe) = exe(target.app) else { return false };
    let now = Instant::now();
    {
        let mut held = attempts().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(a) = held.get(&target.port) {
            let age = now.duration_since(a.at);
            // Still opening: wait for it, do not open a second one.
            if a.starting && age < START_TIMEOUT {
                return true;
            }
            // It failed a moment ago: do not try again yet.
            if !a.starting && age < RETRY_AFTER {
                return false;
            }
        }
        held.insert(target.port, Attempt { at: now, starting: true });
    }
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("--control").arg(target.port.to_string());
    // The link names its project: opening it is what makes the composition resolve. An unsaved
    // link (the replace flow saves the project itself) has none, and the app opens as it was.
    if !target.project.is_empty() && Path::new(&target.project).is_file() {
        cmd.arg(&target.project);
    }
    match cmd.spawn() {
        Ok(_) => true,
        Err(e) => {
            log::warn!("dynamic link: cannot start {} ({}): {e}", target.app.id(), exe.display());
            let mut held = attempts().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            held.insert(target.port, Attempt { at: Instant::now(), starting: false });
            false
        }
    }
}
