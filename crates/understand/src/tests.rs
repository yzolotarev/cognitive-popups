use std::cell::{Cell, RefCell};
use std::path::Path;

use cognitive_exoskeleton_ai::{AiConfig, ProviderKind};
use serde_json::json;

use super::*;

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

const TEXT: &str = "Ранг матрицы — число линейно независимых строк. Элементарные преобразования \
не меняют решения системы. Система несовместна, если ранг матрицы меньше ранга расширенной.";

fn cue(simple: &str, term: &str, meaning: &str) -> Value {
    json!({"simple": simple, "term": term, "meaning": meaning})
}

fn four() -> String {
    json!({"cues": [
        cue("метрика", "ранг", "число независимых строк"),
        cue("приём", "элементарные преобразования", "не меняют решения"),
        cue("форма", "ступенчатый вид", "нули под ведущими"),
        cue("условие", "совместность", "ранги равны"),
    ]})
    .to_string()
}

#[test]
fn weights_hold_over_many_presses() {
    // F-02 criterion: 1000 presses → words in 65–75 %.
    let mut words = 0;
    for i in 0..1000 {
        let roll = (i as f64 + 0.5) / 1000.0;
        if pick(&[], true, roll) == Some(Mechanic::Words) {
            words += 1;
        }
    }
    assert!((650..=750).contains(&words), "{words}");
    assert_eq!(pick(&[], true, 0.71), Some(Mechanic::Explain));
    assert_eq!(pick(&[], true, 0.999), Some(Mechanic::Character));
}

#[test]
fn without_text_nothing_is_picked_and_switched_off_ones_are_skipped() {
    assert_eq!(pick(&[], false, 0.1), None);
    assert_eq!(
        pick(&[Mechanic::Words], true, 0.0),
        Some(Mechanic::Explain),
        "the others are renormalised"
    );
    assert_eq!(pick(&Mechanic::ALL, true, 0.3), None);
}

#[test]
fn the_system_roll_is_in_range() {
    for _ in 0..100 {
        let r = roll();
        assert!((0.0..1.0).contains(&r));
    }
}

#[test]
fn four_words_are_checked_and_repaired_once() {
    let three = json!({"cues": [cue("a", "b", "c"), cue("a", "b", "c"), cue("a", "b", "c")]});
    let c = client(&[&three.to_string(), &four()]);
    let cues = words(&c, &prompts(), TEXT, Priority::Foreground).unwrap();
    assert_eq!(cues.len(), 4);
    assert_eq!(c.transport().calls.get(), 2);
    assert!(c.transport().inputs.borrow()[1].contains("ровно 4"));
}

#[test]
fn two_word_simple_is_rejected() {
    let bad = json!({"cues": [cue("две части", "b", "c"), cue("a", "b", "c"), cue("a", "b", "c"), cue("a", "b", "c")]});
    assert!(cues_check(&bad).unwrap_err().contains("одним словом"));
    let long = json!({"cues": [cue("a", "b", "один два три четыре пять шесть семь восемь девять десять одиннадцать двенадцать тринадцать"), cue("a", "b", "c"), cue("a", "b", "c"), cue("a", "b", "c")]});
    assert!(cues_check(&long).is_err());
}

#[test]
fn a_failed_four_words_says_so() {
    let bad = json!({"cues": []}).to_string();
    let c = client(&[&bad, &bad]);
    let error = words(&c, &prompts(), TEXT, Priority::Foreground).unwrap_err();
    assert_eq!(
        error.user_message(Mechanic::Words, "Gemini"),
        "Сбой системы: не удалось собрать четыре слова. Попробуйте ещё раз."
    );
}

#[test]
fn no_text_asks_nothing_of_the_model() {
    let c = client(&[]);
    assert!(matches!(
        words(&c, &prompts(), "  ", Priority::Foreground),
        Err(UnderstandError::NoMaterial)
    ));
    assert_eq!(c.transport().calls.get(), 0);
}

#[test]
fn own_words_meet_by_stem() {
    let cues: Vec<Cue> =
        serde_json::from_value(serde_json::from_str::<Value>(&four()).unwrap()["cues"].clone())
            .unwrap();
    let mine = own_words("Ранга, строки\nступени, нули");
    let result = compare(&mine, &cues);
    assert_eq!(result.mine, vec![Some(0), None, Some(2), None]);
    assert_eq!(result.cues, vec![true, false, true, false]);
    assert_eq!(result.overlap(), 2);
}

#[test]
fn stems_are_light_but_not_too_greedy() {
    assert_eq!(stem("Матрицы"), stem("матрица"));
    assert_eq!(stem("ёлка"), stem("елки"));
    assert_ne!(stem("ранг"), stem("рана"));
}

#[test]
fn questions_and_terms_are_told_apart() {
    assert!(looks_like_question("что такое ранг"));
    assert!(looks_like_question("ранг связан с решениями?"));
    assert!(looks_like_question("можно ли умножить на ноль"));
    assert!(!looks_like_question("несовместна, ведущий элемент"));
    assert!(!looks_like_question("ранг"));
}

