//! One-scenario GUI probe: a small translucent popup, no focus theft, auto-close.
//! Run under the real compositor; it is not part of the product build.

use std::time::{Duration, Instant};

slint::include_modules!();

fn main() {
    let started = Instant::now();
    let ui = ProbeWindow::new().expect("create window");
    ui.set_body("Проверка: наценка ≠ маржа. Кириллица, цифры 123.".into());
    ui.show().expect("show window");
    eprintln!("shown_ms={}", started.elapsed().as_millis());
    report_rss("after_show");

    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::SingleShot,
        Duration::from_secs(2),
        || slint::quit_event_loop().expect("quit event loop"),
    );
    slint::run_event_loop().expect("run event loop");
    drop(timer);
    report_rss("after_close");
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
