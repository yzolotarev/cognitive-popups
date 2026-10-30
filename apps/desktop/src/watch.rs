//! Registering the full study text without buttons (B78 step 1b). With the
//! consent «Учебный текст из буфера» and only while an Alt+I lesson is open,
//! two `wl-paste --watch` children report that the clipboard (Ctrl+C) or the
//! mouse selection (primary) changed. Only the fact of a change crosses the
//! pipe — one empty line per change — so the protocol needs no delimiter:
//! the content stays in `wl-paste`. Once a selection has stopped changing
//! for a second (`watch::Settle`), it is read the way Alt+W reads it; a text
//! that is not a full marked study text (`watch::is_full_text`) is dropped
//! at once: never stored, logged or journaled. A full one is registered for
//! the lesson (idempotent: the same text again shows nothing) and a quiet
//! notice says so (INV-13: it does not take the keyboard).
//!
//! The children run inside `sh`, which waits on our stdin: when the lesson
//! closes, the consent goes off or the daemon exits (even killed), the pipe
//! closes and the shell kills its `wl-paste`. Nothing is left running.

use std::cell::{Cell, RefCell};
use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cognitive_exoskeleton_capture::{Desktop, Wayland};
use cognitive_exoskeleton_lessons::LessonStore;
use cognitive_exoskeleton_materials::Materials;
use cognitive_exoskeleton_materials::watch::{Settle, is_full_text};
use cognitive_exoskeleton_nudges::{Consent, Nudges};
use gtk::{gio, glib};

use crate::ctx::{self, Ctx};
use crate::notice::Notice;

/// `wl-paste [--primary] --watch …` that dies with our stdin. Each change
/// prints one empty line; the content itself is swallowed by `cat`.
const SCRIPT: &str = r#"wl-paste "$@" --watch sh -c 'cat >/dev/null; echo' & w=$!
cat >/dev/null
kill "$w" 2>/dev/null"#;

const TICK: Duration = Duration::from_millis(250);
/// The consent is re-read every this many ticks (3 s).
const CONSENT_TICKS: u32 = 12;

#[derive(Clone, Copy, Debug)]
enum Selection {
    Clipboard,
    Primary,
}

struct Watcher {
    selection: Selection,
    child: Child,
    stdin: Option<ChildStdin>,
    settle: Arc<Mutex<Settle>>,
}

impl Watcher {
    fn spawn(selection: Selection, epoch: Instant) -> Option<Self> {
        let flags: &[&str] = match selection {
            Selection::Clipboard => &[],
            Selection::Primary => &["--primary"],
        };
        let mut child = Command::new("sh")
            .args(["-c", SCRIPT, "sh"])
            .args(flags)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| eprintln!("material watch: {e}"))
            .ok()?;
        let settle = Arc::new(Mutex::new(Settle::new(millis(epoch))));
        let stdout = child.stdout.take()?;
        let changes = settle.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if line.is_err() {
                    break;
                }
                if let Ok(mut settle) = changes.lock() {
                    settle.change(millis(epoch));
                }
            }
        });
        Some(Self {
            selection,
            stdin: child.stdin.take(),
            child,
            settle,
        })
    }

    /// Closing stdin makes the shell kill its `wl-paste`; the wait is off the
    /// GTK thread.
    fn stop(mut self) {
        drop(self.stdin.take());
        std::thread::spawn(move || {
            let _ = self.child.wait();
        });
    }
}

fn millis(epoch: Instant) -> i64 {
    epoch.elapsed().as_millis() as i64
}

#[derive(Clone)]
pub struct MaterialWatch(Rc<Inner>);

struct Inner {
    ctx: Ctx,
    notice: Notice,
    watchers: RefCell<Vec<Watcher>>,
    consent: Cell<bool>,
    ticks: Cell<u32>,
    epoch: Instant,
}

impl MaterialWatch {
    pub fn start(ctx: Ctx, notice: Notice) -> Self {
        let watch = Self(Rc::new(Inner {
            ctx,
            notice,
            watchers: RefCell::default(),
            consent: Cell::new(false),
            ticks: Cell::new(0),
            epoch: Instant::now(),
        }));
        let this = watch.clone();
        glib::timeout_add_local(TICK, move || {
            this.tick();
            glib::ControlFlow::Continue
        });
        watch
    }

