//! One instance per support directory.
//!
//! The control socket is the liveness check, but it is removed on quit and can be absent while an
//! instance is starting. A lock file gives the same guarantee for a dev or test support directory
//! and makes two builds with different support directories independent.

use crate::config::Config;
use std::fs::{File, OpenOptions};
use std::os::unix::io::AsRawFd;
pub struct Instance {
    file: File,
}

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

/// Takes an exclusive, non-blocking lock. The file itself stays behind on purpose: removing a
/// lock file while another process is opening it creates a race.
pub fn acquire() -> Result<Instance, String> {
    let dir = Config::support_dir();
    std::fs::create_dir_all(&dir).map_err(|error| format!("support dir: {error}"))?;
    let path = dir.join("instance.lock");
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| format!("lock file: {error}"))?;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result != 0 {
        return Err(format!("another instance owns {}", path.display()));
    }
    Ok(Instance { file })
}
