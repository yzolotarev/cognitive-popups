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

fn client(answers: &[Value]) -> Client<Stub> {
    let config = AiConfig::new(ProviderKind::Gemini, "m", None).with_key(Some("k".into()));
    Client::new(
        config,
        Stub {
            answers: RefCell::new(answers.iter().map(Value::to_string).collect()),
            inputs: RefCell::new(Vec::new()),
            calls: Cell::new(0),
        },
    )
}

fn prompts() -> PromptStore {
    PromptStore::from_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/v2/port/prompts"))
        .unwrap()
}

const TEXT: &str = "Элементарное преобразование: к строке R2 прибавляют строку R1, умноженную на число. \
Например, R2 − 3·R1. Перед этим считают произведения: 3·4 = 12. \
Ссылку B3 делают абсолютной: $B$3 не меняется при копировании.";

fn numeric(check: &str) -> Value {
    json!({"title":"Умножить на число","invariant":"одно произведение","source_span":"3·4 = 12",
        "relevance":"счёт","kind":"numeric","example":{"before":"3·4","after":"12"},
        "params":{"a":{"kind":"int","min":2,"max":9},"b":{"kind":"int","min":2,"max":9}},
        "derived":{"r":"a*b"},"check":check,"prompt":"{a}·{b}","answer":"{r}"})
}

fn arithmetic_drill() -> Value {
    json!({"title":"Вычесть кратное","invariant":"R2 − k·R1","source_span":"Например, R2 − 3·R1.",
        "relevance":"шаг Гаусса","kind":"drill","pairs":[
            {"prompt":"7 − 3·2","answer":"1"},{"prompt":"9 − 2·4","answer":"1"},
            {"prompt":"10 − 3·3","answer":"2"},{"prompt":"8 − 2·2","answer":"4"},
            {"prompt":"6 − 1·5","answer":"1"},{"prompt":"12 − 4·2","answer":"4"}]})
}

fn reference_drill(span: &str) -> Value {
    json!({"title":"Сделать ссылку абсолютной","invariant":"$ перед столбцом и строкой",
        "source_span":span,"relevance":"Excel","kind":"drill","pairs":[
            {"prompt":"B3","answer":"$B$3"},{"prompt":"C7","answer":"$C$7"},
            {"prompt":"A1","answer":"$A$1"},{"prompt":"D12","answer":"$D$12"},
            {"prompt":"F2","answer":"F2"},{"prompt":"E5","answer":"$E$6"}]})
}

#[test]
fn the_code_decides_what_is_shown() {
    let client = client(&[
        json!({"groups":[numeric("r == a*b"), arithmetic_drill(),
                         reference_drill("$B$3 не меняется при копировании")]}),
        json!({"bad":[9]}),
    ]);
    let tree = probe_tree(&client, &prompts(), TEXT, Priority::Foreground).unwrap();
    assert_eq!(tree.groups.len(), 3, "{:#?}", tree.notes);

    let product = &tree.groups[0];
    assert_eq!(product.kind, Kind::Numeric);
    for card in std::iter::once(&product.example)
        .chain(&product.cards)
        .chain(&product.reserve)
    {
        let (a, b) = card.prompt.split_once('·').unwrap();
        let expected = a.parse::<i64>().unwrap() * b.parse::<i64>().unwrap();
        assert_eq!(card.answer, expected.to_string(), "computed by the code");
    }

    let subtract = &tree.groups[1];
    let shown: Vec<&str> = subtract.cards.iter().map(|c| c.prompt.as_str()).collect();
    assert!(
        !shown.contains(&"10 − 3·3") && !subtract.reserve.iter().any(|c| c.prompt == "10 − 3·3")
    );
    assert_eq!(subtract.example.prompt, "7 − 3·2");

    let reference = &tree.groups[2];
    let all: Vec<&str> = std::iter::once(&reference.example)
        .chain(&reference.cards)
        .chain(&reference.reserve)
        .map(|c| c.prompt.as_str())
        .collect();
    assert!(!all.contains(&"F2"), "identity dropped by the code");
    assert!(!all.contains(&"E5"), "dropped by the verifier");
    assert!(
        tree.notes
            .iter()
            .any(|n| n.contains("проверяльщик убрал «E5 → $E$6»"))
    );

    let verifier_input = client.transport().inputs.borrow()[1].clone();
    assert!(verifier_input.contains(
        "Ветвь «Сделать ссылку абсолютной» (в тексте: «$B$3 не меняется при копировании»)"
    ));
    assert!(
        verifier_input.contains("Ветвь «Умножить на число»"),
        "numeric meaning is checked"
    );
    assert!(
        !verifier_input.contains("7 − 3·2"),
        "arithmetic is not sent to the model"
    );
}

