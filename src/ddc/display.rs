//! DDC/CI display control over IOKit `IOAVService` (Apple silicon path).

use super::ffi;

use core_foundation::base::TCFType;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use core_foundation_sys::base::kCFAllocatorDefault;
use std::ffi::c_char;
use std::fmt;
use std::time::Duration;

pub use ffi::AvService;

/// MCCS VCP feature codes Lumen cares about.
pub const VCP_BRIGHTNESS: u8 = 0x10;
pub const VCP_CONTRAST: u8 = 0x12;
pub const VCP_VOLUME: u8 = 0x62;
pub const VCP_INPUT_SOURCE: u8 = 0x60;
pub const VCP_INPUT_SOURCE_LG_ALT: u8 = 0xF4;
pub const VCP_POWER_MODE: u8 = 0xD6;
pub const VCP_FACTORY_RESET: u8 = 0x04;

const DDC_WRITE_WAIT: Duration = Duration::from_millis(10);
const DDC_READ_WAIT: Duration = Duration::from_millis(45);
const READ_ATTEMPTS: usize = 3;

#[derive(Debug)]
pub enum DdcError {
    Io(i32),
    NoReply,
    BadReply(String),
    Unsupported(u8),
    NoService(String),
}

impl fmt::Display for DdcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DdcError::Io(code) => write!(f, "I/O error 0x{code:08x}"),
            DdcError::NoReply => f.write_str("display did not answer"),
            DdcError::BadReply(why) => write!(f, "malformed reply: {why}"),
            DdcError::Unsupported(code) => write!(f, "display does not support VCP 0x{code:02x}"),
            DdcError::NoService(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for DdcError {}

/// Which I2C data address a DDC packet is sent to.
///
/// Some LG firmware only honors input switching when the packet goes to `0x50` instead of the
/// standard `0x51`. That is the "LG alt" mode BetterDisplay and m1ddc expose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataAddress {
    Host,
    LgAlt,
}

impl DataAddress {
    pub fn as_u32(self) -> u32 {
        match self {
            DataAddress::Host => 0x51,
            DataAddress::LgAlt => 0x50,
        }
    }
}

/// Which command sequence an input switch uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputProtocol {
    /// VCP 0x60 at the standard data address. Works on most displays.
    Standard,
    /// VCP 0xF4 at data address 0x50. Several 2020s LG displays only respond to this.
    LgAlt,
}

impl InputProtocol {
    pub fn vcp_code(self) -> u8 {
        match self {
            InputProtocol::Standard => VCP_INPUT_SOURCE,
            InputProtocol::LgAlt => VCP_INPUT_SOURCE_LG_ALT,
        }
    }

    pub fn data_address(self) -> DataAddress {
        match self {
            InputProtocol::Standard => DataAddress::Host,
            InputProtocol::LgAlt => DataAddress::LgAlt,
        }
    }
}

/// A VCP feature reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VcpValue {
    pub code: u8,
    pub kind: u8,
    pub max: u16,
    pub value: u16,
}

/// One selectable monitor input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputCode {
    pub label: String,
    pub value: u16,
    pub protocol: InputProtocol,
}

/// An external display reachable over DDC/CI.
pub struct Display {
    pub id: u32,
    pub name: String,
    pub manufacturer: String,
    pub model: u32,
    pub serial: u32,
    pub location: String,
    service: AvService,
    chip: u32,
    capabilities: Option<String>,
}

impl fmt::Debug for Display {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Display")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("manufacturer", &self.manufacturer)
            .field("model", &self.model)
            .field("serial", &self.serial)
            .field("chip", &format_args!("0x{:02x}", self.chip))
            .finish()
    }
}

impl Display {
    /// All external displays that expose a DDC channel right now.
    pub fn enumerate() -> Vec<Display> {
        let mut ids = [0u32; 16];
        let mut count = 0u32;
        if unsafe { ffi::CGGetOnlineDisplayList(16, ids.as_mut_ptr(), &mut count) } != 0 {
            return Vec::new();
        }

        ids[..count as usize]
            .iter()
            .filter_map(|id| Display::open(*id))
            .collect()
    }

    /// Finds one display by its CoreGraphics id.
    pub fn find(id: u32) -> Option<Display> {
        Display::open(id)
    }

