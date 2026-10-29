//! Look of every window (DESIGN.md): one style sheet, light and dark colours,
//! following the system theme. `COGNITIVE_THEME=light|dark` overrides it, for
//! screenshots.

use gtk::gdk;

pub const STYLE: &str = include_str!("../assets/style.css");
pub const LIGHT: &str = include_str!("../assets/colors-light.css");
pub const DARK: &str = include_str!("../assets/colors-dark.css");

/// Loads the colours for the current theme and keeps them in step with it.
pub fn install() {
    let display = gdk::Display::default().expect("a display");
    let style = gtk::CssProvider::new();
    style.load_from_string(STYLE);
    gtk::style_context_add_provider_for_display(
        &display,
        &style,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    let colours = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(
        &display,
        &colours,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
    );
    let manager = adw::StyleManager::default();
    let apply = move |dark: bool| colours.load_from_string(if dark { DARK } else { LIGHT });
    match std::env::var("COGNITIVE_THEME").as_deref() {
        Ok("dark") => {
            manager.set_color_scheme(adw::ColorScheme::ForceDark);
            apply(true);
        }
        Ok("light") => {
            manager.set_color_scheme(adw::ColorScheme::ForceLight);
            apply(false);
        }
        _ => {
            apply(manager.is_dark());
            manager.connect_dark_notify(move |m| apply(m.is_dark()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `#rrggbb` or `rgba(r,g,b,a)` → (r, g, b, a) in 0..=1.
    fn parse(value: &str) -> Option<(f64, f64, f64, f64)> {
        let value = value.trim();
        if let Some(hex) = value.strip_prefix('#') {
            let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
            return Some((
                f64::from(byte(0)?) / 255.0,
                f64::from(byte(2)?) / 255.0,
                f64::from(byte(4)?) / 255.0,
                1.0,
            ));
        }
        let inner = value.strip_prefix("rgba(")?.strip_suffix(')')?;
        let parts: Vec<f64> = inner
            .split(',')
            .map(|p| p.trim().parse().ok())
            .collect::<Option<_>>()?;
        let [r, g, b, a] = parts[..] else { return None };
        Some((r / 255.0, g / 255.0, b / 255.0, a))
    }

    /// The named colours of one theme file.
    fn colours(css: &str) -> Vec<(String, (f64, f64, f64, f64))> {
        css.lines()
            .filter_map(|line| {
                let rest = line.trim().strip_prefix("@define-color ")?;
                let (name, value) = rest.split_once(' ')?;
                Some((name.to_string(), parse(value.trim_end_matches(';'))?))
            })
            .collect()
    }

    /// WCAG contrast of `fg` drawn over an opaque `bg` (alpha is composited).
    fn contrast(fg: (f64, f64, f64, f64), bg: (f64, f64, f64, f64)) -> f64 {
        let over = |f: f64, b: f64| f * fg.3 + b * (1.0 - fg.3);
        let fg = (over(fg.0, bg.0), over(fg.1, bg.1), over(fg.2, bg.2));
        let lum = |(r, g, b): (f64, f64, f64)| {
            let ch = |c: f64| {
                if c <= 0.039_28 {
                    c / 12.92
                } else {
                    ((c + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b)
        };
        let (a, b) = (lum(fg), lum((bg.0, bg.1, bg.2)));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// DESIGN.md §3: text ≥ 4.5:1 on what it is drawn on, in both themes.
    #[test]
    fn every_text_colour_passes_contrast_in_both_themes() {
        let pairs = [
            ("cp_text", "cp_surface"),
            ("cp_text_secondary", "cp_surface"),
            ("cp_on_accent", "cp_accent"),
            ("cp_accent_text", "cp_surface"),
            ("cp_success", "cp_surface"),
            ("cp_error", "cp_surface"),
        ];
        for (theme, css) in [("light", LIGHT), ("dark", DARK)] {
            let named = colours(css);
            let get = |n: &str| {
                named
                    .iter()
                    .find(|(name, _)| name == n)
                    .unwrap_or_else(|| panic!("{theme}: no {n}"))
                    .1
            };
            for (fg, bg) in pairs {
                let ratio = contrast(get(fg), get(bg));
                assert!(ratio >= 4.5, "{theme}: {fg} on {bg} = {ratio:.2}");
            }
        }
    }

    #[test]
    fn both_themes_define_the_same_names() {
        let names = |css| {
            let mut n: Vec<String> = colours(css).into_iter().map(|(n, _)| n).collect();
            n.sort();
            n
        };
        assert_eq!(names(LIGHT), names(DARK));
    }

    /// DESIGN.md §10: colours live only in the theme files, sizes only in
    /// style.css — no literals in the window code.
    #[test]
    fn window_code_has_no_colour_literals() {
        for (file, source) in [
            ("note.rs", include_str!("note.rs")),
            ("popup.rs", include_str!("popup.rs")),
            ("understand.rs", include_str!("understand.rs")),
            ("ui.rs", include_str!("ui.rs")),
            ("hypothesis.rs", include_str!("hypothesis.rs")),
            ("practice.rs", include_str!("practice.rs")),
            ("lesson.rs", include_str!("lesson.rs")),
            ("notice.rs", include_str!("notice.rs")),
        ] {
            for bad in ["#", "rgb(", "rgba("] {
                let hit = source
                    .lines()
                    .filter(|l| !l.trim_start().starts_with("//"))
                    .any(|l| l.contains(&format!("\"{bad}")));
                assert!(!hit, "{file} contains a colour literal {bad}");
            }
        }
    }
}
