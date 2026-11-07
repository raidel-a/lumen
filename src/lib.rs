//! Lumen: display control for macOS.
//!
//! The engine talks DDC/CI to external displays over IOKit's `IOAVService`, which is the path
//! that works on Apple silicon. It exposes raw VCP reads and writes, plus the input-source
//! helpers that know about LG's alternate addressing.

pub mod ddc;
pub mod native;

pub use ddc::{Display, VcpValue};
