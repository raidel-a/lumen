//! Launching and tracking an agent browser on a virtual screen.
//!
//! Nothing here moves the cursor or keeps focus: the browser is launched with its own profile and
//! CDP port, positioned on a virtual screen, and driven through CDP afterwards. Chromium activates
//! itself when its window appears, so the app that was in front is put back.
//!
//! Sessions are tracked in memory and mirrored to `config.json`, so a crash cannot leave a browser
//! running on a real display after its screen is gone.

use crate::config::{AgentSessionRecord, Config};
use crate::screens::ScreenInfo;
use block2::RcBlock;
use objc2_app_kit::{
    NSApplicationActivationOptions, NSRunningApplication, NSWorkspace, NSWorkspaceOpenConfiguration,
};
use objc2_foundation::{NSArray, NSBundle, NSError, NSString, NSURL};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Chromium-family browsers, in the order they are tried when none is named.
pub const BROWSERS: &[&str] = &[
    "com.google.Chrome",
    "com.google.Chrome.beta",
    "org.chromium.Chromium",
    "com.brave.Browser",
    "com.microsoft.edgemac",
];

pub const DEFAULT_PORT: u16 = 9222;

/// The app that was in front before the browser was launched, so it can be put back.
#[derive(Debug, Clone)]
pub struct PreviousFront {
    pub pid: i32,
}

#[derive(Debug, Clone)]
pub struct Launch {
    pub port: u16,
    pub profile: PathBuf,
    pub browser: String,
    pub previous: Option<PreviousFront>,
    /// Filled by the Launch Services completion handler; read by `wait_for_pid`.
    pub pid: Arc<AtomicI32>,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub name: String,
    pub pid: i32,
    pub port: u16,
    pub browser: String,
    pub profile: PathBuf,
    pub url: Option<String>,
    pub started: u64,
    pub headless: bool,
}

thread_local! {
    static SESSIONS: RefCell<HashMap<String, Session>> = RefCell::new(HashMap::new());
}

/// A real browser first, then Playwright's bundled Chrome for Testing, which ships with the
/// Playwright install this machine already uses.
fn find_browser() -> Option<String> {
    unsafe {
        let workspace = NSWorkspace::sharedWorkspace();
        for bundle in BROWSERS {
            let identifier = NSString::from_str(bundle);
            if workspace
                .URLForApplicationWithBundleIdentifier(&identifier)
                .is_some()
            {
                return Some((*bundle).to_string());
            }
        }
    }
    playwright_chromium()
}

/// Looks for the newest `chromium-*` in the Playwright browser cache.
fn playwright_chromium() -> Option<String> {
    let home = std::env::var_os("HOME")?;
    let cache = PathBuf::from(home).join("Library/Caches/ms-playwright");
    let mut best: Option<(u64, PathBuf)> = None;
    for entry in std::fs::read_dir(cache).ok()? {
        let path = entry.ok()?.path();
        let name = path.file_name()?.to_string_lossy().to_string();
        let Some(version) = name.strip_prefix("chromium-") else {
            continue;
        };
        let Ok(version) = version.parse::<u64>() else {
            continue;
        };
        for arch in ["chrome-mac-arm64", "chrome-mac-x64", "chrome-mac"] {
            let app = path.join(arch).join("Google Chrome for Testing.app");
            if app.exists() && best.as_ref().map(|(seen, _)| version > *seen).unwrap_or(true) {
                best = Some((version, app));
            }
        }
    }
    best.map(|(_, path)| path.display().to_string())
}

/// Resolves a bundle id or an app path to a URL Launch Services can open.
fn app_url(identity: &str) -> Option<objc2::rc::Retained<NSURL>> {
    unsafe {
        if identity.contains('/') || identity.ends_with(".app") {
            let path = NSString::from_str(identity);
            return Some(NSURL::fileURLWithPath_isDirectory_relativeToURL(
                &path,
                true,
                None,
            ));
        }
        let identifier = NSString::from_str(identity);
        NSWorkspace::sharedWorkspace().URLForApplicationWithBundleIdentifier(&identifier)
    }
}

