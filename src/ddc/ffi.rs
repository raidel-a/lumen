//! Thin IOKit / CoreDisplay bindings used by the DDC engine.
//!
//! These are the same private-ish symbols every Apple silicon DDC tool uses: an `IOAVService`
//! for the display's DCP AV proxy, then I2C reads and writes through it.

#![allow(non_snake_case, non_camel_case_types)]

use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use core_foundation_sys::base::{CFAllocatorRef, CFTypeRef, kCFAllocatorDefault};
use std::ffi::{c_char, c_void};

pub type IoObject = u32;
pub type IoIterator = u32;
pub type IoService = u32;
pub type KernReturn = i32;
pub type IoOptionBits = u32;
pub type IoName = [c_char; 128];

pub const KERN_SUCCESS: KernReturn = 0;
pub const IO_OBJECT_NULL: IoObject = 0;
pub const SERVICE_PLANE: &[u8] = b"IOService\0";
pub const IO_REGISTRY_RECURSIVE: IoOptionBits = 1;

const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

unsafe extern "C" {
    pub fn CFRelease(cf: CFTypeRef);
    pub fn CFGetTypeID(cf: CFTypeRef) -> usize;
    pub fn CFStringGetTypeID() -> usize;
    pub fn CFStringGetLength(string: CFTypeRef) -> isize;
    pub fn CFStringGetCString(string: CFTypeRef, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
    pub fn CFDictionaryGetValue(dict: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    pub fn IORegistryGetRootEntry(main_port: u32) -> IoObject;
    pub fn IORegistryEntryCreateIterator(
        entry: IoObject,
        plane: *const c_char,
        options: IoOptionBits,
        iterator: *mut IoIterator,
    ) -> KernReturn;
    pub fn IOIteratorNext(iterator: IoIterator) -> IoObject;
    pub fn IOObjectRelease(object: IoObject) -> KernReturn;
    pub fn IORegistryEntryGetName(entry: IoObject, name: *mut c_char) -> KernReturn;
    pub fn IORegistryEntryGetParentEntry(
        entry: IoObject,
        plane: *const c_char,
        parent: *mut IoObject,
    ) -> KernReturn;
    pub fn IORegistryEntryCreateCFProperty(
        entry: IoObject,
        key: CFTypeRef,
        allocator: CFAllocatorRef,
        options: IoOptionBits,
    ) -> CFTypeRef;
    pub fn IORegistryEntrySearchCFProperty(
        entry: IoObject,
        plane: *const c_char,
        key: CFTypeRef,
        allocator: CFAllocatorRef,
        options: IoOptionBits,
    ) -> CFTypeRef;
    pub fn IORegistryEntryCopyFromPath(main_port: u32, path: CFTypeRef) -> IoObject;
    pub fn IORegistryEntryGetPath(entry: IoObject, plane: *const c_char, path: *mut c_char) -> KernReturn;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    pub fn CGGetOnlineDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    pub fn CGDisplayIsBuiltin(display: u32) -> u8;
}

#[link(name = "CoreDisplay", kind = "framework")]
unsafe extern "C" {
    pub fn CoreDisplay_DisplayCreateInfoDictionary(display_id: u32) -> CFTypeRef;
    pub fn IOAVServiceCreateWithService(allocator: CFAllocatorRef, service: IoService) -> *mut c_void;
    pub fn IOAVServiceReadI2C(
        service: *mut c_void,
        chip_address: u32,
        offset: u32,
        output_buffer: *mut c_void,
        output_buffer_size: u32,
    ) -> KernReturn;
    pub fn IOAVServiceWriteI2C(
        service: *mut c_void,
        chip_address: u32,
        data_address: u32,
        input_buffer: *const c_void,
        input_buffer_size: u32,
    ) -> KernReturn;
}

/// Copies a CFString into a Rust `String`.
pub fn cf_string(cf: CFTypeRef) -> Option<String> {
    unsafe {
        if cf.is_null() || CFGetTypeID(cf) != CFStringGetTypeID() {
            return None;
        }
        let length = CFStringGetLength(cf);
        let mut buffer = vec![0u8; (length as usize + 1) * 4];
        if CFStringGetCString(cf, buffer.as_mut_ptr() as *mut c_char, buffer.len() as isize, CF_STRING_ENCODING_UTF8) == 0 {
            return None;
        }
        let end = buffer.iter().position(|b| *b == 0).unwrap_or(buffer.len());
        Some(String::from_utf8_lossy(&buffer[..end]).into_owned())
    }
}

fn cf_string_key(key: &str) -> CFString {
    CFString::new(key)
}

/// Reads a CFString property of a registry entry.
pub fn cf_property_string(entry: IoObject, key: &str) -> Option<String> {
    let key = cf_string_key(key);
    unsafe {
        let prop = IORegistryEntryCreateCFProperty(entry, key.as_concrete_TypeRef() as CFTypeRef, kCFAllocatorDefault, 0);
        if prop.is_null() {
            return None;
        }
        let value = cf_string(prop);
        CFRelease(prop);
        value
    }
}

/// Reads a CFString property anywhere below `entry`.
pub fn cf_search_string(entry: IoObject, key: &str) -> Option<String> {
    let key = cf_string_key(key);
    unsafe {
        let prop = IORegistryEntrySearchCFProperty(
            entry,
            SERVICE_PLANE.as_ptr() as *const c_char,
            key.as_concrete_TypeRef() as CFTypeRef,
            kCFAllocatorDefault,
            IO_REGISTRY_RECURSIVE,
        );
        if prop.is_null() {
            return None;
        }
        let value = cf_string(prop);
        CFRelease(prop);
        value
    }
}

/// Returns `dict["ProductAttributes"]["<field>"]` of the entry's `DisplayAttributes` property.
pub fn display_attribute_named(entry: IoObject, field: &str) -> Option<String> {
    let attrs_key = cf_string_key("DisplayAttributes");
    unsafe {
        let attrs = IORegistryEntrySearchCFProperty(
            entry,
            SERVICE_PLANE.as_ptr() as *const c_char,
            attrs_key.as_concrete_TypeRef() as CFTypeRef,
            kCFAllocatorDefault,
            IO_REGISTRY_RECURSIVE,
        );
        if attrs.is_null() {
            return None;
        }
        let products_key = cf_string_key("ProductAttributes");
        let products = CFDictionaryGetValue(attrs, products_key.as_concrete_TypeRef() as CFTypeRef);
        let value = if products.is_null() {
            None
        } else {
            let field_key = cf_string_key(field);
            cf_string(CFDictionaryGetValue(products, field_key.as_concrete_TypeRef() as CFTypeRef))
        };
        CFRelease(attrs);
        value
    }
}

/// Returns the entry's class name as IOKit reports it.
pub fn entry_name(entry: IoObject) -> String {
    let mut buf: IoName = [0; 128];
    unsafe {
        if IORegistryEntryGetName(entry, buf.as_mut_ptr()) != KERN_SUCCESS {
            return String::new();
        }
        let bytes: Vec<u8> = buf.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

/// Owns an `IOAVServiceRef`, releasing it on drop.
pub struct AvService(pub *mut c_void);

impl Drop for AvService {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0 as CFTypeRef) }
        }
    }
}

/// Converts an IODisplayLocation string into a registry entry.
pub fn entry_from_location(location: &str) -> Option<IoObject> {
    let location = CFString::new(location);
    let entry = unsafe { IORegistryEntryCopyFromPath(0, location.as_concrete_TypeRef() as CFTypeRef) };
    (entry != IO_OBJECT_NULL).then_some(entry)
}

/// The entry's path in the IOService plane.
pub fn entry_path(entry: IoObject) -> String {
    let mut buf = [0i8; 2048];
    unsafe {
        if IORegistryEntryGetPath(entry, SERVICE_PLANE.as_ptr() as *const c_char, buf.as_mut_ptr()) != KERN_SUCCESS {
            return String::new();
        }
        let bytes: Vec<u8> = buf.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