    fn open(id: u32) -> Option<Display> {
        if unsafe { ffi::CGDisplayIsBuiltin(id) } != 0 {
            return None;
        }

        let location = unsafe {
            let dict_ref = ffi::CoreDisplay_DisplayCreateInfoDictionary(id);
            if dict_ref.is_null() {
                return None;
            }
            let dict: CFDictionary<CFString, core_foundation::base::CFType> =
                CFDictionary::wrap_under_create_rule(dict_ref as _);
            let key = CFString::from_static_string("IODisplayLocation");
            dict.find(&key)
                .and_then(|v| v.downcast::<CFString>())
                .map(|s| s.to_string())
        }?;

        let adapter = ffi::entry_from_location(&location)?;

        let (service, chip) = find_av_service(&location)?;
        let chip = match std::env::var("LUMEN_CHIP") {
            Ok(value) => u32::from_str_radix(value.trim_start_matches("0x"), 16).unwrap_or(chip),
            Err(_) => chip,
        };
        let cg = core_graphics::display::CGDisplay::new(id);

        let mut display = Display {
            id,
            name: format!("Display {}", id),
            manufacturer: String::new(),
            model: cg.model_number(),
            serial: cg.serial_number(),
            location,
            service,
            chip,
            capabilities: None,
        };
        if let Some(name) = ffi::display_attribute_named(adapter, "ProductName") {
            display.name = name;
        }
        display.manufacturer =
            ffi::display_attribute_named(adapter, "ManufacturerID").unwrap_or_default();
        Some(display)
    }

    pub fn chip_address(&self) -> u32 {
        self.chip
    }

    /// The display's capabilities string, fetched once and cached.
    pub fn capabilities(&mut self) -> Result<String, DdcError> {
        if let Some(caps) = &self.capabilities {
            return Ok(caps.clone());
        }
        let mut out = String::new();
        let mut offset = 0u16;
        loop {
            let mut request = vec![0xF3u8, (offset >> 8) as u8, (offset & 0xff) as u8];
            let payload = self.exchange(&mut request, 40, DataAddress::Host)?;
            if payload.len() < 4 || payload[0] != 0xE3 {
                break;
            }
            let chunk = &payload[3..];
            if chunk.is_empty() {
                break;
            }
            out.push_str(&String::from_utf8_lossy(chunk));
            offset = offset.wrapping_add(chunk.len() as u16);
            if offset > 4096 {
                break;
            }
        }
        if out.is_empty() {
            // A failed read must not poison the cache: the DDC channel is flaky.
            return Err(DdcError::NoReply);
        }
        self.capabilities = Some(out.clone());
        Ok(out)
    }

    /// Reads a VCP feature.
    pub fn read_vcp(&mut self, code: u8) -> Result<VcpValue, DdcError> {
        self.read_vcp_with(code, DataAddress::Host)
    }

    /// Reads a VCP feature through a specific data address.
    pub fn read_vcp_with(&mut self, code: u8, address: DataAddress) -> Result<VcpValue, DdcError> {
        let mut request = vec![0x01u8, code];
        let payload = self.exchange(&mut request, 12, address)?;
        if payload.len() < 8 {
            return Err(DdcError::BadReply(format!("short VCP reply ({})", payload.len())));
        }
        let result = payload[1];
        if result != 0 {
            return Err(DdcError::Unsupported(code));
        }
        Ok(VcpValue {
            code,
            kind: payload[3],
            max: u16::from_be_bytes([payload[4], payload[5]]),
            value: u16::from_be_bytes([payload[6], payload[7]]),
        })
    }

    /// Writes a VCP feature using the standard data address.
    pub fn write_vcp(&mut self, code: u8, value: u16) -> Result<(), DdcError> {
        self.write_vcp_with(code, value, DataAddress::Host)
    }

    /// Writes a VCP feature to a specific data address.
    pub fn write_vcp_with(
        &mut self,
        code: u8,
        value: u16,
        address: DataAddress,
    ) -> Result<(), DdcError> {
        let [hi, lo] = value.to_be_bytes();
        let mut request = vec![0x03u8, code, hi, lo];
        self.exchange(&mut request, 0, address)?;
        Ok(())
    }

    /// Sends a monitor's input switch command.
    pub fn set_input(&mut self, value: u16, protocol: InputProtocol) -> Result<(), DdcError> {
        self.write_vcp_with(protocol.vcp_code(), value, protocol.data_address())
    }

    /// The inputs this display advertises, in a stable display order.
    pub fn input_candidates(&mut self) -> Vec<InputCode> {
        let mut out = Vec::new();
        if let Ok(caps) = self.capabilities() {
            if let Some(values) = parse_vcp_values(&caps, VCP_INPUT_SOURCE) {
                for value in values {
                    out.push(InputCode {
                        label: input_name(value as u16).to_string(),
                        value: value as u16,
                        protocol: InputProtocol::Standard,
                    });
                }
            }
        }

        if self.manufacturer.eq_ignore_ascii_case("GSM") {
            for (value, label) in [(0xD0u16, "DisplayPort"), (0x90, "HDMI 1"), (0x91, "HDMI 2"), (0xD2, "USB-C")] {
                if !out.iter().any(|c| c.value == value) {
                    out.push(InputCode {
                        label: label.to_string(),
                        value,
                        protocol: InputProtocol::LgAlt,
                    });
                }
            }
        }
        out
    }

