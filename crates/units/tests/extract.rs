//! End of a lesson without a network: journal → stub model → checks → store.

use std::cell::{Cell, RefCell};
use std::path::Path;

use cognitive_exoskeleton_ai::{AiConfig, Client, PromptStore, ProviderKind, Transport};
use cognitive_exoskeleton_domain::{LessonId, NewEvent, OperationId};
use cognitive_exoskeleton_journal::Journal;
use cognitive_exoskeleton_units::{UnitStore, extract_lesson};
use serde_json::{Value, json};
use time::macros::datetime;

struct Stub {
    answers: RefCell<Vec<String>>,
    calls: Cell<usize>,
    last_input: RefCell<String>,
}

impl Stub {
    fn new(answers: &[Value]) -> Self {
        Self {
            answers: RefCell::new(answers.iter().rev().map(Value::to_string).collect()),
            calls: Cell::new(0),
            last_input: RefCell::new(String::new()),
        }
    }
}

impl Transport for Stub {
    fn post(&self, _: &str, _: &[(&str, String)], body: &Value) -> Result<(u16, String), String> {
        self.calls.set(self.calls.get() + 1);
        *self.last_input.borrow_mut() = body["contents"][0]["parts"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let text = self.answers.borrow_mut().pop().expect("unexpected call");
        let reply = json!({"candidates":[{"content":{"parts":[{"text": text}]}}]});
        Ok((200, reply.to_string()))
    }
}

fn client(answers: &[Value]) -> Client<Stub> {
    let config = AiConfig::new(ProviderKind::Gemini, "m", None).with_key(Some("k".into()));
    Client::new(config, Stub::new(answers))
}

fn prompts() -> PromptStore {
    PromptStore::from_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/v2/port/prompts"))
        .unwrap()
}

fn lesson_with(journal: &mut Journal, lesson: &str, user_text: Option<&str>) -> LessonId {
    let id = LessonId::new(lesson).unwrap();
    let op = OperationId::new(format!("op-{lesson}")).unwrap();
    let at = datetime!(2026-10-28 09:00 UTC);
    let mut events = vec![
        NewEvent::ai_response(
            op.clone(),
            at,
            format!("{lesson}-ai"),
            "p",
            "m",
            "Задача: ранг?",
        )
        .unwrap()
        .with_lesson(id.clone()),
    ];
    if let Some(text) = user_text {
        events.push(
            NewEvent::user_text(op, at, format!("{lesson}-user"), text)
                .unwrap()
                .with_lesson(id.clone()),
        );
    }
    journal.append(&events).unwrap();
    id
}

#[test]
fn a_lesson_becomes_checked_units_once() {
    let mut journal = Journal::open_in_memory().unwrap();
    let lesson = lesson_with(&mut journal, "L1", Some("ранг 1"));
    let mut store = UnitStore::open_in_memory().unwrap();
    let answer = json!({"units":[
        {"title":"Ранг","action":"вычислить ранг","kind":"execute","events":["e1","e2","e42"],
         "same_as":null,"why":"новая"},
        {"title":"Только ИИ","action":"прочитать задачу","kind":"recall","events":["e1"],
         "same_as":null,"why":"нет действия"}],
        "merge_candidates":[]});
    let client = client(&[answer]);

    let (summary, extraction) =
        extract_lesson(&journal, &mut store, &client, &prompts(), "S", &lesson).unwrap();
    assert_eq!(summary.new_units, 1);
    assert_eq!(extraction.notes.len(), 2, "{:?}", extraction.notes);
    let input = client_input(&client);
    assert!(input.contains("[e2] ->user: ранг 1"), "{input}");
    assert!(input.contains("REGISTRY:\n(пусто)"), "{input}");
    let unit = &store.registry("S").unwrap()[0];
    assert_eq!(store.events_of(&unit.id).unwrap(), vec![1, 2]);

    let (again, _) =
        extract_lesson(&journal, &mut store, &client, &prompts(), "S", &lesson).unwrap();
    assert_eq!(again, summary);
    assert_eq!(
        client_calls(&client),
        1,
        "a repeated extraction costs no request"
    );
}

#[test]
fn a_lesson_without_the_readers_actions_costs_no_request() {
    let mut journal = Journal::open_in_memory().unwrap();
    let lesson = lesson_with(&mut journal, "L1", None);
    let mut store = UnitStore::open_in_memory().unwrap();
    let client = client(&[]);
    let (summary, _) =
        extract_lesson(&journal, &mut store, &client, &prompts(), "S", &lesson).unwrap();
    assert_eq!(summary.notification(), None);
    assert_eq!(client_calls(&client), 0);
}

fn client_calls(client: &Client<Stub>) -> usize {
    client_transport(client).calls.get()
}

fn client_input(client: &Client<Stub>) -> String {
    client_transport(client).last_input.borrow().clone()
}

fn client_transport(client: &Client<Stub>) -> &Stub {
    client.transport()
}
