//! Cognitive Popups desktop (S7): one daemon, pre-created windows, hotkeys as
//! signals over a socket.
//!
//!   cognitive-popups daemon            start once per session (exec-once)
//!   cognitive-popups signal <window> [<mechanic>]   what a hotkey runs; windows: note, understand
//!   cognitive-popups hyprland-config   the lines for hyprland.conf
//!   cognitive-popups lesson export [--last | <id>] [--json]   one lesson for analysis

mod ctx;
mod doctor;
mod help;
mod hub;
mod hypothesis;
mod lesson;
mod math;
mod note;
mod notice;
mod nudge;
mod pill;
mod popup;
mod practice;
mod resume;
mod scale;
mod seed;
mod theme;
mod tray;
mod ui;
mod understand;
mod watch;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use adw::prelude::*;
use cognitive_exoskeleton_capture::{self as capture, Wayland};
use cognitive_exoskeleton_practice::routes::Route;
use cognitive_exoskeleton_understand::{self as understand_core, Mechanic};
use gtk::glib;

/// `seed` is not a window: demo data for the screenshots, only with the AI fixture.
const WINDOWS: [&str; 13] = [
    "note",
    "understand",
    "hypothesis",
    "practice",
    "lesson",
    "pill",
    "notice",
    "nudge",
    "hub",
    "resume",
    "help",
    "hide",
    "seed",
];

fn socket() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").unwrap_or_else(|| "/tmp".into());
    PathBuf::from(dir).join("cognitive-popups.sock")
}

/// `COGNITIVE_DB`, else `$XDG_DATA_HOME/cognitive-popups/journal.db`.
fn database() -> PathBuf {
    if let Some(path) = std::env::var_os("COGNITIVE_DB") {
        return path.into();
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("cognitive-popups");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("journal.db")
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("material" | "experiment" | "memory" | "nudge" | "subject" | "step" | "practice") => {
            core_cli(&args)
        }
        Some("lesson") if args.get(1).map(String::as_str) != Some("export") => core_cli(&args),
        Some("daemon") => daemon(),
        Some("doctor") => doctor::run(),
        Some("subjects") => subjects(&args[1..]),
        Some("lesson") => lesson_export(&args[1..]),
        Some("signal") => signal(
            args.get(1).map(String::as_str),
            args.get(2).map(String::as_str),
        ),
        Some("hyprland-config") => {
            match args.get(1).map(String::as_str) {
                None | Some("--classic") => print!("{}", hyprland_config()),
                Some("--lua") => print!("{}", hyprland_config_lua()),
                Some(_) => {
                    eprintln!("usage: cognitive-popups hyprland-config [--classic|--lua]");
                    return ExitCode::from(2);
                }
            }
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!(
                "usage: cognitive-popups daemon | doctor | signal <{}> | hyprland-config [--classic|--lua] | subjects import <file> [--dry-run] | subjects export | lesson export [--last | <lesson-id>] [--json]",
                WINDOWS.join("|")
            );
            ExitCode::from(2)
        }
    }
}

/// `lesson export [--last | <lesson-id>] [--json]` (B77): one lesson as
/// Markdown (or JSON) on stdout, for analysis. Reads only.
fn lesson_export(args: &[String]) -> ExitCode {
    let usage = || {
        eprintln!("usage: cognitive-popups lesson export [--last | <lesson-id>] [--json]");
        ExitCode::from(2)
    };
    if args.first().map(String::as_str) != Some("export") {
        return usage();
    }
    let mut json = false;
    let mut id = None;
    for arg in &args[1..] {
        match arg.as_str() {
            "--json" => json = true,
            "--last" => {}
            other if other.starts_with('-') || id.is_some() => return usage(),
            other => id = Some(other.to_string()),
        }
    }
    let offset = glib::DateTime::now_local()
        .map(|t| t.utc_offset().as_seconds() as i32)
        .unwrap_or(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    use cognitive_exoskeleton_lessons::export_lesson;
    let export = match export_lesson(&database(), id.as_deref(), now, offset) {
        Ok(export) => export,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };
    if json {
        match serde_json::to_string_pretty(&export) {
            Ok(text) => println!("{text}"),
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::from(1);
            }
        }
    } else {
        print!("{}", export.markdown());
    }
    ExitCode::SUCCESS
}