    /// Sends a DDC packet and optionally reads the reply.
    fn exchange(
        &mut self,
        request: &mut Vec<u8>,
        read_len: u32,
        address: DataAddress,
    ) -> Result<Vec<u8>, DdcError> {
        let data_address = address.as_u32();
        let mut packet = Vec::with_capacity(request.len() + 2);
        packet.push(0x80 | request.len() as u8);
        packet.append(request);
        let checksum = packet
            .iter()
            .fold((self.chip << 1) as u8 ^ data_address as u8, |acc, byte| acc ^ byte);
        packet.push(checksum);

        self.write_packet(&packet, data_address)?;
        if read_len == 0 {
            return Ok(Vec::new());
        }
        std::thread::sleep(DDC_READ_WAIT);
        self.read_reply(read_len, data_address)
    }

    pub fn write_packet(&self, packet: &[u8], data_address: u32) -> Result<(), DdcError> {
        let mut last = 0;
        for attempt in 0..READ_ATTEMPTS {
            if attempt > 0 {
                std::thread::sleep(DDC_WRITE_WAIT * 2);
            }
            last = unsafe {
                ffi::IOAVServiceWriteI2C(
                    self.service.0,
                    self.chip,
                    data_address,
                    packet.as_ptr() as _,
                    packet.len() as u32,
                )
            };
            if last == 0 {
                return Ok(());
            }
        }
        std::thread::sleep(DDC_WRITE_WAIT);
        Err(DdcError::Io(last))
    }

    fn read_reply(&self, len: u32, data_address: u32) -> Result<Vec<u8>, DdcError> {
        let mut buffer = vec![0u8; len as usize];
        for attempt in 0..READ_ATTEMPTS {
            if attempt > 0 {
                std::thread::sleep(DDC_WRITE_WAIT);
                buffer.fill(0);
            }
            let status = unsafe {
                ffi::IOAVServiceReadI2C(
                    self.service.0,
                    self.chip,
                    data_address,
                    buffer.as_mut_ptr() as _,
                    len,
                )
            };
            if status != 0 {
                continue;
            }
            if buffer.len() >= 3 && buffer[1] & 0x80 != 0 {
                let payload_len = (buffer[1] & 0x7f) as usize;
                if 2 + payload_len <= buffer.len() {
                    return Ok(buffer[2..2 + payload_len].to_vec());
                }
            }
        }
        Err(DdcError::NoReply)
    }
}

/// Finds the DCPAVServiceProxy that belongs to a display and wraps it in an `IOAVService`.
///
/// The display's `IODisplayLocation` names its framebuffer inside a port node (`dispext0@70000000`
/// on this machine). The DCP AV service for that port lives under a separate `dcpext` subtree but
/// carries the same port name in its path (`dispext0:dcpav-service-epic:0`), so matching on that
/// name picks the right proxy when several displays are attached.
fn find_av_service(location: &str) -> Option<(AvService, u32)> {
    use super::ffi::*;

    let port = location
        .split('/')
        .rev()
        .nth(1)
        .and_then(|component| component.split('@').next())
        .map(str::to_owned);
    let port_marker = port.as_ref().map(|port| format!("/{port}:dcpav-service-epic"));

    unsafe {
        let root = IORegistryGetRootEntry(0);
        let mut iter = 0;
        if IORegistryEntryCreateIterator(
            root,
            SERVICE_PLANE.as_ptr() as *const c_char,
            IO_REGISTRY_RECURSIVE,
            &mut iter,
        ) != KERN_SUCCESS
        {
            IOObjectRelease(root);
            return None;
        }

        let mut found = None;
        let mut fallback = None;
        loop {
            let service = IOIteratorNext(iter);
            if service == IO_OBJECT_NULL {
                break;
            }

            let is_proxy = entry_name(service) == "DCPAVServiceProxy"
                && cf_search_string(service, "Location").as_deref() == Some("External");
            if is_proxy {
                let path = entry_path(service);
                let exact = port_marker.as_deref().is_none_or(|marker| path.contains(marker));
                let av = IOAVServiceCreateWithService(kCFAllocatorDefault, service);
                if !av.is_null() {
                    let candidate = (AvService(av), chip_address_for(service));
                    if exact && found.is_none() {
                        found = Some(candidate);
                    } else if fallback.is_none() {
                        fallback = Some(candidate);
                    }
                }
            }
            IOObjectRelease(service);
        }

        IOObjectRelease(iter);
        IOObjectRelease(root);
        found.or(fallback)
    }
}

