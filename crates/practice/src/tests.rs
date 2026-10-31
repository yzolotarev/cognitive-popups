use std::cell::{Cell, RefCell};
use std::path::Path;

use cognitive_exoskeleton_ai::{AiConfig, ProviderKind};
use serde_json::json;

use super::*;
use crate::whole;

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
fn after_the_repairs_the_tree_shows_what_survived() {
    let client = client(&[
        json!({"groups":[arithmetic_drill(), numeric("r == 0")]}),
        json!({"groups":[numeric("r == 1")]}),
        json!({"groups":[numeric("r == 2")]}),
    ]);
    let tree = probe_tree(&client, &prompts(), TEXT, Priority::Foreground).unwrap();
    assert_eq!(tree.groups.len(), 1);
    assert_eq!(
        client.transport().calls.get(),
        1 + REPAIR_ROUNDS,
        "only arithmetic left: no verifier call"
    );
}

#[test]
fn reserve_feeds_more_three_and_the_diagnostic() {
    let client = client(&[
        json!({"groups":[numeric("r == a*b")]}),
        json!({"bad":[]}),
        json!({"groups":[numeric("r == 0")]}),
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
        role: Role::Operation,
        template: None,
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
            family: None,
            seed: None,
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

fn types() -> Vec<whole::TaskType> {
    load_types(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/v2/port/prompts")).unwrap()
}

#[test]
fn a_whole_task_gets_a_condition_and_a_hidden_solution() {
    let ready = json!({"status":"ready",
        "condition":"Дана матрица [[2, 1], [6, 5]].\nТребуется: привести её к ступенчатому виду.",
        "grounding":["R2 − 3·R1", "придумано моделью"],"required_operations":[],
        "difficulty":"standard","confidence":"medium","payload":{}});
    let client = client(&[ready]);
    client
        .transport()
        .answers
        .borrow_mut()
        .push("R2 − 3·R1 даёт [0, 2].".into());
    let ops = vec![
        "Вычесть кратное".to_string(),
        "Умножить на число".to_string(),
    ];
    let task = whole_task(
        &client,
        &prompts(),
        &types(),
        TEXT,
        &ops,
        None,
        true,
        Priority::Foreground,
    )
    .unwrap();
    let WholeTask::Ready {
        type_id,
        condition,
        grounding,
        solution,
        notes,
        ..
    } = task
    else {
        panic!("{task:?}");
    };
    assert_eq!(type_id, 1);
    assert!(condition.ends_with("Требуется: привести её к ступенчатому виду."));
    assert_eq!(grounding, vec!["R2 − 3·R1"]);
    assert!(notes[0].contains("придумано моделью"));
    assert!(solution.contains("[0, 2]"));
    let system = client.transport().inputs.borrow()[0].clone();
    assert!(system.contains("ТЕКСТ") || system.contains("МАТЕРИАЛ"));
}

#[test]
fn a_newcomer_cannot_get_a_hard_type_and_no_request_is_made() {
    let client = client(&[]);
    let error = whole_task(
        &client,
        &prompts(),
        &types(),
        TEXT,
        &[],
        Some(5),
        false,
        Priority::Foreground,
    )
    .unwrap_err();
    assert!(error.to_string().contains("новичку"), "{error}");
    assert_eq!(client.transport().calls.get(), 0);
}

#[test]
fn needs_context_is_reported_not_invented() {
    let client = client(&[json!({"status":"needs_context","reason":"в тексте нет метода"})]);
    let task = whole_task(
        &client,
        &prompts(),
        &types(),
        TEXT,
        &[],
        Some(7),
        false,
        Priority::Foreground,
    )
    .unwrap();
    assert_eq!(
        task,
        WholeTask::NeedsContext {
            type_id: 7,
            reason: "в тексте нет метода".into()
        }
    );
    assert_eq!(client.transport().calls.get(), 1, "no solution call");
}

#[test]
fn a_unit_with_a_task_family_gets_a_code_card_without_a_request() {
    let unit = DueUnit {
        id: "u1".into(),
        title: "Целые корни".into(),
        action: "находить целые корни".into(),
        kind: "execute".into(),
        evidence: Vec::new(),
        family: Some(families::Family::IntegerRoots),
    };
    let client = client(&[]);
    let set = recall_cards(&client, &prompts(), &[unit], 7, Priority::Foreground).unwrap();
    assert_eq!(client.transport().calls.get(), 0);
    let card = &set.cards[0];
    assert_eq!(card.family, Some(families::Family::IntegerRoots));
    let task = families::generate(families::Family::IntegerRoots, card.seed.unwrap());
    assert_eq!(
        task.prompt, card.question,
        "the task can be re-created to grade it"
    );
    assert!(families::check(&task, &card.reference));
}

#[test]
fn a_quote_with_a_slip_is_replaced_by_the_text_itself() {
    let text = "Так же меняются dein, sein и unser: deine Mutter, unsere Stadt.";
    assert_eq!(
        verbatim_span(text, "Так же меняются dein, sein und unser").as_deref(),
        Some("Так же меняются dein, sein и unser")
    );
    assert_eq!(
        verbatim_span(text, "deine  Mutter").as_deref(),
        Some("deine  Mutter")
    );
    assert_eq!(
        verbatim_span(text, "совсем другое предложение про ранг"),
        None
    );
}

#[test]
fn feynman_keeps_at_most_two_gaps_and_skips_an_empty_explanation() {
    use crate::feynman;
    let c = client(&[
        json!({"question": "Почему преобразования не меняют решения?", "rubric": "обратимость"}),
    ]);
    let q = feynman::question(&c, &prompts(), TEXT).unwrap();
    assert!(q.question.starts_with("Почему"));
    assert_eq!(
        feynman::check(&c, &prompts(), TEXT, &q, "  ").unwrap(),
        None
    );
    assert_eq!(c.transport().calls.get(), 1);

    let reply = json!({"status": "needs_retry", "gaps": [
        {"location": "a", "type": "missing_causal_link", "description": "один"},
        {"location": "b", "type": "missing_condition", "description": "два"},
        {"location": "c", "type": "missing_condition", "description": "три"}],
        "follow_up": "Что будет при умножении на 0?"});
    let c = client(&[reply]);
    match feynman::check(&c, &prompts(), TEXT, &q, "Потому что так").unwrap() {
        Some(feynman::Feedback::NeedsRetry { gaps, follow_up }) => {
            assert_eq!(gaps.len(), 2);
            assert!(follow_up.is_some());
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn fixed_path_feynman_uses_the_integrative_prompt_and_rejects_unusable_questions() {
    use crate::feynman;
    let reply = json!({
        "question": "Как различие рангов показывает, можно ли решить систему?",
        "rubric": "связать условие совместности с рангами",
        "coverage": {"covered": ["сравнить ранги"], "unverified": []},
        "quality": {"single": true, "integrative": true, "no_hints": true}
    });
    let c = client(std::slice::from_ref(&reply));
    let meaning_map = r#"[{"step":"сравнить ранги","takeaway":"решает число строк"}]"#;
    let question = feynman::integrative_question(&c, &prompts(), meaning_map).unwrap();
    assert!(question.question.starts_with("Как различие"));
    assert!(question.rubric.contains("совместности"));
    let input = &c.transport().inputs.borrow()[0];
    assert!(input.contains("<MEANING_MAP>"));
    assert!(input.contains("сравнить ранги"));
    assert!(
        prompts()
            .get(feynman::INTEGRATIVE_PROMPT)
            .unwrap()
            .contains("ОДИН итоговый вопрос")
    );

    let mut bad = reply;
    bad["quality"]["no_hints"] = json!(false);
    let c = client(&[bad.clone(), bad]);
    assert!(feynman::integrative_question(&c, &prompts(), meaning_map).is_err());
    assert_eq!(c.transport().calls.get(), 2, "bad JSON is retried once");
}

fn shown_group() -> ProbeGroup {
    ProbeGroup {
        title: "Умножить на число".into(),
        invariant: "одно произведение".into(),
        source_span: "3·4 = 12".into(),
        relevance: "счёт".into(),
        kind: Kind::Numeric,
        example: Card {
            prompt: "3·4".into(),
            answer: "12".into(),
        },
        cards: vec![Card {
            prompt: "2·5".into(),
            answer: "10".into(),
        }],
        reserve: Vec::new(),
        role: Role::Operation,
        template: None,
    }
}

#[test]
fn harder_keeps_the_operation_and_asks_for_the_level_the_code_chose() {
    let mut bigger = numeric("r == a*b");
    bigger["title"] = json!("Другое название");
    bigger["params"] =
        json!({"a":{"kind":"int","min":12,"max":99},"b":{"kind":"int","min":12,"max":99}});
    let client = client(&[json!({"groups":[bigger]}), json!({"bad":[]})]);
    let harder = harder_group(
        &client,
        &prompts(),
        TEXT,
        &shown_group(),
        3,
        Priority::Foreground,
    )
    .unwrap();
    assert_eq!(
        harder.title, "Умножить на число",
        "the category keeps its name"
    );
    assert_eq!(harder.cards.len(), CARDS);
    let input = client.transport().inputs.borrow()[0].clone();
    assert!(input.contains("УРОВЕНЬ: 3 из 5"), "{input}");
    assert!(
        input.contains("2·5 → 10"),
        "the cards one level down are shown"
    );
}

#[test]
fn harder_cards_the_code_rejects_are_not_shown() {
    let mut wrong = numeric("r == a*b");
    wrong["source_span"] = json!("этой цитаты нет в тексте");
    let client = client(&[
        json!({"groups":[wrong.clone()]}),
        json!({"groups":[wrong.clone()]}),
        json!({"groups":[wrong]}),
    ]);
    let error = harder_group(
        &client,
        &prompts(),
        TEXT,
        &shown_group(),
        2,
        Priority::Foreground,
    )
    .unwrap_err();
    assert!(matches!(error, PracticeError::Rejected(_)), "{error}");
    let inputs = client.transport().inputs.borrow();
    assert!(
        inputs[1].contains("ОТКЛОНЕНО"),
        "the reason goes back to the model"
    );
}

#[test]
fn stuck_asks_for_concepts_and_a_question_and_never_takes_the_answer() {
    let group = shown_group();
    let leak =
        json!({"concepts": ["a", "b", "c"], "question": format!("Почему {}?", group.invariant)});
    let good = json!({"concepts": ["ошибка", "шаг", "квадрат"], "question": "Что происходит с ошибкой за один шаг?"});
    let client = client(&[leak, good]);
    let scaffold = stuck::scaffold(
        &client,
        &prompts(),
        TEXT,
        &stuck::Target::operation(&group),
        Priority::Foreground,
    )
    .unwrap();
    assert_eq!(scaffold.concepts, ["ошибка", "шаг", "квадрат"]);
    let inputs = client.transport().inputs.borrow();
    assert!(
        inputs[0].contains(&group.title),
        "the operation is in the input"
    );
    assert!(
        inputs[1].contains("пересказывает"),
        "a leaked answer goes back with the reason"
    );
}

#[test]
fn a_text_with_one_operation_gives_one_branch_and_is_not_asked_for_more() {
    let client = client(&[json!({"groups":[numeric("r == a*b")]}), json!({"bad":[]})]);
    let tree = probe_tree(&client, &prompts(), TEXT, Priority::Foreground).unwrap();
    assert_eq!(tree.groups.len(), 1);
    assert_eq!(
        tree.groups[0].role,
        Role::Operation,
        "no role reads as an operation"
    );
    assert_eq!(
        client.transport().calls.get(),
        2,
        "the tree and its verifier; a missing branch is not asked for"
    );
    let client = client_empty();
    let tree = probe_tree(&client, &prompts(), TEXT, Priority::Foreground).unwrap();
    assert!(
        tree.groups.is_empty(),
        "a text without operations gives none"
    );
}

fn client_empty() -> Client<Stub> {
    client(&[json!({"groups":[]})])
}

#[test]
fn one_warm_up_at_most() {
    let mut first = numeric("r == a*b");
    first["role"] = json!("warmup");
    let mut second = reference_drill("Ссылку B3 делают абсолютной");
    second["role"] = json!("warmup");
    let client = client(&[
        json!({"groups":[first, second]}),
        json!({"bad":[]}),
        json!({"groups":[]}),
    ]);
    let tree = probe_tree(&client, &prompts(), TEXT, Priority::Foreground).unwrap();
    let warmups = tree
        .groups
        .iter()
        .filter(|g| g.role == Role::Warmup)
        .count();
    assert_eq!(warmups, 1);
    assert!(tree.notes.iter().any(|n| n.contains("вторая разминка")));
}

#[test]
fn a_numeric_branch_gets_more_cases_from_code_without_a_request() {
    let client = client(&[json!({"groups":[numeric("r == a*b")]}), json!({"bad":[]})]);
    let tree = probe_tree(&client, &prompts(), TEXT, Priority::Foreground).unwrap();
    let group = &tree.groups[0];
    let seen: std::collections::HashSet<String> = group
        .cards
        .iter()
        .chain(&group.reserve)
        .map(|c| c.prompt.clone())
        .collect();
    let more = group.fresh_cards(5, 7, &seen);
    assert_eq!(more.len(), 5);
    assert!(more.iter().all(|c| !seen.contains(&c.prompt)), "no repeats");
    for card in &more {
        let (a, b) = card.prompt.split_once('·').unwrap();
        let product = a.parse::<i64>().unwrap() * b.parse::<i64>().unwrap();
        assert_eq!(card.answer, product.to_string(), "code computes the answer");
    }
    assert_eq!(
        client.transport().calls.get(),
        2,
        "no call for the new cases"
    );
    assert!(
        shown_group().fresh_cards(3, 1, &seen).is_empty(),
        "no template, no cases"
    );
}
