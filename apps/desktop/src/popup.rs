//! A layer-shell window at the cursor (S7, B45): pre-created, shown on the
//! monitor under the cursor, kept on screen. A window the reader opened with a
//! key takes the keyboard (`OnDemand`); one that opened by itself does not
//! (INV-13). Esc always closes; what was typed is kept for next time.

use std::process::Command;

use gtk::gdk;
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

const OFFSET: i32 = 12;

/// Who opened the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opened {
    ByKey,
    #[expect(
        dead_code,
        reason = "the pill and nudges (P7 step 6) open by themselves"
    )]
    ByItself,
}

pub fn new(app: &adw::Application, namespace: &str, width: i32) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::new(app);
    window.add_css_class("cp-popup");
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_namespace(Some(namespace));
    window.set_keyboard_mode(KeyboardMode::None);
    window.set_anchor(Edge::Top, true);
    window.set_anchor(Edge::Left, true);
    window.set_exclusive_zone(-1);
    window.set_default_size(width, -1);

    // Pointing at a window that opened by itself makes it ready for typing
    // before the click lands (Hyprland decides focus at the press, F7).
    let hover = gtk::EventControllerMotion::new();
    let w = window.clone();
    hover.connect_enter(move |_, _, _| {
        if w.keyboard_mode() == KeyboardMode::None {
            w.set_keyboard_mode(KeyboardMode::OnDemand);
        }
    });
    window.add_controller(hover);
    window
}

/// Shows the window next to the cursor, or raises it if it is already open.
pub fn show(window: &gtk::ApplicationWindow, size: (i32, i32), opened: Opened) {
    let mode = match opened {
        Opened::ByKey => KeyboardMode::OnDemand,
        Opened::ByItself => KeyboardMode::None,
    };
    if !window.is_visible() {
        let (monitor, x, y) = place(cursor(), size);
        window.set_monitor(monitor.as_ref());
        window.set_margin(Edge::Left, x);
        window.set_margin(Edge::Top, y);
    }
    window.set_keyboard_mode(mode);
    window.present();
}

pub fn hide(window: &gtk::ApplicationWindow) {
    window.set_visible(false);
    window.set_keyboard_mode(KeyboardMode::None);
}

/// The monitor under the cursor and the window position on it (layer-shell
/// margins are local to one output; the cursor position is global).
fn place((gx, gy): (i32, i32), (width, height): (i32, i32)) -> (Option<gdk::Monitor>, i32, i32) {
    let monitors = gdk::Display::default()
        .map(|d| d.monitors())
        .map(|list| {
            (0..list.n_items())
                .filter_map(|i| list.item(i)?.downcast::<gdk::Monitor>().ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let hit = monitors.iter().find(|m| {
        let g = m.geometry();
        gx >= g.x() && gx < g.x() + g.width() && gy >= g.y() && gy < g.y() + g.height()
    });
    match hit.or(monitors.first()) {
        Some(m) => {
            let g = m.geometry();
            let x = (gx - g.x() + OFFSET).clamp(0, (g.width() - width).max(0));
            let y = (gy - g.y() + OFFSET).clamp(0, (g.height() - height).max(0));
            (Some(m.clone()), x, y)
        }
        None => (None, gx + OFFSET, gy + OFFSET),
    }
}

/// Hyprland knows the cursor; `COGNITIVE_CURSOR=x,y` stands in for it in tests.
fn cursor() -> (i32, i32) {
    let parse = |s: &str| {
        let (x, y) = s.trim().split_once(',')?;
        Some((x.trim().parse::<i32>().ok()?, y.trim().parse::<i32>().ok()?))
    };
    if let Some(p) = std::env::var("COGNITIVE_CURSOR")
        .ok()
        .and_then(|s| parse(&s))
    {
        return p;
    }
    Command::new("hyprctl")
        .arg("cursorpos")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| parse(&s))
        .unwrap_or((200, 150))
}
