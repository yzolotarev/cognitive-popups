//! «FSRS! N» (S4): a quiet pill in the bottom-right corner when reviews are
//! due. No sound, no keyboard (INV-13), only with consent (INV-09), and it can
//! be dragged out of the way. A click opens Alt+T with «Достань из памяти»
//! first (B31).

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use cognitive_exoskeleton_memory::Memory;
use cognitive_exoskeleton_nudges::{Consent, Nudges};
use cognitive_exoskeleton_practice::due::sync_memory;
use cognitive_exoskeleton_units::UnitStore;
use gtk::glib;
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, LayerShell};

use crate::ctx::{self, Ctx};
use crate::popup::{self, Opened};
use crate::ui;

/// How often the count is refreshed.
const EVERY: Duration = Duration::from_secs(60);
const MARGIN: i32 = 16;

#[derive(Clone)]
pub struct Pill {
    window: gtk::ApplicationWindow,
    label: gtk::Label,
    ctx: Ctx,
}

impl Pill {
    pub fn new(app: &adw::Application, ctx: Ctx, open: impl Fn() + 'static) -> Self {
        let window = popup::corner(app, "cognitive-pill", -1, 0);
        window.set_anchor(Edge::Top, false);
        window.set_anchor(Edge::Bottom, true);
        window.set_margin(Edge::Bottom, MARGIN);
        window.set_margin(Edge::Right, MARGIN);
        window.add_css_class("cp-pill");
        let label = ui::text("", &["cp-callout"]);
        label.set_margin_start(12);
        label.set_margin_end(12);
        label.set_margin_top(6);
        label.set_margin_bottom(6);
        label.set_tooltip_text(Some("Подошли повторения — нажми, чтобы вспомнить"));
        window.set_child(Some(&label));

        // Drag moves the pill; a press without a drag opens the reviews.
        let drag = gtk::GestureDrag::new();
        let start = Rc::new(Cell::new((MARGIN, MARGIN)));
        let moved = Rc::new(Cell::new(false));
        let (w, s, m) = (window.clone(), start.clone(), moved.clone());
        drag.connect_drag_begin(move |_, _, _| {
            s.set((w.margin(Edge::Right), w.margin(Edge::Bottom)));
            m.set(false);
        });
        let (w, s, m) = (window.clone(), start, moved.clone());
        drag.connect_drag_update(move |_, dx, dy| {
            if dx.abs() + dy.abs() > 4.0 {
                m.set(true);
            }
            let (right, bottom) = s.get();
            w.set_margin(Edge::Right, (right - dx as i32).max(0));
            w.set_margin(Edge::Bottom, (bottom - dy as i32).max(0));
        });
        drag.connect_drag_end(move |_, _, _| {
            if !moved.get() {
                open();
            }
        });
        window.add_controller(drag);
        Self { window, label, ctx }
    }

    /// Checks the count now and then every minute.
    pub fn start(&self) {
        self.refresh();
        let this = self.clone();
        glib::timeout_add_local(EVERY, move || {
            this.refresh();
            glib::ControlFlow::Continue
        });
    }

    pub fn refresh(&self) {
        let db = self.ctx.db.clone();
        let this = self.clone();
        ctx::background(
            move || -> usize {
                let allowed = Nudges::open(&db)
                    .and_then(|n| n.consented(Consent::Reviews))
                    .unwrap_or(false);
                if !allowed {
                    return 0;
                }
                let (Ok(units), Ok(mut memory)) = (UnitStore::open(&db), Memory::open(&db)) else {
                    return 0;
                };
                let now = ctx::now();
                let _ = sync_memory(&units, &mut memory, now);
                memory.due(now).map(|d| d.len()).unwrap_or(0)
            },
            move |due| this.set(due),
        );
    }

    /// `signal pill <N>`: the pill as it looks with N due (screenshots).
    pub fn show_count(&self, due: usize) {
        self.set(due);
    }

    fn set(&self, due: usize) {
        if due == 0 {
            popup::hide(&self.window);
            return;
        }
        self.label.set_text(&format!("FSRS! {due}"));
        if !self.window.is_visible() {
            popup::show_corner(&self.window, Opened::ByItself);
        }
    }

    pub fn hide(&self) {
        popup::hide(&self.window);
    }
}