#[test]
fn more_than_four_terms_are_cut_and_named() {
    let (terms, omitted) = split_terms("a, b, c, d, e, f");
    assert_eq!(terms, ["a", "b", "c", "d"]);
    assert_eq!(omitted, ["e", "f"]);
}

#[test]
fn explanations_are_cut_to_80_words_and_marked_when_not_in_text() {
    let long = vec!["слово"; 90].join(" ");
    let answer = json!({"lines": [
        {"term": "ранг", "explanation": long},
        {"term": "базис", "explanation": "набор независимых векторов"},
    ]})
    .to_string();
    let c = client(&[&answer]);
    let result = explain_terms(&c, &prompts(), TEXT, "ранг, базис").unwrap();
    assert!(result.lines[0].shortened);
    assert_eq!(result.lines[0].explanation.split_whitespace().count(), 80);
    assert!(result.lines[0].in_text);
    assert!(!result.lines[1].in_text);
    assert!(result.omitted.is_empty());
}

#[test]
fn a_term_not_asked_for_is_repaired() {
    let wrong = json!({"lines": [{"term": "другое", "explanation": "x"}]}).to_string();
    let right = json!({"lines": [{"term": "ранг", "explanation": "число строк"}]}).to_string();
    let c = client(&[&wrong, &right]);
    let result = explain_terms(&c, &prompts(), TEXT, "ранг").unwrap();
    assert_eq!(result.lines.len(), 1);
    assert_eq!(c.transport().calls.get(), 2);
}

#[test]
fn an_answer_keeps_only_a_real_quote() {
    let real = json!({"grounding": "in_text", "answer": "Когда ранги различаются.",
        "basis": "«Система несовместна, если ранг матрицы меньше ранга расширенной.»"})
    .to_string();
    let c = client(&[&real]);
    let answer = answer_question(&c, &prompts(), TEXT, "когда нет решений?").unwrap();
    assert_eq!(answer.grounding, Grounding::InText);
    assert!(answer.basis.unwrap().starts_with("Система несовместна"));

    let invented =
        json!({"grounding": "in_text", "answer": "x", "basis": "ранг всегда равен нулю"})
            .to_string();
    let c = client(&[&invented]);
    let answer = answer_question(&c, &prompts(), TEXT, "что?").unwrap();
    assert_eq!(answer.basis, None);
    assert_eq!(answer.grounding, Grounding::PartlyInText);
    assert!(answer.grounding.label().starts_with("Часть"));
}

#[test]
fn an_example_without_material_or_request_is_refused() {
    let c = client(&["Пример."]);
    assert!(matches!(
        example(&c, &prompts(), "", None),
        Err(UnderstandError::NoMaterial)
    ));
    assert_eq!(
        example(&c, &prompts(), "", Some("покажи XLOOKUP")).unwrap(),
        "Пример."
    );
}

#[test]
fn the_ladder_sends_at_most_eight_known_lines() {
    let answer = json!({"from": "ранг", "to": "совместность", "text": "Шаг."}).to_string();
    let c = client(&[&answer]);
    let known: Vec<Known> = (0..12)
        .map(|i| Known {
            mark: "верно",
            text: format!("мысль {i}"),
        })
        .collect();
    let rung = ladder(&c, &prompts(), "теорема Кронекера–Капелли", TEXT, &known).unwrap();
    assert_eq!(rung.to, "совместность");
    let input = &c.transport().inputs.borrow()[0];
    assert!(input.contains("мысль 7") && !input.contains("мысль 8"));
}

#[test]
fn the_character_keeps_only_verbatim_facts() {
    let answer = json!({"role": "кладовщик", "monologue": "«Я складываю ящики…»",
        "facts": ["число линейно независимых строк", "придуманная цитата"],
        "fiction": ["ящики"]})
    .to_string();
    let c = client(&[&answer]);
    let m = character(&c, &prompts(), TEXT).unwrap();
    assert_eq!(m.facts, ["число линейно независимых строк"]);
    assert_eq!(m.monologue, "Я складываю ящики…");
}

#[test]
fn the_cache_clears_at_the_thirty_third() {
    let mut cache = Cache::default();
    for i in 0..32 {
        cache.put(Mechanic::Example, &format!("t{i}"), json!(i));
    }
    assert!(cache.get(Mechanic::Example, "t0").is_some());
    cache.put(Mechanic::Example, "t32", json!(32));
    assert!(cache.get(Mechanic::Example, "t0").is_none());
    assert!(cache.get(Mechanic::Example, " t32 ").is_some());
    assert!(cache.get(Mechanic::Words, "t32").is_none());
}

#[test]
fn the_store_keeps_choices_own_words_and_rungs() {
    let mut store = Store::open_in_memory().unwrap();
    store.record_choice(Mechanic::Words, false, &[], 1).unwrap();
    store
        .record_own_cues(TEXT, &["ранг".into()], &[], 1, 2)
        .unwrap();
    let rung = Rung {
        from: "ранг".into(),
        to: "совместность".into(),
        text: "…".into(),
    };
    let id = store.add_rung("цель", &rung, 3).unwrap();
    store.mark_rung(id, true).unwrap();
    let rungs = store.recent_rungs(6).unwrap();
    assert_eq!(rungs[0].mark.as_deref(), Some("known"));
}
