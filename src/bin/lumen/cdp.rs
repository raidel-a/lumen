//! Minimal Chrome DevTools Protocol client over WebSocket.
//!
//! Only used to measure and move the agent browser window, so it speaks just enough of RFC 6455:
//! a text frame out, text frames in, pings answered. No TLS, localhost only.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

const MASK: [u8; 4] = [0x37, 0xfa, 0x21, 0x3d];
/// Any 16-byte base64 value is accepted; this one is the canonical RFC example.
const KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";

#[derive(Debug, Clone)]
pub struct Window {
    pub window_id: i64,
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
    pub state: String,
}

impl Window {
    pub fn json(&self) -> Value {
        json!({
            "id": self.window_id,
            "left": self.left,
            "top": self.top,
            "width": self.width,
            "height": self.height,
            "state": self.state,
        })
    }

    /// Whether the window sits fully inside the given screen rectangle.
    pub fn inside(&self, x: i32, y: i32, width: u32, height: u32) -> bool {
        let left = f64::from(x);
        let top = f64::from(y);
        let right = left + f64::from(width);
        let bottom = top + f64::from(height);
        self.left >= left
            && self.top >= top
            && self.left + self.width <= right + 1.0
            && self.top + self.height <= bottom + 1.0
    }
}

fn connect(port: u16) -> Result<TcpStream, String> {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let stream = TcpStream::connect_timeout(&address, Duration::from_millis(500))
        .map_err(|error| format!("cdp connect: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(|error| error.to_string())?;
    Ok(stream)
}

fn read_http_body(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
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
            Err(error) => return Err(error.to_string()),
        }
    }
    let start = body_start.ok_or_else(|| "cdp: no http headers".to_string())?;
    Ok(response[start..].to_vec())
}

