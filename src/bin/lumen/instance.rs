//! One instance per support directory.
//!
//! The control socket is the liveness check, but it is removed on quit and can be absent while an
//! instance is starting. A lock file gives the same guarantee for a dev or test support directory
//! and makes two builds with different support directories independent.

use crate::config::Config;
use std::fs::{File, OpenOptions};
use std::os::unix::io::AsRawFd;
use std::path::Path;

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
    acquire_at(&Config::support_dir())
}

fn acquire_at(dir: &Path) -> Result<Instance, String> {
    std::fs::create_dir_all(dir).map_err(|error| format!("support dir: {error}"))?;
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

#[cfg(test)]
mod tests {
    use super::acquire_at;

    #[test]
    fn only_one_instance_per_directory() {
        let dir = std::env::temp_dir().join(format!("lumen-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = acquire_at(&dir).expect("first lock");
        assert!(acquire_at(&dir).is_err());
        drop(first);
        let second = acquire_at(&dir).expect("lock after release");
        drop(second);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
