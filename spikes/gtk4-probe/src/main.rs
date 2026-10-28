//! P0 stack probe (`docs/v2/ROADMAP.md`, B35): Rust + GTK 4 + gtk4-layer-shell
//! on Hyprland. Every criterion prints one `key=value` line, so the run can be
//! pasted back as is.
//!
//!   gtk4-probe daemon      pre-creates the window and listens on a socket
//!   gtk4-probe show        a window that opens by itself: no keyboard
//!   gtk4-probe show --focus on_demand|exclusive
//!                          a window opened by a hotkey: it must take the
//!                          keyboard at once (INV-13); which mode does it on
//!                          Hyprland is what this measures
//!
//! 1. The window appears at the cursor without taking the keyboard
//!    (`KeyboardMode::None`); a click switches it to `OnDemand`.
//! 2. `latency_ms` — from the signal to the first painted frame.
//! 3. Russian input and paste: type into the field, press Enter.
//! 4. The formula is rendered with typst if installed, else shown as text.
//! 5. Esc hides the window; a second `show` raises the same window.

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;
use std::time::Instant;

use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

const FORMULA: &str = "P(A∣B) = P(A∩B) / P(B)";

thread_local! {
    static PROBE: RefCell<Option<Probe>> = const { RefCell::new(None) };
}

struct Probe {
    window: gtk::ApplicationWindow,
    entry: gtk::Entry,
}

fn socket() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").unwrap_or_else(|| "/tmp".into());
    PathBuf::from(dir).join("gtk4-probe.sock")
}

fn report(line: &str) {
    println!("{line}");
    let _ = std::io::stdout().flush();
}

fn main() -> glib::ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("show") => {
            let focus = match std::env::args().nth(2).as_deref() {
                Some("--focus") => std::env::args()
                    .nth(3)
                    .unwrap_or_else(|| "on_demand".into()),
                _ => "none".into(),
            };
            let message = format!("show {focus}\n");
            let sent =
                UnixStream::connect(socket()).and_then(|mut s| s.write_all(message.as_bytes()));
            if let Err(e) = sent {
                eprintln!("daemon not running ({e}); start: gtk4-probe daemon");
                return glib::ExitCode::FAILURE;
            }
            glib::ExitCode::SUCCESS
        }
        Some("hide") => {
            let _ = UnixStream::connect(socket()).and_then(|mut s| s.write_all(b"hide\n"));
            glib::ExitCode::SUCCESS
        }
        Some("daemon") => daemon(),
        _ => {
            eprintln!("usage: gtk4-probe daemon | show [--focus on_demand|exclusive] | hide");
            glib::ExitCode::FAILURE
        }
    }
}

fn daemon() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("dev.cognitive.Gtk4Probe")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(|app| {
        report(&format!(
            "layer_shell_supported={}",
            gtk4_layer_shell::is_supported()
        ));
        let probe = build(app);
        PROBE.with(|p| *p.borrow_mut() = Some(probe));
        listen();
        report("ready=1");
    });
    // The window is hidden between shows; keep the daemon alive anyway.
    let _hold = app.hold();
    app.run_with_args::<&str>(&[])
}

fn build(app: &gtk::Application) -> Probe {
    let window = gtk::ApplicationWindow::new(app);
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_namespace(Some("cognitive-probe"));
    window.set_keyboard_mode(KeyboardMode::None);
    window.set_anchor(Edge::Top, true);
    window.set_anchor(Edge::Left, true);
    // Margins count from the monitor edge, not from below a bar's reserved zone.
    window.set_exclusive_zone(-1);
    window.set_default_size(360, -1);

    let css = gtk::CssProvider::new();
    css.load_from_string(
        "window { background: rgba(250,250,252,0.94); border-radius: 14px; }
         box.root { padding: 16px; }
         label.title { font-weight: 600; font-size: 15pt; }
         label.formula { font-size: 13pt; }",
    );
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("a display"),
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
    root.add_css_class("root");
    let title = gtk::Label::new(Some("Проба стека"));
    title.add_css_class("title");
    title.set_xalign(0.0);
    root.append(&title);
    root.append(&formula());
    let entry = gtk::Entry::builder()
        .placeholder_text("Напиши по-русски и нажми Enter")
        .build();
    entry.connect_activate(|entry| report(&format!("input={}", entry.text())));
    root.append(&entry);
    window.set_child(Some(&root));

    // Criterion 1: pointing at the window turns keyboard input on, so the
    // compositor already sees OnDemand when the click arrives; switching only
    // on the press is too late (Hyprland decides focus at the press, F7).
    let hover = gtk::EventControllerMotion::new();
    let w = window.clone();
    hover.connect_enter(move |_, _, _| {
        if w.keyboard_mode() == KeyboardMode::None {
            w.set_keyboard_mode(KeyboardMode::OnDemand);
            report("keyboard=on_demand (pointer)");
        }
    });
    window.add_controller(hover);
    let click = gtk::GestureClick::new();
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    let w = window.clone();
    click.connect_pressed(move |_, _, _, _| {
        if w.keyboard_mode() != KeyboardMode::OnDemand {
            w.set_keyboard_mode(KeyboardMode::OnDemand);
            report("keyboard=on_demand");
        }
    });
    window.add_controller(click);

    // Criterion 5: Esc hides.
    let keys = gtk::EventControllerKey::new();
    let w = window.clone();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape {
            w.set_visible(false);
            w.set_keyboard_mode(KeyboardMode::None);
            report("closed_by=escape");
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(keys);

    // Whether the compositor actually gave the keyboard to the window.
    let focus = gtk::EventControllerFocus::new();
    focus.connect_enter(|_| report("keyboard_focus=gained"));
    window.add_controller(focus);

    WidgetExt::realize(&window);
    Probe { window, entry }
}

/// Criterion 4: a local render, never a network service.
fn formula() -> gtk::Widget {
    let dir = std::env::temp_dir().join("gtk4-probe");
    let _ = std::fs::create_dir_all(&dir);
    let source = dir.join("f.typ");
    let png = dir.join("f.png");
    let typ = "#set page(width: auto, height: auto, margin: 4pt, fill: none)\n\
               #set text(size: 15pt)\n\
               $ P(A | B) = P(A inter B) / P(B) $\n";
    let started = Instant::now();
    let rendered = std::fs::write(&source, typ).is_ok()
        && Command::new("typst")
            .args(["compile", "--format", "png", "--ppi", "192"])
            .arg(&source)
            .arg(&png)
            .status()
            .is_ok_and(|s| s.success());
    if rendered {
        report(&format!(
            "formula=typst render_ms={}",
            started.elapsed().as_millis()
        ));
        let picture = gtk::Picture::for_filename(&png);
        picture.set_can_shrink(false);
        picture.set_halign(gtk::Align::Start);
        picture.upcast()
    } else {
        report("formula=text risk=no_typst");
        let label = gtk::Label::new(Some(FORMULA));
        label.add_css_class("formula");
        label.set_xalign(0.0);
        label.upcast()
    }
}

/// The socket thread hands each signal to the GTK main loop with its arrival time.
fn listen() {
    let path = socket();
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).expect("socket");
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            let received = Instant::now();
            if BufReader::new(stream).read_line(&mut line).is_ok() {
                if line.trim() == "hide" {
                    glib::MainContext::default().invoke(hide);
                    continue;
                }
                let mode = match line.trim() {
                    "show none" | "show" => KeyboardMode::None,
                    "show exclusive" => KeyboardMode::Exclusive,
                    "show on_demand" => KeyboardMode::OnDemand,
                    _ => continue,
                };
                glib::MainContext::default().invoke(move || show(received, mode));
            }
        }
    });
}

