//! The real desktop on Hyprland: `wl-paste` / `wl-copy` (wl-clipboard),
//! `hyprctl` for the active window and for Ctrl+C, `wtype` as a fallback.

use std::io::Write;
use std::process::{Command, Stdio};

use crate::{Desktop, Window};

/// Ctrl+C to the active window: Hyprland 0.56 (Lua) and the classic form. The key
/// is the keycode (54 = the C key), not the letter: with a Russian layout active
/// the letter «C» is not found («key not found») and nothing is copied.
const LUA_COPY: &str =
    "hl.dsp.send_shortcut({ mods = \"CTRL\", key = \"code:54\", window = \"activewindow\" })";
const CLASSIC_COPY: &str = "sendshortcut CTRL,code:54,activewindow";

#[derive(Debug, Default, Clone, Copy)]
pub struct Wayland;

fn output(program: &str, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new(program)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

fn lines(bytes: Option<Vec<u8>>) -> Vec<String> {
    bytes
        .and_then(|b| String::from_utf8(b).ok())
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

impl Desktop for Wayland {
    fn active_window(&self) -> Option<Window> {
        let json = output("hyprctl", &["activewindow", "-j"])?;
        let value: serde_json::Value = serde_json::from_slice(&json).ok()?;
        Some(Window {
            class: value["class"].as_str()?.to_string(),
            title: value["title"].as_str().unwrap_or_default().to_string(),
        })
    }

    fn primary_types(&self) -> Vec<String> {
        lines(output("wl-paste", &["--primary", "--list-types"]))
    }

    fn primary_text(&self) -> Option<String> {
        output("wl-paste", &["--primary", "--no-newline", "--type", "text"])
            .and_then(|b| String::from_utf8(b).ok())
    }

    fn clipboard_types(&self) -> Vec<String> {
        lines(output("wl-paste", &["--list-types"]))
    }

    fn clipboard_read(&self, mime: &str) -> Option<Vec<u8>> {
        output("wl-paste", &["--no-newline", "--type", mime])
    }

    /// wl-copy offers one type per call; the first restorable one is enough
    /// for text, and an image keeps its own type.
    fn clipboard_write(&self, items: &[(String, Vec<u8>)]) -> Result<(), String> {
        let (mime, bytes) = items
            .iter()
            .find(|(m, _)| m == "image/png")
            .or_else(|| items.first())
            .ok_or("nothing to restore")?;
        let mut child = Command::new("wl-copy")
            .args(["--type", mime])
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        child
            .stdin
            .take()
            .ok_or("no stdin")?
            .write_all(bytes)
            .map_err(|e| e.to_string())?;
        let status = child.wait().map_err(|e| e.to_string())?;
        status
            .success()
            .then_some(())
            .ok_or("wl-copy failed".into())
    }

    fn send_copy(&self) -> Result<(), String> {
        // `hyprctl dispatch` exits 0 even when the dispatcher is unknown, so the
        // answer text is checked: «ok» only. Hyprland 0.56 (Lua) and the classic
        // syntax differ, both are tried before the `wtype` fallback.
        let dispatched = |argument: &str| {
            Command::new("hyprctl")
                .args(["dispatch", argument])
                .stderr(Stdio::null())
                .output()
                .is_ok_and(|o| {
                    o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "ok"
                })
        };
        if dispatched(LUA_COPY) || dispatched(CLASSIC_COPY) {
            return Ok(());
        }
        let wtype = Command::new("wtype")
            .args(["-M", "ctrl", "c", "-m", "ctrl"])
            .status()
            .map_err(|e| e.to_string())?;
        wtype.success().then_some(()).ok_or("wtype failed".into())
    }

    fn sleep_ms(&self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}
