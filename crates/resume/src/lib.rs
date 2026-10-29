//! Alt+B — продолжить с места (F-43): the last place of study (Firefox, Okular,
//! Obsidian — Q-47), the page Okular remembers, and the reader's own last word
//! about it: a refuted guess first, then a guess, a note, the last fragment.
//! Nothing is stored and nothing is made up.

use std::path::{Path, PathBuf};

use cognitive_exoskeleton_domain::Payload;
use cognitive_exoskeleton_hypotheses::{self as hypotheses, Verdict};
use cognitive_exoskeleton_journal::Journal;
use cognitive_exoskeleton_notes::Notes;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum App {
    Firefox,
    Okular,
    Obsidian,
}

impl App {
    pub fn name(self) -> &'static str {
        match self {
            App::Firefox => "Firefox",
            App::Okular => "Okular",
            App::Obsidian => "Obsidian",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Place {
    pub app: App,
    pub document: String,
    /// The window title as it was: used to raise the same window.
    pub title: String,
    /// Obsidian's vault, from its title.
    pub vault: Option<String>,
    /// Unix time of the last selection there.
    pub at: i64,
    /// Every text selected in this document, newest last.
    pub fragments: Vec<String>,
}

/// The document name from a window: the app's suffix is cut off. Windows of
/// other programs and our own are not places of study.
pub fn document(class: &str, title: &str) -> Option<(App, String, Option<String>)> {
    let class = class.to_lowercase();
    let cut = |suffixes: &[&str]| {
        suffixes
            .iter()
            .find_map(|s| title.strip_suffix(s))
            .unwrap_or(title)
            .trim()
            .to_string()
    };
    let (app, name, vault) = if class.contains("firefox") {
        (
            App::Firefox,
            cut(&[" — Mozilla Firefox", " - Mozilla Firefox", " — Firefox"]),
            None,
        )
    } else if class.contains("okular") {
        (App::Okular, cut(&[" — Okular", " - Okular"]), None)
    } else if class.contains("obsidian") {
        // «Note - Vault - Obsidian v1.6.7»
        let parts: Vec<&str> = title.split(" - ").collect();
        match parts.as_slice() {
            [note, vault, ..] if parts.len() >= 3 => (
                App::Obsidian,
                note.trim().to_string(),
                Some(vault.trim().to_string()),
            ),
            _ => (App::Obsidian, title.trim().to_string(), None),
        }
    } else {
        return None;
    };
    (!name.is_empty()).then_some((app, name, vault))
}

/// The last place among the recent selections of study text.
pub fn last_place(journal: &Journal) -> Option<Place> {
    let events = journal.recent_sources(300).ok()?;
    let mut places: Vec<Place> = Vec::new();
    for stored in events {
        let Payload::Source(snapshot) = stored.event.payload() else {
            continue;
        };
        if !snapshot.context.is_study() {
            continue;
        }
        let (Some(class), Some(title)) = (&snapshot.application, &snapshot.window_title) else {
            continue;
        };
        if class.to_lowercase().contains("cognitive") {
            continue;
        }
        let Some((app, name, vault)) = document(class, title) else {
            continue;
        };
        let at = stored.event.occurred_at().unix_timestamp();
        match places
            .iter_mut()
            .find(|p| p.app == app && p.document == name)
        {
            Some(place) => {
                place.at = at;
                place.title = title.clone();
                place.fragments.push(snapshot.text.clone());
            }
            None => places.push(Place {
                app,
                document: name,
                title: title.clone(),
                vault,
                at,
                fragments: vec![snapshot.text.clone()],
            }),
        }
    }
    places.into_iter().max_by_key(|p| p.at)
}

/// The reader's own last word about the place (F-43, in this order).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hint {
    pub label: &'static str,
    pub quote: String,
}

pub fn hint(db: &Path, place: &Place) -> Option<Hint> {
    let here = |text: &str| {
        let text = text.trim();
        !text.is_empty() && place.fragments.iter().any(|f| f.trim() == text)
    };
    let guess = hypotheses::Store::open(db)
        .and_then(|s| s.recent(50))
        .ok()
        .and_then(|list| list.into_iter().find(|h| here(&h.material)));
    if let Some(h) = &guess
        && h.verdict == Some(Verdict::Miss)
    {
        return Some(Hint {
            label: "Ты остановился на расхождении",
            quote: h.text.clone(),
        });
    }
    let note = Notes::open(db)
        .and_then(|n| n.open_notes())
        .ok()
        .and_then(|notes| {
            notes.into_iter().find(|n| {
                !n.anchor.is_empty()
                    && place.fragments.iter().any(|f| {
                        f.split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ")
                            .starts_with(&n.anchor)
                    })
            })
        });
    match (guess, note) {
        (Some(h), Some(n)) if n.created_at > h.at => Some(Hint {
            label: "Твоя последняя мысль здесь",
            quote: n.comment,
        }),
        (Some(h), _) => Some(Hint {
            label: "Твоя последняя гипотеза",
            quote: h.text,
        }),
        (None, Some(n)) => Some(Hint {
            label: "Твоя последняя мысль здесь",
            quote: n.comment,
        }),
        (None, None) => place.fragments.last().map(|f| Hint {
            label: "Последний фрагмент",
            quote: f.chars().take(200).collect(),
        }),
    }
}

/// Where Okular keeps its per-document positions (native and Flatpak).
pub fn okular_docdata_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    vec![
        data.join("okular/docdata"),
        home.join(".var/app/org.kde.okular/data/okular/docdata"),
    ]
}

/// The file and the page (1-based) Okular remembers for a document name.
pub fn okular_position(dirs: &[PathBuf], document: &str) -> Option<(PathBuf, u32)> {
    let mut best: Option<(std::time::SystemTime, PathBuf, u32)> = None;
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(xml) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let Some((path, page)) = parse_docdata(&xml) else {
                continue;
            };
            if path.file_name().and_then(|n| n.to_str()) != Some(document) {
                continue;
            }
            let modified = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            if best.as_ref().is_none_or(|(t, ..)| modified > *t) {
                best = Some((modified, path, page));
            }
        }
    }
    best.map(|(_, path, page)| (path, page))
}

/// `<documentInfo url="file:///…">` and `<current viewport="41;…"/>` (0-based).
pub fn parse_docdata(xml: &str) -> Option<(PathBuf, u32)> {
    let attribute = |tag: &str, name: &str| -> Option<String> {
        let start = xml.find(&format!("<{tag}"))?;
        let rest = &xml[start..];
        let key = format!("{name}=\"");
        let from = rest.find(&key)? + key.len();
        let to = rest[from..].find('"')?;
        Some(rest[from..from + to].to_string())
    };
    let url = attribute("documentInfo", "url")?;
    let path = PathBuf::from(percent_decode(url.strip_prefix("file://").unwrap_or(&url)));
    let page = attribute("current", "viewport")
        .and_then(|v| v.split(';').next()?.parse::<u32>().ok())
        .map(|p| p + 1)
        .unwrap_or(1);
    Some((path, page))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(byte) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests;
