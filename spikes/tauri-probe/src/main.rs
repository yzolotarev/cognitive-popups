//! One-scenario GUI probe: a small translucent popup, no focus theft, auto-close.
//! Run under the real compositor; it is not part of the product build.

use std::time::{Duration, Instant};
use tauri::webview::PageLoadEvent;

fn main() {
    let started = Instant::now();
    tauri::Builder::default()
        .on_page_load(move |_window, payload| {
            if matches!(payload.event(), PageLoadEvent::Finished) {
                eprintln!("page_load_ms={}", started.elapsed().as_millis());
                report_rss("after_page_load");
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(3));
                handle.exit(0);
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("run tauri probe");
}

fn report_rss(label: &str) {
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            if line.starts_with("VmRSS:") {
                eprintln!("{label}_{}", line.replace(' ', "_").to_lowercase());
            }
        }
    }
}
