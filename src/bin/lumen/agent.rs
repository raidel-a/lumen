//! Launching an agent browser on a virtual screen without taking focus.
//!
//! The goal is that nothing here moves the cursor or changes the front application. Chrome is
//! launched through `NSWorkspace` with `activates = false`, its own profile and CDP port, and a
//! window position on the virtual screen. Everything after that is CDP, which never synthesises
//! input.

use crate::config::Config;
use crate::screens::ScreenInfo;
use objc2_app_kit::{
    NSApplicationActivationOptions, NSRunningApplication, NSWorkspace, NSWorkspaceOpenConfiguration,
};
use objc2_foundation::{NSArray, NSString, NSURL};
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
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

pub fn profile_dir(name: &str) -> PathBuf {
    Config::support_dir()
        .join("agents")
        .join(name)
        .join("chrome-profile")
}

/// Starts Chrome with `activates = false`. Returns immediately; the CDP endpoint appears later.
pub fn launch(
    screen: &ScreenInfo,
    name: &str,
    url: Option<&str>,
    port: u16,
    browser: Option<&str>,
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
        format!("--window-position={x},{y}"),
        format!("--window-size={width},{height}"),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--disable-session-crashed-bubble".to_string(),
        "--hide-crash-restore-bubble".to_string(),
    ];
    if let Some(url) = url {
        args.push(url.to_string());
    }

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
        workspace.openApplicationAtURL_configuration_completionHandler(&app, &configuration, None);
    }

    Ok(Launch {
        port,
        profile,
        browser: bundle,
        previous,
    })
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
        if std::env::var_os("LUMEN_TRACE").is_some() {
            eprintln!(
                "restore: current pid={} bundle={:?} previous={}",
                current.processIdentifier(),
                current.bundleIdentifier().map(|value| value.to_string()),
                previous.pid
            );
        }
        if current.processIdentifier() == previous.pid {
            return true;
        }
        let bundle = current
            .bundleIdentifier()
            .map(|value| value.to_string());
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
            let activated = app.activateWithOptions(NSApplicationActivationOptions(0));
            if std::env::var_os("LUMEN_TRACE").is_some() {
                eprintln!("restore: activate -> {activated}");
            }
            return activated;
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

