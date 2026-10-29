//! The AI settings of the hub (S2 rule 1, Q-68, Q-69): provider, model and
//! address in `settings.json`; the key in the Secret Service keyring, or, when
//! there is none, in a file only the owner can read — and the reader is told so.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::AiConfig;

const SERVICE: [&str; 4] = ["service", "cognitive-popups", "account", "ai-key"];

/// `$XDG_CONFIG_HOME/cognitive-popups`, else `~/.config/cognitive-popups`.
pub fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("cognitive-popups")
}

pub fn load_settings(dir: &Path) -> Option<AiConfig> {
    let text = std::fs::read_to_string(dir.join("settings.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// The key is never written here: `AiConfig` skips it when serialized.
pub fn save_settings(dir: &Path, config: &AiConfig) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("settings.json"), text).map_err(|e| e.to_string())
}

/// Where the key ended up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyPlace {
    Keyring,
    /// No keyring: a private file. The hub shows a warning (Q-69).
    File(PathBuf),
}

fn key_file(dir: &Path) -> PathBuf {
    dir.join("key")
}

/// Saves the key: the keyring if `secret-tool` works, else a 600 file.
pub fn store_key(dir: &Path, key: &str, try_keyring: bool) -> Result<KeyPlace, String> {
    let key = key.trim();
    if try_keyring && keyring_store(key) {
        let _ = std::fs::remove_file(key_file(dir));
        return Ok(KeyPlace::Keyring);
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = key_file(dir);
    write_private(&path, key)?;
    Ok(KeyPlace::File(path))
}

pub fn load_key(dir: &Path, try_keyring: bool) -> Option<String> {
    if try_keyring && let Some(key) = keyring_lookup() {
        return Some(key);
    }
    let path = key_file(dir);
    if !private(&path) {
        return None;
    }
    std::fs::read_to_string(path)
        .ok()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
}

pub fn forget_key(dir: &Path, try_keyring: bool) {
    if try_keyring {
        let _ = Command::new("secret-tool")
            .arg("clear")
            .args(SERVICE)
            .status();
    }
    let _ = std::fs::remove_file(key_file(dir));
}

fn keyring_store(key: &str) -> bool {
    let child = Command::new("secret-tool")
        .args(["store", "--label=Cognitive Popups AI key"])
        .args(SERVICE)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else { return false };
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        if stdin.write_all(key.as_bytes()).is_err() {
            return false;
        }
    }
    child.wait().map(|s| s.success()).unwrap_or(false) && keyring_lookup().as_deref() == Some(key)
}

fn keyring_lookup() -> Option<String> {
    let output = Command::new("secret-tool")
        .arg("lookup")
        .args(SERVICE)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let key = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (output.status.success() && !key.is_empty()).then_some(key)
}

#[cfg(unix)]
fn write_private(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let _ = std::fs::remove_file(path);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(text.as_bytes()).map_err(|e| e.to_string())
}

#[cfg(not(unix))]
fn write_private(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| e.to_string())
}

/// A key file others can read is not used (Q-69).
#[cfg(unix)]
fn private(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o077 == 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn private(path: &Path) -> bool {
    path.exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProviderKind;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cp-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn settings_round_trip_without_the_key() {
        let dir = dir("round");
        let config = AiConfig::new(
            ProviderKind::OpenaiCompatible,
            "m",
            Some("https://x/v1".into()),
        )
        .with_key(Some("secret".into()));
        save_settings(&dir, &config).unwrap();
        let text = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(!text.contains("secret"));
        let loaded = load_settings(&dir).unwrap();
        assert_eq!(loaded.model, "m");
        assert_eq!(loaded.key(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn without_a_keyring_the_key_is_a_private_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = dir("file");
        let place = store_key(&dir, " k-123 ", false).unwrap();
        let KeyPlace::File(path) = place else {
            panic!()
        };
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(load_key(&dir, false).as_deref(), Some("k-123"));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(load_key(&dir, false), None, "readable by others → not used");
        forget_key(&dir, false);
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