/// The bundle id behind an identity that may be a bundle id or an app path.
fn bundle_id_for(identity: &str) -> Option<String> {
    if identity.contains('/') || identity.ends_with(".app") {
        unsafe {
            let path = NSString::from_str(identity);
            let bundle = NSBundle::bundleWithPath(&path)?;
            return bundle.bundleIdentifier().map(|value| value.to_string());
        }
    }
    Some(identity.to_string())
}

/// Every process carrying this screen's browser profile. Chrome keeps helpers in the same
/// profile, so this is how an orphan is found after a crash or a forwarded launch.
pub fn profile_pids(name: &str) -> Vec<i32> {
    let marker = format!("--user-data-dir={}", profile_dir(name).display());
    let Ok(output) = Command::new("/bin/ps").args(["-axo", "pid=,command="]).output() else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let (pid, command) = line.split_once(' ')?;
            if command.contains(&marker) {
                pid.parse().ok()
            } else {
                None
            }
        })
        .collect()
}

/// Stops every process carrying this screen's profile, after checking the command line again.
/// Only SIGTERM is used; nothing is force-killed.
pub fn stop_profile(name: &str) -> usize {
    let marker = format!("--user-data-dir={}", profile_dir(name).display());
    let mut stopped = 0;
    for pid in profile_pids(name) {
        let Ok(output) = Command::new("/bin/ps")
            .args(["-p", &pid.to_string(), "-o", "command="])
            .output()
        else {
            continue;
        };
        if !String::from_utf8_lossy(&output.stdout).contains(&marker) {
            continue;
        }
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
        stopped += 1;
    }
    stopped
}

pub fn pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()
        .map(|output| !output.stdout.is_empty())
        .unwrap_or(false)
}

pub fn profile_dir(name: &str) -> PathBuf {
    Config::support_dir()
        .join("agents")
        .join(name)
        .join("chrome-profile")
}

/// Starts a browser with `activates = false`. Returns immediately; the CDP endpoint appears later.
pub fn launch(
    screen: &ScreenInfo,
    name: &str,
    url: Option<&str>,
    port: u16,
    browser: Option<&str>,
    headless: bool,
) -> Result<Launch, String> {
    let bundle = match browser {
        Some(bundle) => bundle.to_string(),
        None => find_browser().ok_or_else(|| {
            format!(
                "no Chromium-based browser found (tried {})",
                BROWSERS.join(", ")
            )
        })?,
    };
    let profile = profile_dir(name);
    std::fs::create_dir_all(&profile).map_err(|error| format!("profile dir: {error}"))?;

    // Leave room for the menu bar and the window chrome.
    let width = screen.width.saturating_sub(40).max(640);
    let height = screen.height.saturating_sub(90).max(480);
    let x = screen.x + 20;
    let y = screen.y + 50;

    let previous = unsafe {
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .map(|app| PreviousFront {
                pid: app.processIdentifier(),
            })
    };

    let mut args: Vec<String> = vec![
        format!("--user-data-dir={}", profile.display()),
        format!("--remote-debugging-port={port}"),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--disable-session-crashed-bubble".to_string(),
        "--hide-crash-restore-bubble".to_string(),
    ];
    if headless {
        // New headless still serves CDP and never creates a window, so it cannot steal focus.
        args.push("--headless=new".to_string());
        args.push(format!("--window-size={width},{height}"));
    } else {
        args.push(format!("--window-position={x},{y}"));
        args.push(format!("--window-size={width},{height}"));
    }
    if let Some(url) = url {
        args.push(url.to_string());
    }

    let pid = Arc::new(AtomicI32::new(0));
    unsafe {
        let workspace = NSWorkspace::sharedWorkspace();
        let app = app_url(&bundle).ok_or_else(|| format!("browser {bundle} is not installed"))?;

        let configuration = NSWorkspaceOpenConfiguration::configuration();
        configuration.setActivates(false);
        configuration.setCreatesNewApplicationInstance(true);
        configuration.setAddsToRecentItems(false);
        let arguments = NSArray::from_vec(
            args.iter()
                .map(|arg| NSString::from_str(arg))
                .collect::<Vec<_>>(),
        );
        configuration.setArguments(&arguments);

        let slot = Arc::clone(&pid);
        let handler = RcBlock::new(move |application: *mut NSRunningApplication, _error: *mut NSError| {
            if let Some(application) = application.as_ref() {
                slot.store(application.processIdentifier(), Ordering::SeqCst);
            }
        });
        workspace.openApplicationAtURL_configuration_completionHandler(
            &app,
            &configuration,
            Some(&handler),
        );
    }

    Ok(Launch {
        port,
        profile,
        browser: bundle,
        previous,
        pid,
    })
}