/// The whole study plan at once (B72): subjects, goals, deadlines and
/// programs from one text file, and back.
fn subjects(args: &[String]) -> ExitCode {
    use cognitive_exoskeleton_lessons::{
        LessonStore, export_subjects, import_subjects, parse_subjects,
    };
    let fail = |message: String| {
        eprintln!("{message}");
        ExitCode::from(1)
    };
    let mut store = match LessonStore::open(database()) {
        Ok(store) => store,
        Err(e) => return fail(format!("база: {e}")),
    };
    match (args.first().map(String::as_str), args.get(1)) {
        (Some("export"), None) => match export_subjects(&store) {
            Ok(text) => {
                print!("{text}");
                ExitCode::SUCCESS
            }
            Err(e) => fail(e.to_string()),
        },
        (Some("import"), Some(file)) => {
            let text = match std::fs::read_to_string(file) {
                Ok(text) => text,
                Err(e) => return fail(format!("{file}: {e}")),
            };
            let drafts = match parse_subjects(&text) {
                Ok(drafts) => drafts,
                Err(e) => return fail(format!("{file}: {e}")),
            };
            if args.iter().any(|a| a == "--dry-run") {
                for draft in &drafts {
                    println!(
                        "{} · тем: {}{}",
                        draft.name,
                        draft.topics.len(),
                        draft
                            .deadline
                            .as_deref()
                            .map(|d| format!(" · срок {d}"))
                            .unwrap_or_default()
                    );
                }
                println!("Проверка прошла, ничего не записано.");
                return ExitCode::SUCCESS;
            }
            match import_subjects(&mut store, &drafts, ctx::now()) {
                Ok(report) => {
                    let line = |label: &str, names: &[String]| {
                        if !names.is_empty() {
                            println!("{label}: {}", names.join(", "));
                        }
                    };
                    line("Добавлены", &report.added);
                    line("Обновлены", &report.updated);
                    line("Без изменений", &report.unchanged);
                    ExitCode::SUCCESS
                }
                Err(e) => fail(e.to_string()),
            }
        }
        _ => {
            eprintln!(
                "usage: cognitive-popups subjects import <file> [--dry-run] | subjects export"
            );
            ExitCode::from(2)
        }
    }
}

