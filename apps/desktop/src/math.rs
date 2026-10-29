//! Formulas drawn like in a textbook (F-62, B53): the strict parser of
//! `crates/formula` gives Typst math, Typst (built in, fonts included, no
//! network) draws it. A formula that does not parse or does not compile is
//! shown as readable Unicode text instead — never as LaTeX source.

use std::cell::RefCell;
use std::collections::HashMap;

use cognitive_exoskeleton_formula as formula;
use gtk::prelude::*;

use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime};
use typst::layout::{Frame, FrameItem, PagedDocument};
use typst::syntax::{FileId, Source, VirtualPath};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, World};

/// Typst points per logical pixel at 96 dpi: 10 pt ≈ 13 px, the body size.
const PX_PER_PT: f32 = 96.0 / 72.0;
/// Drawn at twice the size for HiDPI screens, shown at half.
const OVERSAMPLE: f32 = 2.0;
const SIZE_PT: f32 = 11.0;

struct Fonts {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
}

thread_local! {
    static FONTS: Fonts = {
        let fonts: Vec<Font> = typst_assets::fonts()
            .flat_map(|data| Font::iter(Bytes::new(data)))
            .collect();
        Fonts {
            library: LazyHash::new(Library::default()),
            book: LazyHash::new(FontBook::from_fonts(&fonts)),
            fonts,
        }
    };
    static CACHE: RefCell<HashMap<(String, bool, bool), Option<Drawn>>> = RefCell::default();
}

/// A drawn formula: premultiplied BGRA for cairo, and its size in logical
/// pixels (the image itself is `OVERSAMPLE` times larger).
#[derive(Clone)]
pub struct Drawn {
    pixels: std::rc::Rc<Vec<u8>>,
    raster: (i32, i32),
    pub width: i32,
    pub height: i32,
    /// Distance from the top of the image to the text baseline of the
    /// formula, in logical pixels.
    pub baseline: i32,
}

struct Page<'a> {
    fonts: &'a Fonts,
    main: Source,
}

impl World for Page<'_> {
    fn library(&self) -> &LazyHash<Library> {
        &self.fonts.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.fonts.book
    }

    fn main(&self) -> FileId {
        self.main.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main.id() {
            Ok(self.main.clone())
        } else {
            Err(FileError::AccessDenied)
        }
    }

    fn file(&self, _: FileId) -> FileResult<Bytes> {
        Err(FileError::AccessDenied)
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.fonts.get(index).cloned()
    }

    fn today(&self, _: Option<i64>) -> Option<Datetime> {
        None
    }
}

/// The Typst document for one formula in the text colour of the theme.
pub fn document(math: &str, display: bool, dark: bool) -> String {
    let colour = if dark { "#ebebeb" } else { "#1f1f1f" };
    let body = if display {
        format!("$ {math} $")
    } else {
        // The probe is invisible and 0.1 pt wide: it only tells where the
        // text baseline of the line is (see `probe_baseline`).
        format!("#text(size: {PROBE_PT}pt, fill: rgb(\"#00000000\"))[x]${math}$")
    };
    format!(
        "#set page(width: auto, height: auto, margin: (x: 1pt, top: 2pt, bottom: 4pt), fill: none)\n\
         #set text(size: {SIZE_PT}pt, fill: rgb(\"{colour}\"))\n{body}\n"
    )
}

/// Size of the invisible probe glyph placed before an inline formula: its
/// text item sits exactly on the baseline of the line.
const PROBE_PT: f64 = 0.1;

/// Baseline of the probe glyph from the frame's top, in points.
fn probe_baseline(frame: &Frame) -> Option<f64> {
    frame.items().find_map(|(pos, item)| match item {
        FrameItem::Text(text) if (text.size.to_pt() - PROBE_PT).abs() < 1e-6 => Some(pos.y.to_pt()),
        FrameItem::Group(group) => {
            probe_baseline(&group.frame).map(|b| pos.y.to_pt() + group.transform.ty.to_pt() + b)
        }
        _ => None,
    })
}

