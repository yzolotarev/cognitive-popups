//! The tray icon (StatusNotifierItem, e.g. waybar's tray): the way into the hub
//! (F-73). A click sends the same signal a hotkey would, so the tray needs no
//! access to the windows. No tray on the system — no icon, nothing else breaks.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use ksni::blocking::TrayMethods;

struct Tray {
    socket: PathBuf,
}

impl Tray {
    fn send(&self, line: &str) {
        if let Ok(mut stream) = UnixStream::connect(&self.socket) {
            let _ = stream.write_all(format!("{line}\n").as_bytes());
        }
    }
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "cognitive-popups".into()
    }

    fn title(&self) -> String {
        "Cognitive Popups".into()
    }

    fn icon_name(&self) -> String {
        "accessories-dictionary-symbolic".into()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send("show hub");
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            StandardItem {
                label: "Открыть хаб".into(),
                activate: Box::new(|tray: &mut Self| tray.send("show hub")),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Справка (Alt+K)".into(),
                activate: Box::new(|tray: &mut Self| tray.send("show help")),
                ..Default::default()
            }
            .into(),
        ]
    }
}

pub fn spawn(socket: PathBuf) {
    // `assume_sni_available`: waybar may start after the daemon.
    match (Tray { socket }).assume_sni_available(true).spawn() {
        // The icon lives as long as the daemon.
        Ok(handle) => std::mem::forget(handle),
        Err(e) => eprintln!("tray: {e}"),
    }
}