/// What a hotkey runs. With no daemon the reader gets a system notification
/// instead of silence (Q-74).
fn signal(window: Option<&str>, argument: Option<&str>) -> ExitCode {
    let Some(window) = window.filter(|w| WINDOWS.contains(w)) else {
        eprintln!("unknown window; one of: {}", WINDOWS.join(", "));
        return ExitCode::from(2);
    };
    let sent = UnixStream::connect(socket()).and_then(|mut s| {
        let line = match argument {
            Some(argument) => format!("show {window} {argument}\n"),
            None => format!("show {window}\n"),
        };
        s.write_all(line.as_bytes())
    });
    if sent.is_err() {
        let _ = Command::new("notify-send")
            .args([
                "-h",
                "boolean:suppress-sound:true",
                "Cognitive Popups не запущен",
                "Запусти: cognitive-popups daemon (или exec-once в hyprland.conf)",
            ])
            .status();
        eprintln!("daemon not running");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn hyprland_config() -> String {
    "# Cognitive Popups — classic hyprland.conf syntax.\n\
     # For Hyprland Lua use: cognitive-popups hyprland-config --lua\n\
     exec-once = cognitive-popups daemon\n\
     bind = ALT, E, exec, cognitive-popups signal note\n\
     bind = ALT, W, exec, cognitive-popups signal understand\n\
     bind = CTRL ALT, P, exec, cognitive-popups signal hypothesis\n\
     bind = ALT, T, exec, cognitive-popups signal practice\n\
     bind = ALT, I, exec, cognitive-popups signal lesson\n\
     bind = ALT, B, exec, cognitive-popups signal resume\n\
     bind = ALT, S, exec, cognitive-popups signal hub\n\
     bind = ALT, K, exec, cognitive-popups signal help\n\
     layerrule = blur, cognitive-.*\n\
     layerrule = ignorezero, cognitive-.*\n"
        .to_string()
}

fn hyprland_config_lua() -> &'static str {
    r#"-- Cognitive Popups — Hyprland 0.56 Lua syntax.
hl.on("hyprland.start", function()
  hl.exec_cmd("cognitive-popups daemon")
end)
hl.bind("ALT + E", hl.dsp.exec_cmd("cognitive-popups signal note"))
hl.bind("ALT + W", hl.dsp.exec_cmd("cognitive-popups signal understand"))
hl.bind("CTRL + ALT + P", hl.dsp.exec_cmd("cognitive-popups signal hypothesis"))
hl.bind("ALT + T", hl.dsp.exec_cmd("cognitive-popups signal practice"))
hl.bind("ALT + I", hl.dsp.exec_cmd("cognitive-popups signal lesson"))
hl.bind("ALT + B", hl.dsp.exec_cmd("cognitive-popups signal resume"))
hl.bind("ALT + S", hl.dsp.exec_cmd("cognitive-popups signal hub"))
hl.bind("ALT + K", hl.dsp.exec_cmd("cognitive-popups signal help"))
hl.layer_rule({ match = { namespace = "cognitive-.*" }, blur = true, ignore_alpha = 0.0 })
"#
}

fn daemon() -> ExitCode {
    let app = adw::Application::builder()
        .application_id("dev.cognitive.Popups")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(|app| {
        theme::install();
        scale::install();
        let ctx = ctx::Ctx::load(database());
        let note = note::NoteWindow::new(app, ctx.db.clone());
        let understand = understand::UnderstandWindow::new(app, ctx.clone());
        let hypothesis = hypothesis::HypothesisWindow::new(app, ctx.clone());
        let links = practice::Links {
            ladder: {
                let understand = understand.clone();
                Box::new(move |target: String| understand.ladder(target))
            },
            hypothesis: {
                let hypothesis = hypothesis.clone();
                Box::new(move || hypothesis.open(grab_text()))
            },
        };
        let practice = practice::PracticeWindow::new(app, ctx.clone(), links);
        let notice = notice::Notice::new(app);
        let pill = pill::Pill::new(app, ctx.clone(), {
            let practice = practice.clone();
            move || practice.open(None, None, true)
        });
        pill.start();
        let lesson = lesson::LessonUi::new(app, ctx.clone(), notice.clone(), pill.clone());
        lesson.restore();
        let material_watch = watch::MaterialWatch::start(ctx.clone(), notice.clone());
        app.connect_shutdown(move |_| material_watch.stop());
        let hub = hub::Hub::new(app, ctx.clone(), {
            let pill = pill.clone();
            move || pill.refresh()
        });
        tray::spawn(socket());
        let resume = resume::Resume::new(app, ctx.clone());
        let help = help::Help::new(app, {
            let hub = hub.clone();
            move || hub.open(None)
        });
        let nudge = nudge::start(
            app,
            ctx.db.clone(),
            {
                let lesson = lesson.clone();
                move || lesson.press(None)
            },
            {
                let practice = practice.clone();
                move || practice.open(None, None, true)
            },
        );
        let seed_db = ctx.db.clone();
        listen(move |line| {
            let (window, argument) = line.split_once(' ').unwrap_or((line, ""));
            // The selection only: instant, and never a Ctrl+C into our own window.
            let selection = || capture::selection(&Wayland).ok();
            // The selection, else Ctrl+C into the window the reader is in (A4, B-capture):
            // taken before our window opens. Primary selection can be off in the
            // compositor (`misc:middle_click_paste = false`), then only Ctrl+C reads.
            let grab = || grab_text();
            match window {
                "note" => note.open(selection()),
                // `signal understand example`: that help, not a weighted pick.
                // `signal understand own [words…]`: [Сначала сам] at once; the
                // words, if given, are compared (screenshots without a keyboard).
                "understand" if argument.starts_with("own") => {
                    understand.open(grab(), Some(Mechanic::Words));
                    understand.write_own();
                    let mine = understand_core::own_words(&argument["own".len()..]);
                    if !mine.is_empty() {
                        understand.compare(mine);
                    }
                }
                "understand" => understand.open(grab(), Mechanic::parse(argument)),
                // `signal hide`: every window away (screenshots, scripts).
                // `signal hypothesis [guess…]`: a given guess is checked at once.
                "hypothesis" => {
                    hypothesis.open(grab());
                    if !argument.is_empty() {
                        hypothesis.submit_text(argument);
                    }
                }
                // `signal practice [probes|methods|feynman|continue|old|skill|pill|whole]`;
                // `continue reveal` also opens the solution of the task on screen.
                "practice" => {
                    let (word, rest) = argument.split_once(' ').unwrap_or((argument, ""));
                    let route = match word {
                        "probes" | "whole" => Some(Route::Probes),
                        "methods" => Some(Route::ChooseMethod),
                        "feynman" => Some(Route::Feynman),
                        "continue" => Some(Route::ContinueTask),
                        "old" => Some(Route::OldMaterial),
                        "skill" => Some(Route::Skill),
                        _ => None,
                    };
                    practice.open(grab(), route, word == "pill");
                    if word == "whole" {
                        practice.whole_demo();
                    }
                    if rest == "reveal" {
                        practice.reveal_solution();
                    }
                }
                "lesson" if argument.is_empty() => lesson.press(selection()),
                "lesson" if argument == "feynman" => lesson.preview_feynman(),
                "lesson" => lesson.script(argument),
                "pill" => pill.show_count(argument.parse().unwrap_or(0)),
                // `signal notice <text>`: a quiet notice from a script.
                "notice" => notice.show("Cognitive Popups", argument),
                "nudge" if argument == "daily" => {
                    nudge.preview(cognitive_exoskeleton_nudges::Kind::Daily)
                }
                "nudge" if argument == "opportunity" => {
                    nudge.preview(cognitive_exoskeleton_nudges::Kind::Opportunity)
                }
                "hub" => hub.open(Some(argument).filter(|a| !a.is_empty())),
                "resume" => resume.open(),
                "help" => help.open(),
                "seed" if seed::allowed() => {
                    if let Err(e) = seed::demo(&seed_db, ctx::now()) {
                        eprintln!("seed: {e}");
                    }
                }
                "hide" => {
                    note.hide();
                    understand.hide();
                    hypothesis.hide();
                    practice.hide();
                    lesson.hide();
                    notice.hide();
                    nudge.hide();
                    pill.hide();
                    hub.hide();
                    resume.hide();
                    help.hide();
                }
                _ => {}
            }
        });
        println!("ready");
    });
    let _hold = app.hold();
    let status = app.run_with_args::<&str>(&[]);
    status.into()
}

/// The socket thread hands each `show <window>` to the GTK main loop.
fn listen(mut on_show: impl FnMut(&str) + 'static) {
    let path = socket();
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("cannot listen on {}: {e}", path.display());
            std::process::exit(1);
        }
    };
    let (sender, receiver) = async_channel_pair();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            if BufReader::new(stream).read_line(&mut line).is_ok()
                && let Some(window) = line.trim().strip_prefix("show ")
            {
                let _ = sender.send(window.to_string());
            }
        }
    });
    glib::spawn_future_local(async move {
        while let Ok(window) = receiver.recv().await {
            on_show(&window);
        }
    });
}