/// M1 Macs route built-in HDMI through an MCDP29xx bridge that uses a different I2C address.
fn chip_address_for(proxy: ffi::IoService) -> u32 {
    use super::ffi::*;
    unsafe {
        let mut parent = IO_OBJECT_NULL;
        if IORegistryEntryGetParentEntry(
            proxy,
            SERVICE_PLANE.as_ptr() as *const c_char,
            &mut parent,
        ) != KERN_SUCCESS
        {
            return 0x37;
        }
        let provider = cf_property_string(parent, "EPICProviderClass");
        IOObjectRelease(parent);
        match provider.as_deref() {
            Some("AppleDCPMCDP29XX") => 0xB7,
            _ => 0x37,
        }
    }
}

/// True while the display is online (a successful input switch removes it from the list).
pub fn is_online(id: u32) -> bool {
    let mut ids = [0u32; 16];
    let mut count = 0u32;
    if unsafe { ffi::CGGetOnlineDisplayList(16, ids.as_mut_ptr(), &mut count) } != 0 {
        return false;
    }
    ids[..count as usize].contains(&id)
}

/// Parses `vcp(...)` out of a capabilities string and returns a feature's advertised values.
pub fn parse_vcp_values(caps: &str, code: u8) -> Option<Vec<u8>> {
    let start = caps.find("vcp(")? + 4;
    let body = &caps[start..];
    let mut depth = 1usize;
    let mut index = 0usize;
    let bytes = body.as_bytes();

    while index < bytes.len() && depth > 0 {
        match bytes[index] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }

        if depth == 1 {
            // Try to read a two-digit hex token at this position.
            if let Some((token, next)) = hex_token(bytes, index) {
                if next < bytes.len() && bytes[next] == b'(' {
                    if token as u8 == code {
                        let values_start = next + 1;
                        let mut values_depth = 1usize;
                        let mut cursor = values_start;
                        let mut values = Vec::new();
                        while cursor < bytes.len() && values_depth > 0 {
                            match bytes[cursor] {
                                b'(' => values_depth += 1,
                                b')' => {
                                    values_depth -= 1;
                                    if values_depth == 0 {
                                        break;
                                    }
                                }
                                _ => {
                                    if let Some((value, after)) = hex_token(bytes, cursor) {
                                        values.push(value as u8);
                                        cursor = after;
                                        continue;
                                    }
                                }
                            }
                            cursor += 1;
                        }
                        return Some(values);
                    }
                    // Skip the nested value list.
                    let mut nested = 1usize;
                    let mut cursor = next + 1;
                    while cursor < bytes.len() && nested > 0 {
                        match bytes[cursor] {
                            b'(' => nested += 1,
                            b')' => nested -= 1,
                            _ => {}
                        }
                        cursor += 1;
                    }
                    index = cursor;
                    continue;
                }
                index = next;
                continue;
            }
        }
        index += 1;
    }
    None
}

fn hex_token(bytes: &[u8], at: usize) -> Option<(u16, usize)> {
    let mut cursor = at;
    while cursor < bytes.len() && bytes[cursor] == b' ' {
        cursor += 1;
    }
    let start = cursor;
    let mut value = 0u16;
    while cursor < bytes.len() && cursor - start < 4 {
        let Some(digit) = (bytes[cursor] as char).to_digit(16) else { break };
        value = value * 16 + digit as u16;
        cursor += 1;
    }
    ((cursor - start) >= 2).then_some((value, cursor))
}

/// Human names for common MCCS input source values, including LG's alternative codes.
pub fn input_name(value: u16) -> &'static str {
    match value {
        0x01 => "VGA 1",
        0x02 => "VGA 2",
        0x03 => "DVI 1",
        0x04 => "DVI 2",
        0x05 => "Composite 1",
        0x06 => "Composite 2",
        0x0F => "DisplayPort 1",
        0x10 => "DisplayPort 2",
        0x11 => "HDMI 1",
        0x12 => "HDMI 2",
        0x13 => "HDMI 3",
        0x1B => "USB-C 1",
        0x1C => "USB-C 2",
        0x90 => "HDMI 1 (LG alt)",
        0x91 => "HDMI 2 (LG alt)",
        0xD0 => "DisplayPort (LG alt)",
        0xD1 => "DisplayPort 2 (LG alt)",
        0xD2 => "USB-C (LG alt)",
        _ => "Input",
    }
}
