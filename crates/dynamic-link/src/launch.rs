//! Starting the source app on demand.
//!
//! The way Dynamic Link brings After Effects up when a linked composition has to be drawn: a link
//! whose source app is not running starts it in the background — with the project the link names,
//! so the composition resolves — and talks to it. A FilmCraft project full of links then opens,
//! previews and renders without anyone starting EffectCraft first.
//!
//! The app starts as a render service (`--headless`): no window, no menus, just the project and
//! the control channel. Nothing of it is seen, and the copy the user edits is opened only when
//! asked for ([`open_window`]), from the link's menu — as in Adobe's Edit Original.

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

/// A service this process started. Its stdin stays open in `_stdin` for as long as this process
/// lives: closing the pipe is how the service learns its host is gone (`--watch-stdin`), so the
/// handle must not be dropped early, and the child must be kept to be killed when its window is
/// asked for.
struct Service {
    port: u16,
    child: std::process::Child,
    _stdin: Option<std::process::ChildStdin>,
}

fn exes() -> &'static Mutex<HashMap<LinkApp, PathBuf>> {
    static EXES: OnceLock<Mutex<HashMap<LinkApp, PathBuf>>> = OnceLock::new();
    EXES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn attempts() -> &'static Mutex<HashMap<u16, Attempt>> {
    static ATTEMPTS: OnceLock<Mutex<HashMap<u16, Attempt>>> = OnceLock::new();
    ATTEMPTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn services() -> &'static Mutex<Vec<Service>> {
    static SERVICES: OnceLock<Mutex<Vec<Service>>> = OnceLock::new();
    SERVICES.get_or_init(|| Mutex::new(Vec::new()))
}

/// Where a source app's executable is. The preferences name it (Dynamic Link settings); an empty
/// path forgets it, and links then only work against an app that is already running.
pub fn set_exe(app: LinkApp, exe: Option<PathBuf>) {
    let mut held = exes().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    match exe {
        Some(path) if !path.as_os_str().is_empty() => {
            held.insert(app, path);
        }
        _ => {
            held.remove(&app);
        }
    }
}

pub fn exe(app: LinkApp) -> Option<PathBuf> {
    exes().lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&app).cloned()
}

/// Connect to the source app at `target.port`, starting it in the background when it is not
/// running. Errors when the app cannot be started or does not answer in time; a caller that gets
/// an error draws its offline slate, and the next attempt is a moment away rather than immediate.
pub fn connect(target: &LinkTarget) -> crate::Result<SourceClient> {
    if let Ok(client) = SourceClient::connect(target) {
        return Ok(client);
    }
    if !start(target) {
        return SourceClient::connect(target);
    }
    let deadline = Instant::now() + START_TIMEOUT;
    let mut last = None;
    while Instant::now() < deadline {
        match SourceClient::connect(target) {
            Ok(client) => return Ok(client),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(POLL);
            }
        }
    }
    attempts().lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(target.port, Attempt { at: Instant::now(), starting: false });
    if let Some(e) = last {
        log::warn!("dynamic link: {} did not answer on port {} in time ({e})", target.app.id(), target.port);
    }
    SourceClient::connect(target)
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
    // The window the user edits is opened by [`open_window`]; what a frame needs is the service.
    match spawn_service(&exe, target) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("dynamic link: cannot start {} ({}): {e}", target.app.id(), exe.display());
            let mut held = attempts().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            held.insert(target.port, Attempt { at: Instant::now(), starting: false });
            false
        }
    }
}

fn spawn_service(exe: &Path, target: &LinkTarget) -> std::io::Result<()> {
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--headless").arg("--control").arg(target.port.to_string()).arg("--watch-stdin");
    // The link names its project: opening it is what makes the composition resolve. An unsaved
    // link (the replace flow saves the project itself) has none, and the app opens as it was.
    if !target.project.is_empty() && Path::new(&target.project).is_file() {
        cmd.arg(&target.project);
    }
    cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let mut child = cmd.spawn()?;
    let stdin = child.stdin.take();
    let mut held = services().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    // Services that ended on their own (an idle timeout, a crash) are only kept for their record.
    held.retain_mut(|s| s.child.try_wait().map(|s| s.is_none()).unwrap_or(false));
    held.push(Service { port: target.port, child, _stdin: stdin });
    Ok(())
}

/// Open the source app's own window on the link's project, for editing the composition by hand.
///
/// A service started from here makes way first: one app per project, and the window must be the
/// copy the user edits rather than a second one behind it. A source that already answers (a window
/// the user opened) is asked to focus; its project is left alone unless it is the link's own.
pub fn open_window(target: &LinkTarget) -> crate::Result<()> {
    let Some(exe) = exe(target.app) else {
        return Err(crate::Error::Other(format!("no {} executable is set in Preferences ▸ Dynamic Link", target.app.label())));
    };
    stop_service(target.port);
    if SourceClient::connect(target).is_ok() {
        // Something is up: the user's own window. Bring it forward, and open the project only when
        // the link's project is the one it already has — opening another would drop unsaved work.
        let mut client = SourceClient::connect(target)?;
        let summary = client.execute("engine.execute", serde_json::json!({"command": "project.summary", "params": {}}))?;
        let open = summary.get("result").and_then(|r| r.get("path")).and_then(serde_json::Value::as_str).unwrap_or("").to_owned();
        if target.project.is_empty() || same_path(&open, &target.project) {
            let _ = client.execute("ui.focus", serde_json::json!({}));
            return Ok(());
        }
        return Err(crate::Error::Other(format!(
            "{} is open with another project ({}); open {} there by hand",
            target.app.label(),
            if open.is_empty() { "untitled" } else { &open },
            target.project
        )));
    }
    // The port is freed only once the service has really gone: the window's control server binds
    // it, and a leftover socket would leave the window without one.
    let free_by = Instant::now() + Duration::from_secs(5);
    while Instant::now() < free_by && SourceClient::connect(target).is_ok() {
        std::thread::sleep(POLL);
    }
    let mut cmd = std::process::Command::new(&exe);
    // With the control channel, so the link stays live while the window is open: the frames the
    // preview asks for come from what is being edited right there. `--focus`: the user asked for
    // this window, so it comes to the front.
    cmd.arg("--control").arg(target.port.to_string()).arg("--focus");
    if !target.project.is_empty() {
        cmd.arg(&target.project);
    }
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    // The port belongs to the window until it answers: a frame asked for while it opens must wait
    // for it, not start a service that would take the port from under it.
    attempts().lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(target.port, Attempt { at: Instant::now(), starting: true });
    cmd.spawn().map_err(|e| crate::Error::Other(format!("cannot start {} ({}): {e}", target.app.label(), exe.display())))?;
    Ok(())
}

/// Kill the service this process started on `port`, if it is still up.
fn stop_service(port: u16) {
    let mut held = services().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    held.retain_mut(|s| {
        if s.port != port {
            return true;
        }
        if s.child.try_wait().map(|s| s.is_some()).unwrap_or(false) {
            return false;
        }
        let _ = s.child.kill();
        let _ = s.child.wait();
        false
    });
}

fn same_path(a: &str, b: &str) -> bool {
    let norm = |p: &str| p.replace('\\', "/").trim_end_matches('/').to_lowercase();
    !a.is_empty() && norm(a) == norm(b)
}