/// For the latency series: hide without a key press.
fn hide() {
    PROBE.with(|p| {
        if let Some(probe) = p.borrow().as_ref() {
            probe.window.set_visible(false);
        }
    });
}

fn show(received: Instant, mode: KeyboardMode) {
    PROBE.with(|p| {
        let guard = p.borrow();
        let Some(probe) = guard.as_ref() else { return };
        if probe.window.is_visible() {
            probe.window.set_keyboard_mode(mode);
            probe.window.present();
            report("second_show=raised_existing");
            return;
        }
        let (gx, gy) = cursor();
        let (monitor, x, y) = place(gx, gy);
        probe.window.set_monitor(monitor.as_ref());
        probe.window.set_margin(Edge::Left, x);
        probe.window.set_margin(Edge::Top, y);
        probe.window.set_keyboard_mode(mode);
        probe.entry.set_text("");
        if mode != KeyboardMode::None {
            probe.entry.grab_focus();
        }
        if let Some(clock) = probe.window.frame_clock() {
            let handler: Rc<RefCell<Option<glib::SignalHandlerId>>> = Rc::default();
            let inner = handler.clone();
            let id = clock.connect_after_paint(move |clock| {
                report(&format!(
                    "latency_ms={:.1}",
                    received.elapsed().as_secs_f64() * 1000.0
                ));
                if let Some(id) = inner.borrow_mut().take() {
                    clock.disconnect(id);
                }
            });
            *handler.borrow_mut() = Some(id);
        }
        probe.window.set_visible(true);
        report(&format!(
            "shown_at={x},{y} on={} cursor={gx},{gy} keyboard={mode:?}",
            monitor
                .as_ref()
                .and_then(|m| m.connector())
                .map(|c| c.to_string())
                .unwrap_or_else(|| "?".into())
        ));
    });
}

/// The monitor under the cursor and the window position on it: layer-shell
/// margins are local to one output, `hyprctl cursorpos` is global (F6). The
/// window is kept inside that monitor.
fn place(gx: i32, gy: i32) -> (Option<gdk::Monitor>, i32, i32) {
    const OFFSET: i32 = 12;
    const WIDTH: i32 = 360;
    const HEIGHT: i32 = 160;
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
            let x = (gx - g.x() + OFFSET).clamp(0, (g.width() - WIDTH).max(0));
            let y = (gy - g.y() + OFFSET).clamp(0, (g.height() - HEIGHT).max(0));
            (Some(m.clone()), x, y)
        }
        None => (None, gx + OFFSET, gy + OFFSET),
    }
}

/// Hyprland knows where the cursor is; other compositors do not tell clients.
/// `PROBE_CURSOR=x,y` stands in for it in tests without Hyprland.
fn cursor() -> (i32, i32) {
    let from_env = std::env::var("PROBE_CURSOR").ok().and_then(|s| {
        let (x, y) = s.split_once(',')?;
        Some((x.trim().parse::<i32>().ok()?, y.trim().parse::<i32>().ok()?))
    });
    if let Some(position) = from_env {
        return position;
    }
    let parsed = Command::new("hyprctl")
        .arg("cursorpos")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| {
            let (x, y) = s.trim().split_once(',')?;
            Some((x.trim().parse::<i32>().ok()?, y.trim().parse::<i32>().ok()?))
        });
    parsed.unwrap_or_else(|| {
        report("cursor=unknown (no hyprctl), placed at 200,150");
        (188, 138)
    })
}