/// A thread → main loop channel without an extra dependency.
fn async_channel_pair() -> (Sender, Receiver) {
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let waker: std::sync::Arc<std::sync::Mutex<Option<std::task::Waker>>> = Default::default();
    (
        Sender {
            tx,
            waker: waker.clone(),
        },
        Receiver { rx, waker },
    )
}

struct Sender {
    tx: std::sync::mpsc::Sender<String>,
    waker: std::sync::Arc<std::sync::Mutex<Option<std::task::Waker>>>,
}

impl Sender {
    fn send(&self, value: String) -> Result<(), ()> {
        self.tx.send(value).map_err(|_| ())?;
        if let Some(waker) = self.waker.lock().map_err(|_| ())?.take() {
            waker.wake();
        }
        Ok(())
    }
}

struct Receiver {
    rx: std::sync::mpsc::Receiver<String>,
    waker: std::sync::Arc<std::sync::Mutex<Option<std::task::Waker>>>,
}

impl Receiver {
    async fn recv(&self) -> Result<String, ()> {
        std::future::poll_fn(|cx| match self.rx.try_recv() {
            Ok(value) => std::task::Poll::Ready(Ok(value)),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => std::task::Poll::Ready(Err(())),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                if let Ok(mut slot) = self.waker.lock() {
                    *slot = Some(cx.waker().clone());
                }
                // A value may have arrived between try_recv and storing the waker.
                match self.rx.try_recv() {
                    Ok(value) => std::task::Poll::Ready(Ok(value)),
                    Err(_) => std::task::Poll::Pending,
                }
            }
        })
        .await
    }
}

/// The reader's selection; if there is none, a Ctrl+C into their window. Never
/// into our own windows: the hub is a normal window and would swallow it.
fn grab_text() -> Option<capture::Captured> {
    if let Ok(selected) = capture::selection(&Wayland) {
        return Some(selected);
    }
    use capture::Desktop;
    let ours = Wayland
        .active_window()
        .is_some_and(|w| w.class.starts_with("dev.cognitive") || w.class.starts_with("cognitive-"));
    if ours {
        return None;
    }
    capture::capture(&Wayland, false).ok()
}

fn core_cli(args: &[String]) -> ExitCode {
    let binary = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.join("cognitive-popups-core")));
    let Some(binary) = binary else {
        return ExitCode::FAILURE;
    };
    let mut command = std::process::Command::new(binary);
    command.args(args);
    if !args.iter().any(|a| a == "--db") {
        command.arg("--db").arg(database());
    }
    let needs_prompts = matches!(
        (
            args.first().map(String::as_str),
            args.get(1).map(String::as_str)
        ),
        (Some("material"), Some("register"))
            | (Some("experiment"), Some("cards"))
            | (Some("lesson"), Some("plan"))
            | (Some("practice"), Some("tree"))
    );
    if needs_prompts && !args.iter().any(|a| a == "--prompts") {
        command.arg("--prompts").arg(ctx::prompts_dir());
    }
    match command.status() {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("core CLI: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;

    #[test]
    fn both_configs_cover_the_same_hotkey_actions() {
        let classic = hyprland_config();
        let lua = hyprland_config_lua();
        for action in [
            "note",
            "understand",
            "hypothesis",
            "practice",
            "lesson",
            "resume",
            "help",
        ] {
            let command = format!("cognitive-popups signal {action}");
            assert!(classic.contains(&command));
            assert!(lua.contains(&command));
        }
        assert!(lua.contains("hl.on(\"hyprland.start\""));
        assert!(lua.contains("hl.layer_rule("));
    }
}