#[test]
fn a_wrong_citation_or_a_false_check_is_asked_for_again_once() {
    let client = client(&[
        json!({"groups":[numeric("r == a + b"), arithmetic_drill(),
                         reference_drill("ссылки бывают абсолютными")]}),
        json!({"groups":[numeric("r == a*b"), reference_drill("Ссылку B3 делают абсолютной")]}),
        json!({"bad":[]}),
    ]);
    let tree = probe_tree(&client, &prompts(), TEXT, Priority::Foreground).unwrap();
    assert_eq!(tree.groups.len(), 3, "{:#?}", tree.notes);
    let repair = client.transport().inputs.borrow()[1].clone();
    assert!(repair.contains("УЖЕ ПРИНЯТО: Вычесть кратное"), "{repair}");
    assert!(repair.contains("check ложен при"), "{repair}");
    assert!(repair.contains("нет в тексте дословно"), "{repair}");
    assert_eq!(client.transport().calls.get(), 3);
}

#[test]
fn after_one_repair_the_tree_shows_what_survived() {
    let client = client(&[
        json!({"groups":[arithmetic_drill(), numeric("r == 0")]}),
        json!({"groups":[numeric("r == 1")]}),
    ]);
    let tree = probe_tree(&client, &prompts(), TEXT, Priority::Foreground).unwrap();
    assert_eq!(tree.groups.len(), 1);
    assert_eq!(
        client.transport().calls.get(),
        2,
        "only arithmetic left: no verifier call"
    );
}

#[test]
fn reserve_feeds_more_three_and_the_diagnostic() {
    let client = client(&[
        json!({"groups":[numeric("r == a*b")]}),
        json!({"bad":[]}),
        json!({"groups":[numeric("r == 0")]}),
    ]);
    let mut tree = probe_tree(&client, &prompts(), TEXT, Priority::Foreground).unwrap();
    let group = &mut tree.groups[0];
    assert_eq!(group.reserve.len(), RESERVE);
    let first = group.diagnostic().unwrap();
    assert_eq!(group.more_three().unwrap().len(), 3);
    assert!(!group.reserve.contains(&first));
}

#[test]
fn an_empty_text_costs_no_request() {
    let client = client(&[]);
    assert!(matches!(
        probe_tree(&client, &prompts(), "  \n", Priority::Foreground),
        Err(PracticeError::EmptyText)
    ));
    assert_eq!(client.transport().calls.get(), 0);
}

#[test]
fn a_numeric_template_the_verifier_rejects_is_asked_for_again() {
    let client = client(&[
        json!({"groups":[numeric("r == a*b"), arithmetic_drill(),
                         reference_drill("$B$3 не меняется при копировании")]}),
        json!({"bad":[2]}),
        json!({"groups":[numeric("r == a*b")]}),
        json!({"bad":[]}),
    ]);
    let tree = probe_tree(&client, &prompts(), TEXT, Priority::Foreground).unwrap();
    assert_eq!(tree.groups.len(), 3, "{:#?}", tree.notes);
    let repair = client.transport().inputs.borrow()[2].clone();
    assert!(repair.contains("проверяльщик отклонил шаблон"), "{repair}");
    assert!(repair.contains("УЖЕ ПРИНЯТО: Вычесть кратное; Сделать ссылку абсолютной"));
}

#[test]
fn a_drill_whose_answer_never_changes_is_not_an_operation() {
    let constant = json!({"title":"Обнулить константу","source_span":"Например, R2 − 3·R1.",
        "kind":"drill","pairs":[{"prompt":"(1)′","answer":"0"},{"prompt":"(2)′","answer":"0"},
        {"prompt":"(3)′","answer":"0"},{"prompt":"(4)′","answer":"0"}]});
    let error = accept(&constant, TEXT, 1).err().unwrap();
    assert!(error.contains("не зависит от данных"), "{error}");
}

fn group(title: &str) -> ProbeGroup {
    let card = Card {
        prompt: "a".into(),
        answer: "b".into(),
    };
    ProbeGroup {
        title: title.into(),
        invariant: String::new(),
        source_span: String::new(),
        relevance: String::new(),
        kind: Kind::Drill,
        example: card.clone(),
        cards: vec![card],
        reserve: Vec::new(),
    }
}

fn titles(tree: &[Category]) -> Vec<String> {
    tree.iter()
        .map(|c| match c {
            Category::Probe(g) => g.title.clone(),
            Category::Recall { .. } => "память".into(),
        })
        .collect()
}

#[test]
fn b31_recall_replaces_the_third_category_or_comes_first_from_the_pill() {
    let probes = || vec![group("A"), group("B"), group("C")];
    let due = || {
        vec![RecallCard {
            unit: "u1".into(),
            question: "q".into(),
            reference: "r s".into(),
            events: vec![1],
        }]
    };
    assert_eq!(titles(&arrange(probes(), vec![], false)), ["A", "B", "C"]);
    assert_eq!(
        titles(&arrange(probes(), due(), false)),
        ["A", "B", "память"]
    );
    assert_eq!(
        titles(&arrange(probes(), due(), true)),
        ["память", "A", "B"]
    );
    assert_eq!(titles(&arrange(vec![], due(), true)), ["память"]);
}