    fn tick(&self) {
        let inner = &self.0;
        let tick = inner.ticks.get();
        inner.ticks.set(tick.wrapping_add(1));
        if tick.is_multiple_of(CONSENT_TICKS) {
            inner.consent.set(
                Nudges::open(&inner.ctx.db)
                    .and_then(|n| n.consented(Consent::StudyClipboard))
                    .unwrap_or(false),
            );
        }
        let lesson = inner.ctx.lesson.borrow().clone();
        let Some(lesson) = lesson.filter(|_| inner.consent.get()) else {
            self.stop();
            return;
        };
        if inner.watchers.borrow().is_empty() {
            let started: Vec<Watcher> = [Selection::Clipboard, Selection::Primary]
                .into_iter()
                .filter_map(|s| Watcher::spawn(s, inner.epoch))
                .collect();
            *inner.watchers.borrow_mut() = started;
        }
        let now = millis(inner.epoch);
        let due: Vec<Selection> = inner
            .watchers
            .borrow()
            .iter()
            .filter(|w| w.settle.lock().is_ok_and(|mut s| s.due(now)))
            .map(|w| w.selection)
            .collect();
        for selection in due {
            self.read(selection, lesson.as_str().to_string());
        }
    }

    /// Off the GTK thread: read, drop unless a full text, register.
    fn read(&self, selection: Selection, lesson: String) {
        let db = self.0.ctx.db.clone();
        let notice = self.0.notice.clone();
        let ai = self.0.ctx.ai();
        let prompts = self.0.ctx.prompts.clone();
        let feed = self.0.ctx.precompute_feed.get();
        let handle = gio::spawn_blocking(move || {
            let text = match selection {
                Selection::Primary => Wayland.primary_text(),
                Selection::Clipboard => Wayland
                    .clipboard_read("text")
                    .and_then(|b| String::from_utf8(b).ok()),
            }?;
            if !is_full_text(&text) {
                return None;
            }
            if !Nudges::open(&db)
                .and_then(|n| n.consented(Consent::StudyClipboard))
                .unwrap_or(false)
            {
                return None;
            }
            let subject = LessonStore::open(&db)
                .ok()
                .and_then(|s| s.lesson(&lesson).ok().flatten())
                .map(|l| l.subject);
            let registered = Materials::open(&db)
                .and_then(|mut m| m.register(Some(&lesson), subject.as_deref(), &text, ctx::now()));
            match registered {
                Ok((material, new)) => {
                    if let Ok(m) = Materials::open(&db)
                        && let Err(e) = m.generate_cards(&db, &ai, &prompts)
                    {
                        eprintln!("block cards: {e}");
                    }
                    if feed {
                        prepare_feed(&db, &ai, &prompts, &material);
                    }
                    new.then(|| material.registered_notice())
                }
                Err(e) => {
                    eprintln!("material watch: {e}");
                    None
                }
            }
        });
        glib::spawn_future_local(async move {
            if let Ok(Some(text)) = handle.await {
                notice.show("Учебный текст", &text);
            }
        });
    }

    /// No lesson, no consent or the daemon is leaving: nothing watches.
    pub fn stop(&self) {
        for watcher in self.0.watchers.borrow_mut().drain(..) {
            watcher.stop();
        }
    }
}

/// B90: the probe tree for a registered text, built once and kept, so Alt+T
/// opens «Лента» without waiting. Runs off the GTK thread.
fn prepare_feed(
    db: &std::path::Path,
    ai: &ctx::Ai,
    prompts: &cognitive_exoskeleton_ai::PromptStore,
    material: &cognitive_exoskeleton_materials::Material,
) {
    use cognitive_exoskeleton_practice::{self as practice, feed::FeedStore};
    let Ok(store) = FeedStore::open(db) else {
        return;
    };
    if store.tree(&material.id).ok().flatten().is_some() {
        return;
    }
    match practice::probe_tree_for(
        ai,
        prompts,
        &material.text,
        crate::practice::lesson_focus(db).as_deref(),
        cognitive_exoskeleton_ai::Priority::Background,
    ) {
        Ok(tree) if !tree.groups.is_empty() => {
            if let Err(e) = store.save_tree(&material.id, &tree, ctx::now()) {
                eprintln!("feed: {e}");
            }
        }
        Ok(_) => {}
        Err(e) => eprintln!("feed: prepare: {e}"),
    }
}