fn http_json(port: u16, path: &str) -> Result<Value, String> {
    let mut stream = connect(port)?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: */*\r\n\r\n"
    )
    .map_err(|error| error.to_string())?;
    let body = read_http_body(&mut stream)?;
    // `/json/list` is an array, so parse the body as a whole rather than slicing braces.
    serde_json::from_slice(&body).map_err(|error| format!("cdp {path}: {error}"))
}

fn ws_path(port: u16) -> Result<String, String> {
    let version = http_json(port, "/json/version")?;
    let url = version
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .ok_or_else(|| "cdp: no websocket url".to_string())?;
    let rest = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .ok_or_else(|| "cdp: bad websocket url".to_string())?;
    let slash = rest
        .find('/')
        .ok_or_else(|| "cdp: bad websocket url".to_string())?;
    Ok(rest[slash..].to_string())
}

fn page_target(port: u16) -> Result<String, String> {
    let list = http_json(port, "/json/list")?;
    list.as_array()
        .and_then(|targets| {
            targets
                .iter()
                .find(|target| target.get("type").and_then(Value::as_str) == Some("page"))
        })
        .and_then(|target| target.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "cdp: no page target".to_string())
}

fn ws_connect(port: u16, path: &str) -> Result<TcpStream, String> {
    let mut stream = connect(port)?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {KEY}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    )
    .map_err(|error| error.to_string())?;
    let mut header = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if stream.read(&mut byte).map_err(|error| error.to_string())? == 0 {
            return Err("cdp: websocket closed during handshake".to_string());
        }
        header.push(byte[0]);
        if header.ends_with(b"\r\n\r\n") {
            break;
        }
        if header.len() > 8192 {
            return Err("cdp: websocket header too large".to_string());
        }
    }
    let text = String::from_utf8_lossy(&header);
    if !text.starts_with("HTTP/1.1 101") {
        return Err(format!(
            "cdp: websocket handshake failed: {}",
            text.lines().next().unwrap_or("")
        ));
    }
    Ok(stream)
}

fn write_frame(stream: &mut TcpStream, opcode: u8, payload: &[u8]) -> Result<(), String> {
    let mut frame = Vec::with_capacity(payload.len() + 14);
    frame.push(0x80 | opcode);
    if payload.len() < 126 {
        frame.push(0x80 | payload.len() as u8);
    } else if payload.len() <= 65535 {
        frame.push(0x80 | 126);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        frame.push(0x80 | 127);
        frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }
    frame.extend_from_slice(&MASK);
    for (index, byte) in payload.iter().enumerate() {
        frame.push(byte ^ MASK[index % 4]);
    }
    stream
        .write_all(&frame)
        .map_err(|error| format!("cdp write: {error}"))
}

fn read_frame(stream: &mut TcpStream) -> Result<(bool, u8, Vec<u8>), String> {
    let mut head = [0u8; 2];
    stream
        .read_exact(&mut head)
        .map_err(|error| format!("cdp read: {error}"))?;
    let fin = head[0] & 0x80 != 0;
    let opcode = head[0] & 0x0f;
    let masked = head[1] & 0x80 != 0;
    let mut length = usize::from(head[1] & 0x7f);
    if length == 126 {
        let mut bytes = [0u8; 2];
        stream
            .read_exact(&mut bytes)
            .map_err(|error| error.to_string())?;
        length = usize::from(u16::from_be_bytes(bytes));
    } else if length == 127 {
        let mut bytes = [0u8; 8];
        stream
            .read_exact(&mut bytes)
            .map_err(|error| error.to_string())?;
        length = u64::from_be_bytes(bytes) as usize;
    }
    let mask = if masked {
        let mut bytes = [0u8; 4];
        stream
            .read_exact(&mut bytes)
            .map_err(|error| error.to_string())?;
        Some(bytes)
    } else {
        None
    };
    let mut payload = vec![0u8; length];
    stream
        .read_exact(&mut payload)
        .map_err(|error| error.to_string())?;
    if let Some(mask) = mask {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
    }
    Ok((fin, opcode, payload))
}

fn message(stream: &mut TcpStream) -> Result<Value, String> {
    let mut payload = Vec::new();
    loop {
        let (fin, opcode, chunk) = read_frame(stream)?;
        match opcode {
            0x1 => {
                payload.extend_from_slice(&chunk);
                if fin {
                    return serde_json::from_slice(&payload)
                        .map_err(|error| format!("cdp json: {error}"));
                }
            }
            0x9 => write_frame(stream, 0xa, &chunk)?,
            0x8 => return Err("cdp: websocket closed".to_string()),
            _ => {}
        }
    }
}

fn command(stream: &mut TcpStream, id: i64, method: &str, params: Value) -> Result<Value, String> {
    let request = json!({ "id": id, "method": method, "params": params });
    write_frame(stream, 0x1, request.to_string().as_bytes())?;
    loop {
        let value = message(stream)?;
        if value.get("id").and_then(Value::as_i64) != Some(id) {
            continue;
        }
        if let Some(error) = value.get("error") {
            return Err(format!("cdp {method}: {error}"));
        }
        return Ok(value.get("result").cloned().unwrap_or(Value::Null));
    }
}

fn parse_window(result: &Value) -> Window {
    let bounds = result.get("bounds").cloned().unwrap_or(Value::Null);
    Window {
        window_id: result.get("windowId").and_then(Value::as_i64).unwrap_or(0),
        left: bounds.get("left").and_then(Value::as_f64).unwrap_or(0.0),
        top: bounds.get("top").and_then(Value::as_f64).unwrap_or(0.0),
        width: bounds.get("width").and_then(Value::as_f64).unwrap_or(0.0),
        height: bounds.get("height").and_then(Value::as_f64).unwrap_or(0.0),
        state: bounds
            .get("windowState")
            .and_then(Value::as_str)
            .unwrap_or("normal")
            .to_string(),
    }
}

/// Reads the browser window bounds.
pub fn window(port: u16) -> Result<Window, String> {
    let target = page_target(port)?;
    let path = ws_path(port)?;
    let mut stream = ws_connect(port, &path)?;
    let result = command(
        &mut stream,
        1,
        "Browser.getWindowForTarget",
        json!({ "targetId": target }),
    )?;
    Ok(parse_window(&result))
}

/// Sets explicit browser window bounds.
pub fn set_window(
    port: u16,
    left: f64,
    top: f64,
    width: f64,
    height: f64,
) -> Result<Window, String> {
    let target = page_target(port)?;
    let path = ws_path(port)?;
    let mut stream = ws_connect(port, &path)?;
    let result = command(
        &mut stream,
        1,
        "Browser.getWindowForTarget",
        json!({ "targetId": target }),
    )?;
    let window_id = result.get("windowId").and_then(Value::as_i64).unwrap_or(0);
    if window_id == 0 {
        return Err("cdp: no browser window".to_string());
    }
    command(
        &mut stream,
        2,
        "Browser.setWindowBounds",
        json!({
            "windowId": window_id,
            "bounds": {
                "left": left,
                "top": top,
                "width": width,
                "height": height,
                "windowState": "normal",
            },
        }),
    )?;
    window(port)
}

/// Moves the browser window onto a screen, unless it is already there.
pub fn place_window(
    port: u16,
    x: i32,
    y: i32,
    screen_width: u32,
    screen_height: u32,
) -> Result<Window, String> {
    let current = window(port)?;
    if current.inside(x, y, screen_width, screen_height) {
        return Ok(current);
    }
    let width = f64::from(screen_width.saturating_sub(40).max(640));
    let height = f64::from(screen_height.saturating_sub(90).max(480));
    let left = f64::from(x) + 20.0;
    let top = f64::from(y) + 50.0;

    set_window(port, left, top, width, height)
}