/// Waits for the completion handler to report the browser's pid.
pub fn wait_for_pid(launch: &Launch, timeout: Duration) -> i32 {
    let started = Instant::now();
    while started.elapsed() < timeout {
        let pid = launch.pid.load(Ordering::SeqCst);
        if pid > 0 {
            return pid;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    launch.pid.load(Ordering::SeqCst)
}

/// Puts the previous app back in front after Chromium has shown its window.
///
/// Chromium activates itself when a window appears, and neither `activates = false` nor
/// `open -g` changes that. Restoring is only done when the browser is still the front app, so a
/// user who switched elsewhere in the meantime is left alone.
pub fn restore_front(previous: &PreviousFront, _browser: &str) -> bool {
    unsafe {
        let workspace = NSWorkspace::sharedWorkspace();
        let Some(current) = workspace.frontmostApplication() else {
            return false;
        };
        if current.processIdentifier() == previous.pid {
            return true;
        }
        let bundle = current.bundleIdentifier().map(|value| value.to_string());
        let ours = bundle
            .as_deref()
            .map(|id| {
                let id = id.to_lowercase();
                id.starts_with("com.google.chrome")
                    || id == "org.chromium.chromium"
                    || id == "com.brave.browser"
                    || id == "com.microsoft.edgemac"
            })
            .unwrap_or(false);
        if !ours {
            return false;
        }
        if let Some(app) =
            NSRunningApplication::runningApplicationWithProcessIdentifier(previous.pid)
        {
            // The IgnoringOtherApps flag is deprecated and has no effect on macOS 14+; a plain
            // activation is enough once the browser is frontmost.
            return app.activateWithOptions(NSApplicationActivationOptions(0));
        }
        false
    }
}

fn read_version(port: u16) -> Option<Value> {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(400)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    // The DevTools server speaks HTTP/1.1 only, and keeps the connection open, so the reply is
    // read by Content-Length rather than by waiting for EOF.
    write!(
        stream,
        "GET /json/version HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: */*\r\n\r\n"
    )
    .ok()?;
    let mut response = Vec::new();
    let mut buffer = [0u8; 4096];
    let mut body_start = None;
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                response.extend_from_slice(&buffer[..read]);
                if body_start.is_none() {
                    body_start = response
                        .windows(4)
                        .position(|window| window == b"\r\n\r\n")
                        .map(|position| position + 4);
                }
                if let Some(start) = body_start {
                    let headers = String::from_utf8_lossy(&response[..start]).to_lowercase();
                    let length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok());
                    match length {
                        Some(length) if response.len() >= start + length => break,
                        None if response[start..].contains(&b'}') => break,
                        _ => {}
                    }
                }
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                break
            }
            Err(_) => return None,
        }
    }
    let start = body_start?;
    let text = String::from_utf8_lossy(&response[start..]);
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(&text[start..=end]).ok()
}

/// Waits for the DevTools endpoint to answer.
pub fn wait_for_cdp(port: u16, timeout: Duration) -> Result<Value, String> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if let Some(version) = read_version(port) {
            return Ok(version);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(format!(
        "Chrome did not open a DevTools endpoint on 127.0.0.1:{port} within {}s",
        timeout.as_secs()
    ))
}

/// Whether a DevTools endpoint already answers, for reusing a running session.
pub fn endpoint_alive(port: u16) -> bool {
    read_version(port).is_some()
}

// ---------------------------------------------------------------- sessions

pub fn register(session: Session) {
    SESSIONS.with(|sessions| {
        sessions.borrow_mut().insert(session.name.clone(), session);
    });
}

pub fn session(name: &str) -> Option<Session> {
    SESSIONS.with(|sessions| sessions.borrow().get(name).cloned())
}

