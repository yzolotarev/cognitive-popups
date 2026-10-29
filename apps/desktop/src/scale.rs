//! Per-window keyboard text scaling (F-99).
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::cell::Cell;
use std::rc::Rc;

const LEVELS: [u16; 12] = [85, 100, 110, 120, 130, 140, 150, 160, 170, 180, 190, 200];

fn next_level(old: usize, key: gdk::Key) -> Option<usize> {
    match key {
        gdk::Key::equal | gdk::Key::plus | gdk::Key::KP_Add => {
            Some((old + 1).min(LEVELS.len() - 1))
        }
        gdk::Key::minus | gdk::Key::KP_Subtract => Some(old.saturating_sub(1)),
        gdk::Key::_0 | gdk::Key::KP_0 => Some(1),
        _ => None,
    }
}

pub fn factor(widget: &impl IsA<gtk::Widget>) -> f64 {
    let Some(root) = widget.as_ref().root() else {
        return 1.0;
    };
    let Some(window) = root.downcast_ref::<gtk::Window>() else {
        return 1.0;
    };
    LEVELS
        .iter()
        .find(|level| window.has_css_class(&format!("cp-zoom-{level}")))
        .map_or(1.0, |level| f64::from(*level) / 100.0)
}

pub fn install() {
    let mut css = String::new();
    for level in LEVELS {
        if level == 100 {
            continue;
        }
        let size = |base: f64| base * f64::from(level) / 100.0;
        let root = format!("window.cp-zoom-{level}");
        css.push_str(&format!(
            "{root}, {root} * {{ font-size: {:.2}px; }}\n",
            size(13.0)
        ));
        for (selector, base) in [
            (".cp-title", 17.0),
            (".cp-headline", 13.0),
            (".cp-callout", 12.0),
            (".cp-footnote", 10.0),
            (".cp-quote", 12.0),
            (".cp-body", 13.0),
            (".cp-badge", 10.0),
            ("button.cp-choice", 12.0),
        ] {
            css.push_str(&format!(
                "{root} {selector} {{ font-size: {:.2}px; }}\n",
                size(base)
            ));
        }
    }
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&css);
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("display"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 2,
    );
}

pub fn attach(window: &impl IsA<gtk::Window>) {
    let window = window.as_ref();
    let initial = std::env::var("COGNITIVE_TEXT_SCALE")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .and_then(|scale| LEVELS.iter().position(|&level| level == scale))
        .unwrap_or(1);
    let level = Rc::new(Cell::new(initial));
    window.add_css_class(&format!("cp-zoom-{}", LEVELS[initial]));
    let controller = gtk::EventControllerKey::new();
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = window.downgrade();
    controller.connect_key_pressed(move |_, key, _, modifiers| {
        if !modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
            return glib::Propagation::Proceed;
        }
        let old = level.get();
        let Some(new) = next_level(old, key) else {
            return glib::Propagation::Proceed;
        };
        if let Some(window) = weak.upgrade() {
            window.remove_css_class(&format!("cp-zoom-{}", LEVELS[old]));
            window.add_css_class(&format!("cp-zoom-{}", LEVELS[new]));
            level.set(new);
        }
        glib::Propagation::Stop
    });
    window.add_controller(controller);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shortcuts_clamp_and_reset_the_scale() {
        assert_eq!(next_level(0, gdk::Key::minus), Some(0));
        assert_eq!(next_level(11, gdk::Key::equal), Some(11));
        assert_eq!(next_level(11, gdk::Key::_0), Some(1));
        assert_eq!(next_level(1, gdk::Key::plus), Some(2));
        assert_eq!(next_level(1, gdk::Key::a), None);
    }
}