/// Compiles and rasterizes; `None` when Typst refuses the source. The last
/// value is the baseline from the top of the image, in image pixels.
pub fn rasterize(source: &str) -> Option<(Vec<u8>, u32, u32, f32)> {
    FONTS.with(|fonts| {
        let main = Source::new(
            FileId::new(None, VirtualPath::new("/formula.typ")),
            source.into(),
        );
        let page = Page { fonts, main };
        let document = typst::compile::<PagedDocument>(&page).output.ok()?;
        let first = document.pages.first()?;
        let pixmap = typst_render::render(first, PX_PER_PT * OVERSAMPLE);
        let baseline = probe_baseline(&first.frame).map_or(pixmap.height() as f32 / 2.0, |pt| {
            pt as f32 * PX_PER_PT * OVERSAMPLE
        });
        Some((
            pixmap.data().to_vec(),
            pixmap.width(),
            pixmap.height(),
            baseline,
        ))
    })
}

/// The formula drawn, cached by source and theme; `None` → show Unicode.
pub fn draw(tex: &str, display: bool) -> Option<Drawn> {
    let dark = adw::StyleManager::default().is_dark();
    let key = (tex.to_string(), display, dark);
    if let Some(found) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return found;
    }
    let drawn = formula::parse(tex)
        .ok()
        .and_then(|nodes| rasterize(&document(&formula::to_typst(&nodes), display, dark)))
        .map(|(mut data, width, height, baseline)| {
            // tiny-skia gives RGBA; cairo's ARGB32 is BGRA in memory.
            for pixel in data.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
            Drawn {
                pixels: std::rc::Rc::new(data),
                raster: (width as i32, height as i32),
                width: (width as f32 / OVERSAMPLE).round() as i32,
                height: (height as f32 / OVERSAMPLE).round() as i32,
                baseline: (baseline / OVERSAMPLE).round() as i32,
            }
        });
    CACHE.with(|c| c.borrow_mut().insert(key, drawn.clone()));
    drawn
}

/// The formula at exactly its logical size (a `gtk::Picture` would ask for
/// the oversampled size and leave empty space under the text).
///
/// A text view sets a child on the line's baseline by its bottom edge, so an
/// inline formula with a descender (or a fraction) looks raised. `lowered` is
/// a tag on the anchor character: its `rise` is kept at minus the part of the
/// formula under its baseline, so the formula's baseline meets the text's.
pub fn picture(drawn: &Drawn, lowered: Option<gtk::TextTag>) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(drawn.width);
    area.set_content_height(drawn.height);
    area.set_halign(gtk::Align::Start);
    area.set_valign(gtk::Align::Center);
    let (base_width, base_height) = (drawn.width, drawn.height);
    let below = f64::from(drawn.height - drawn.baseline);
    area.add_tick_callback(move |area, _| {
        let scale = crate::scale::factor(area);
        let width = (f64::from(base_width) * scale).round() as i32;
        let height = (f64::from(base_height) * scale).round() as i32;
        if area.content_width() != width || area.content_height() != height {
            area.set_content_width(width);
            area.set_content_height(height);
        }
        if let Some(tag) = &lowered {
            let rise = -(below * scale * f64::from(gtk::pango::SCALE)).round() as i32;
            if tag.rise() != rise {
                tag.set_rise(rise);
            }
        }
        gtk::glib::ControlFlow::Continue
    });
    let drawn = drawn.clone();
    area.set_draw_func(move |_, cr, width, height| {
        let (w, h) = drawn.raster;
        let Ok(surface) = gtk::cairo::ImageSurface::create_for_data(
            drawn.pixels.as_ref().clone(),
            gtk::cairo::Format::ARgb32,
            w,
            h,
            w * 4,
        ) else {
            return;
        };
        cr.scale(
            f64::from(width) / f64::from(w),
            f64::from(height) / f64::from(h),
        );
        let _ = cr.set_source_surface(&surface, 0.0, 0.0);
        let _ = cr.paint();
    });
    area
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every formula of the corpus compiles in Typst: the parser and the
    /// renderer agree on the whole subset.
    #[test]
    fn the_whole_corpus_renders() {
        for tex in formula::CORPUS {
            let nodes = formula::parse(tex).unwrap();
            for display in [false, true] {
                let source = document(&formula::to_typst(&nodes), display, false);
                let (data, w, h, _) =
                    rasterize(&source).unwrap_or_else(|| panic!("{tex}\n{source}"));
                assert!(
                    w > 4 && h > 4 && data.iter().any(|b| *b != 0),
                    "{tex}: empty"
                );
            }
        }
    }

    #[test]
    fn a_formula_typst_rejects_is_not_drawn() {
        assert!(rasterize("$frac(1,$").is_none());
    }
}
