use std::cell::{Cell, RefCell};
use std::path::Path;

use cognitive_exoskeleton_ai::{AiConfig, ProviderKind};
use serde_json::json;

use super::*;
use cognitive_exoskeleton_ai::Transport;

struct Stub {
    answers: RefCell<Vec<String>>,
    inputs: RefCell<Vec<String>>,
    calls: Cell<usize>,
}

impl Transport for Stub {
    fn post(&self, _: &str, _: &[(&str, String)], body: &Value) -> Result<(u16, String), String> {
        self.calls.set(self.calls.get() + 1);
        self.inputs.borrow_mut().push(
            body["contents"][0]["parts"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
        let text = self.answers.borrow_mut().remove(0);
        let reply = json!({"candidates":[{"content":{"parts":[{"text": text}]}}]});
        Ok((200, reply.to_string()))
    }
}

fn client(answers: &[&str]) -> Client<Stub> {
    let config = AiConfig::new(ProviderKind::Gemini, "m", None).with_key(Some("k".into()));
    Client::new(
        config,
        Stub {
            answers: RefCell::new(answers.iter().map(|s| s.to_string()).collect()),
            inputs: RefCell::new(Vec::new()),
            calls: Cell::new(0),
        },
    )
}

fn prompts() -> PromptStore {
    PromptStore::from_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/v2/port/prompts"))
        .unwrap()
}

const TEXT: &str = "Система несовместна, если ранг матрицы меньше ранга расширенной. \
Элементарные преобразования не меняют решения системы.";

fn answer(vague: bool, verdict: &str, quote: &str) -> String {
    json!({"vague": vague, "verdict": verdict, "reply": "Ранг расширенной больше — решений нет.", "quote": quote}).to_string()
}

#[test]
fn a_check_keeps_only_a_verbatim_quote() {
    let c = client(&[&answer(
        false,
        "match",
        "«ранг матрицы меньше ранга расширенной»",
    )]);
    let result = check(
        &c,
        &prompts(),
        "если ранг расширенной больше, решений нет",
        TEXT,
        false,
    )
    .unwrap();
    assert_eq!(result.verdict, Verdict::Match);
    assert_eq!(
        result.quote.as_deref(),
        Some("ранг матрицы меньше ранга расширенной")
    );
    assert!(!result.vague);

    let c = client(&[&answer(false, "partly", "придуманная цитата")]);
    let result = check(&c, &prompts(), "x", TEXT, false).unwrap();
    assert_eq!(result.quote, None);
}

#[test]
fn an_unknown_verdict_is_repaired_once() {
    let c = client(&[&answer(false, "great", ""), &answer(true, "miss", "")]);
    let result = check(&c, &prompts(), "ранг как-то связан", TEXT, false).unwrap();
    assert!(result.vague);
    assert_eq!(result.verdict, Verdict::Miss);
    assert_eq!(c.transport().calls.get(), 2);
    assert!(c.transport().inputs.borrow()[0].contains("<INSIST>\nno"));
}

#[test]
fn no_material_asks_nothing() {
    let c = client(&[]);
    assert!(matches!(
        check(&c, &prompts(), "x", " ", false),
        Err(HypothesisError::NoMaterial)
    ));
    assert_eq!(c.transport().calls.get(), 0);
}

#[test]
fn nodes_skip_used_ones_and_quote_verbatim() {
    let reply = json!({"vague": false, "mark": "wrong", "sure": true,
        "gap_quote": "ранг матрицы меньше ранга расширенной", "specific": true,
        "words": ["ранг", "свободные переменные", "базис", "ступенчатый вид", "минор", "столбец"]})
    .to_string();
    let c = client(&[&reply]);
    let n = nodes(&c, &prompts(), "x", TEXT, &["ранг".to_string()]).unwrap();
    assert_eq!(
        n.words,
        ["свободные переменные", "базис", "ступенчатый вид", "минор"]
    );
    assert!(n.gap_quote.is_some());
}

#[test]
fn reframe_may_honestly_refuse() {
    let c = client(&[
        &json!({"status": "insufficient", "frame": "", "implication": "", "basis": ""}).to_string(),
    ]);
    assert_eq!(reframe(&c, &prompts(), TEXT, "").unwrap(), None);
    let c = client(&[&json!({"status": "proposal", "frame": "Ранг как размерность", "implication": "видно, сколько свобод", "basis": "ранг матрицы"}).to_string()]);
    let r = reframe(&c, &prompts(), TEXT, "").unwrap().unwrap();
    assert_eq!(r.basis.as_deref(), Some("ранг матрицы"));
}

#[test]
fn the_guess_is_stored_before_the_check_and_an_empty_one_is_a_cancel() {
    let mut store = Store::open_in_memory().unwrap();
    assert_eq!(store.add("  ", TEXT, 1).unwrap(), None);
    let id = store
        .add("ранг связан с решениями", TEXT, 1)
        .unwrap()
        .unwrap();
    assert_eq!(store.get(id).unwrap().unwrap().verdict, None);
    store.reword(id, "больше ранг — больше решений").unwrap();
    let check = Check {
        vague: false,
        why_vague: None,
        verdict: Verdict::Miss,
        reply: "Нет.".into(),
        quote: None,
    };
    store.set_check(id, &check).unwrap();
    let saved = store.recent(5).unwrap().remove(0);
    assert_eq!(saved.text, "больше ранг — больше решений");
    assert_eq!(saved.verdict, Some(Verdict::Miss));
    assert_eq!(known_mark(saved.verdict), "ошибка");
}

#[test]
fn only_a_refuted_guess_becomes_a_unit_and_only_once() {
    let dir = std::env::temp_dir().join(format!("hyp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("j.db");
    let _ = std::fs::remove_file(&db);
    let mut store = Store::open(&db).unwrap();
    let id = store
        .add("больше ранг — больше решений", TEXT, 1)
        .unwrap()
        .unwrap();
    let mut check = Check {
        vague: false,
        why_vague: None,
        verdict: Verdict::Partly,
        reply: "…".into(),
        quote: None,
    };
    store.set_check(id, &check).unwrap();
    let h = store.get(id).unwrap().unwrap();
    assert_eq!(enroll_refuted(&db, &h, &[1, 2], 10).unwrap(), None);

    check.verdict = Verdict::Miss;
    store.set_check(id, &check).unwrap();
    let h = store.get(id).unwrap().unwrap();
    let unit = enroll_refuted(&db, &h, &[1, 2], 10).unwrap().unwrap();
    let h = store.get(id).unwrap().unwrap();
    assert_eq!(h.unit.as_deref(), Some(unit.as_str()));
    assert_eq!(
        enroll_refuted(&db, &h, &[1, 2], 10).unwrap().as_deref(),
        Some(unit.as_str())
    );
    let due = cognitive_exoskeleton_memory::Memory::open(&db)
        .unwrap()
        .due(11)
        .unwrap();
    assert_eq!(due.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}
