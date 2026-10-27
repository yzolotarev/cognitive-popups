//! Application services. Network and credential errors are deliberately redacted:
//! provider response bodies, keys and user text never enter diagnostics.
pub mod archive;
pub mod assessment;
pub mod experiment;
pub mod json;
pub mod learning;
pub mod mechanism;
pub mod outbox;
pub mod provider;
pub mod store;

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("storage error")]
    Database(#[from] rusqlite::Error),
    #[error("cannot access application storage")]
    Io(#[from] std::io::Error),
    #[error("invalid stored data")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Invalid(&'static str),
    #[error("another window saved this item; reopen it before editing")]
    Conflict,
}

pub fn data_dir() -> Result<PathBuf, Error> {
    let dirs = directories::ProjectDirs::from("", "", "cognitive-exoskeleton-v1")
        .ok_or(Error::Invalid("application data directory unavailable"))?;
    let path = dirs.state_dir().unwrap_or(dirs.data_local_dir()).to_owned();
    std::fs::create_dir_all(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(path)
}

/// OS locks are released on crash. A second hotkey invocation cannot open a
/// competing editor or create an extra capture/pseudo-attempt.
/// Every surface that may exist only once per desktop session. The session
/// window is its own surface, so it must be listed here as well.
pub const INSTANCE_WINDOWS: &[&str] =
    &["hub", "note", "four_words", "task", "prediction", "session"];

pub struct InstanceGuard(std::fs::File);

impl InstanceGuard {
    pub fn acquire(name: &str) -> Result<Self, Error> {
        if !INSTANCE_WINDOWS.contains(&name) {
            return Err(Error::Invalid("invalid instance lock"));
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(data_dir()?.join(format!("{name}.lock")))?;
        file.try_lock().map_err(|_| {
            Error::Invalid(
                "this window is already open; return to it instead of creating a duplicate",
            )
        })?;
        Ok(Self(file))
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_user_facing_surface_has_its_own_single_instance_lock() {
        for name in ["hub", "note", "four_words", "task", "prediction", "session"] {
            assert!(INSTANCE_WINDOWS.contains(&name), "{name} must be lockable");
        }
        assert!(!INSTANCE_WINDOWS.contains(&"unknown"));
    }
}