pub fn sessions() -> Vec<Session> {
    SESSIONS.with(|sessions| sessions.borrow().values().cloned().collect())
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub fn session_record(session: &Session) -> AgentSessionRecord {
    AgentSessionRecord {
        name: session.name.clone(),
        pid: session.pid,
        browser: session.browser.clone(),
        profile: session.profile.display().to_string(),
        url: session.url.clone(),
        headless: session.headless,
    }
}

/// Terminates a browser only when the pid still runs the recorded profile and browser.
fn terminate(pid: i32, browser: &str, profile: &Path) -> Result<bool, String> {
    if pid <= 0 {
        return Ok(false);
    }
    let output = Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .map_err(|error| format!("ps: {error}"))?;
    let command = String::from_utf8_lossy(&output.stdout);
    if command.trim().is_empty() {
        // The pid is gone, so there is nothing left to stop.
        return Ok(false);
    }
    if !command.contains(&profile.display().to_string()) {
        return Err("the process does not carry the recorded profile".to_string());
    }
    unsafe {
        let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
            .ok_or_else(|| "the browser is no longer running".to_string())?;
        if let (Some(expected), Some(actual)) = (
            bundle_id_for(browser),
            app.bundleIdentifier().map(|value| value.to_string()),
        ) {
            if !expected.eq_ignore_ascii_case(&actual) {
                return Err("the pid now belongs to a different app".to_string());
            }
        }
        Ok(app.terminate())
    }
}

/// Stops the session for a screen. Safe to call when there is none.
pub fn stop(name: &str) -> Result<bool, String> {
    let Some(session) = session(name) else {
        return Ok(false);
    };
    let terminated = terminate(session.pid, &session.browser, &session.profile)?;
    SESSIONS.with(|sessions| sessions.borrow_mut().remove(name));
    Ok(terminated)
}

/// Stops every tracked session, returning the names that were asked to quit.
pub fn stop_all() -> Vec<String> {
    let all = sessions();
    let mut stopped = Vec::new();
    for session in all {
        if terminate(session.pid, &session.browser, &session.profile).unwrap_or(false) {
            stopped.push(session.name.clone());
        }
        SESSIONS.with(|sessions| sessions.borrow_mut().remove(&session.name));
    }
    stopped
}

/// Terminates one persisted record, after checking the pid still carries its profile.
pub fn terminate_record(record: &AgentSessionRecord) -> Result<bool, String> {
    terminate(record.pid, &record.browser, &PathBuf::from(&record.profile))
}

/// Closes browsers left over from a previous crash, using the persisted records.
pub fn reconcile(records: &[AgentSessionRecord]) -> usize {
    let mut stopped = 0;
    for record in records {
        let profile = PathBuf::from(&record.profile);
        match terminate(record.pid, &record.browser, &profile) {
            Ok(true) => stopped += 1,
            Ok(false) => {}
            Err(error) => {
                if std::env::var_os("LUMEN_TRACE").is_some() {
                    eprintln!("agent session {} not closed: {error}", record.name);
                }
            }
        }
        // Helpers and forwarded instances carry the same profile.
        stop_profile(&record.name);
    }
    stopped
}

/// Whether the tracked browser is still running.
pub fn session_alive(session: &Session) -> bool {
    unsafe {
        NSRunningApplication::runningApplicationWithProcessIdentifier(session.pid).is_some()
    }
}

/// Builds the session returned to a client.
pub fn session_json(session: &Session, endpoint: Option<Value>) -> Value {
    serde_json::json!({
        "name": session.name,
        "pid": session.pid,
        "port": session.port,
        "browser": session.browser,
        "profile": session.profile.display().to_string(),
        "url": session.url,
        "started": session.started,
        "headless": session.headless,
        "endpoint": endpoint,
    })
}

pub fn new_session(
    name: &str,
    launch: &Launch,
    pid: i32,
    url: Option<String>,
    headless: bool,
) -> Session {
    Session {
        name: name.to_string(),
        pid,
        port: launch.port,
        browser: launch.browser.clone(),
        profile: launch.profile.clone(),
        url,
        started: now_seconds(),
        headless,
    }
}
