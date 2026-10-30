use cognitive_exoskeleton_domain::{
    NewEvent, OperationId, SourceContext, SourceKind, SourceSnapshot,
};
use time::OffsetDateTime;

use super::*;

fn source(
    journal: &mut Journal,
    n: i64,
    class: &str,
    title: &str,
    text: &str,
    context: SourceContext,
) {
    let snapshot = SourceSnapshot {
        kind: SourceKind::Selection,
        text: text.into(),
        application: Some(class.into()),
        window_title: Some(title.into()),
        context,
        blocks: Vec::new(),
    };
    let at = OffsetDateTime::from_unix_timestamp(1_700_000_000 + n).unwrap();
    let event = NewEvent::source(
        OperationId::new(format!("op{n}")).unwrap(),
        at,
        format!("k{n}"),
        snapshot,
    )
    .unwrap();
    journal.append(&[event]).unwrap();
}

#[test]
fn documents_are_named_without_the_app() {
    assert_eq!(
        document("org.kde.okular", "linal.pdf — Okular"),
        Some((App::Okular, "linal.pdf".into(), None))
    );
    assert_eq!(
        document("firefox", "Ранг матрицы — Википедия — Mozilla Firefox"),
        Some((App::Firefox, "Ранг матрицы — Википедия".into(), None))
    );
    assert_eq!(
        document("obsidian", "Ранг - Учёба - Obsidian v1.6.7"),
        Some((App::Obsidian, "Ранг".into(), Some("Учёба".into())))
    );
    assert_eq!(document("kitty", "zsh"), None);
}

#[test]
fn the_last_place_skips_development_and_own_windows() {
    let mut journal = Journal::open_in_memory().unwrap();
    source(
        &mut journal,
        1,
        "org.kde.okular",
        "linal.pdf — Okular",
        "ранг матрицы",
        SourceContext::Study,
    );
    source(
        &mut journal,
        2,
        "firefox",
        "Ранг — Mozilla Firefox",
        "другое",
        SourceContext::Study,
    );
    source(
        &mut journal,
        3,
        "org.kde.okular",
        "linal.pdf — Okular",
        "совместность",
        SourceContext::Study,
    );
    source(
        &mut journal,
        4,
        "kitty",
        "vim",
        "code",
        SourceContext::Development,
    );
    source(
        &mut journal,
        5,
        "firefox",
        "Claude — Mozilla Firefox",
        "чат",
        SourceContext::AiChat,
    );
    let place = last_place(&journal).unwrap();
    assert_eq!(place.app, App::Okular);
    assert_eq!(place.fragments, ["ранг матрицы", "совместность"]);
}

#[test]
fn okular_remembers_the_page() {
    let xml = r#"<?xml version="1.0"?><documentInfo url="file:///home/u/%D0%BB%D0%B8%D0%BD/linal.pdf"><generalInfo><history><current viewport="41;C2:0.5:0.3:1"/></history></generalInfo></documentInfo>"#;
    let (path, page) = parse_docdata(xml).unwrap();
    assert_eq!(path, PathBuf::from("/home/u/лин/linal.pdf"));
    assert_eq!(page, 42);
    let dir = std::env::temp_dir().join(format!("cp-okular-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("123.linal.pdf.xml"), xml).unwrap();
    assert_eq!(
        okular_position(std::slice::from_ref(&dir), "linal.pdf")
            .unwrap()
            .1,
        42
    );
    assert_eq!(
        okular_position(std::slice::from_ref(&dir), "other.pdf"),
        None
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_refuted_guess_comes_first_then_a_guess_then_the_fragment() {
    let dir = std::env::temp_dir().join(format!("cp-resume-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("j.db");
    let _ = std::fs::remove_file(&db);
    let place = Place {
        app: App::Okular,
        document: "linal.pdf".into(),
        title: "linal.pdf — Okular".into(),
        vault: None,
        at: 0,
        fragments: vec!["система несовместна".into()],
    };
    assert_eq!(hint(&db, &place).unwrap().label, "Последний фрагмент");
    let mut store = hypotheses::Store::open(&db).unwrap();
    let id = store
        .add("решений много", "система несовместна", 10)
        .unwrap()
        .unwrap();
    assert_eq!(hint(&db, &place).unwrap().label, "Твоя последняя гипотеза");
    let check = hypotheses::Check {
        vague: false,
        why_vague: None,
        verdict: Verdict::Miss,
        reply: "нет".into(),
        quote: None,
    };
    store.set_check(id, &check).unwrap();
    let h = hint(&db, &place).unwrap();
    assert_eq!(h.label, "Ты остановился на расхождении");
    assert_eq!(h.quote, "решений много");
    let _ = std::fs::remove_dir_all(&dir);
}
