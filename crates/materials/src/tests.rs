use cognitive_exoskeleton_domain::marks::tagged;

use super::*;

/// A study text as ChatGPT writes it: a title, visible marker lines and an
/// invisible mark before every sentence (B76), some on the marker lines too.
fn sample() -> String {
    let t = |m: &str| tagged(m);
    format!(
        "Ожидаемый убыток\n\n\
         ⟦b1 · проблема⟧\n\
         {b1}Сколько в среднем теряет страховщик на одном полисе? {b1}Это нужно для цены.\n\n\
         **⟦b2 · Механизм⟧**\n\
         {b2}Каждый исход умножаем на его вероятность. {b2}Произведения складываем.\n\n\
         {b3}⟦b3 - пример⟧\n\
         {b3}E(L) = 0,3·25 + 0,7·0 = 7,5.\n\n\
         ⟦ b4 : вывод ⟧\n\
         {b4}Среднее — не то, что случится в одном полисе.\n",
        b1 = t("b1"),
        b2 = t("b2"),
        b3 = t("b3"),
        b4 = t("b4"),
    )
}

#[test]
fn visible_markers_split_the_text_into_blocks_with_roles() {
    let blocks = parse(&sample()).unwrap();
    let ids: Vec<&str> = blocks.iter().map(|b| b.block.as_str()).collect();
    assert_eq!(ids, ["b1", "b2", "b3", "b4"]);
    let roles: Vec<&str> = blocks.iter().map(|b| b.role.as_str()).collect();
    assert_eq!(roles, ["проблема", "механизм", "пример", OTHER_ROLE]);
    assert_eq!(
        blocks[0].text,
        "Сколько в среднем теряет страховщик на одном полисе? Это нужно для цены."
    );
    assert_eq!(blocks[2].text, "E(L) = 0,3·25 + 0,7·0 = 7,5.");
    assert_eq!(blocks[3].position, 4);
    assert!(blocks.iter().all(|b| !b.text.contains('⟦')));
}

#[test]
fn without_visible_markers_the_hidden_marks_group_the_sentences() {
    let text = format!(
        "{b1}Первое. {b1}Второе.\n\n{b2}Третье. {b2}Четвёртое.",
        b1 = tagged("b1"),
        b2 = tagged("b2")
    );
    let blocks = parse(&text).unwrap();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].text, "Первое. Второе.");
    assert_eq!(blocks[1].text, "Третье. Четвёртое.");
    assert!(blocks.iter().all(|b| b.role == OTHER_ROLE));
}

#[test]
fn a_text_without_blocks_is_refused() {
    for garbage in [
        "",
        "Просто текст.\nБез меток.",
        "⟦не блок⟧",
        "текст ⟦b1 · пример⟧ дальше",
    ] {
        assert_eq!(parse(garbage), Err(ParseError::NoBlocks), "{garbage:?}");
    }
    assert_eq!(
        ParseError::NoBlocks.to_string(),
        "В тексте нет меток блоков"
    );
}

#[test]
fn the_same_text_registers_once_and_a_new_text_keeps_the_old() {
    let mut store = Materials::open_in_memory().unwrap();
    let (first, new) = store
        .register(Some("l1"), Some("s1"), &sample(), 100)
        .unwrap();
    assert!(new);
    assert_eq!(first.id.len(), 16);
    assert_eq!(
        first.summary(),
        "Материал: 4 блока · проблема, механизм, пример, другое"
    );
    let (again, new) = store
        .register(Some("l1"), Some("s1"), &sample(), 200)
        .unwrap();
    assert!(!new);
    assert_eq!(again, first);

    let other = format!("⟦b1 · пример⟧\n{}Другой текст.", tagged("b1"));
    let (second, new) = store.register(Some("l1"), Some("s1"), &other, 300).unwrap();
    assert!(new);
    assert_ne!(second.id, first.id);
    assert_eq!(store.latest_for_lesson("l1").unwrap(), Some(second));
    assert!(store.material(&first.id).unwrap().is_some());
    assert!(store.register(Some("l1"), None, "без меток", 400).is_err());
}

fn part(block: Option<&str>, text: &str) -> Part {
    Part {
        block: block.map(str::to_string),
        text: text.to_string(),
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn overlapping_fragments_join_without_duplication() {
    let a = "Каждый исход умножаем на его вероятность, затем";
    let b = "на его вероятность, затем произведения складываем.";
    assert_eq!(
        merge(&strings(&[a, b])),
        ["Каждый исход умножаем на его вероятность, затем произведения складываем."]
    );
    // The other way round gives the same text.
    assert_eq!(merge(&strings(&[b, a])), merge(&strings(&[a, b])));
}

#[test]
fn a_contained_fragment_disappears() {
    let whole = "Каждый исход умножаем на его вероятность.";
    assert_eq!(merge(&strings(&["его вероятность", whole])), [whole]);
    assert_eq!(merge(&strings(&[whole, "умножаем", whole])), [whole]);
}

#[test]
fn disjoint_fragments_stay_apart_in_first_seen_order() {
    let first = "Произведения складываем.";
    let second = "Каждый исход умножаем на его вероятность.";
    assert_eq!(merge(&strings(&[first, second])), [first, second]);
    // A short overlap («ость.» = 5 characters) is not enough to join.
    let short = "ость. И ещё одно предложение.";
    assert_eq!(merge(&strings(&[second, short])), [second, short]);
}

#[test]
fn a_bridge_joins_two_earlier_pieces() {
    let left = "Первая часть длинного текста про ожидаемый";
    let right = "убыток страховщика и его цену для клиента.";
    let bridge = "длинного текста про ожидаемый убыток страховщика и его цену";
    assert_eq!(
        merge(&strings(&[left, right, bridge])),
        ["Первая часть длинного текста про ожидаемый убыток страховщика и его цену для клиента."]
    );
}

#[test]
fn selections_rebuild_the_material_and_unseen_blocks_are_unknown() {
    let mut store = Materials::open_in_memory().unwrap();
    let a = "Каждый исход умножаем на его вероятность, затем";
    let b = "на его вероятность, затем произведения складываем.";
    let select = |store: &mut Materials, op: &str, window: &str, block: &str, text: &str, at| {
        store
            .record_selection("l1", op, window, &[part(Some(block), text)], None, at)
            .unwrap()
    };
    assert_eq!(
        select(
            &mut store,
            "op-1",
            window::ALT_T,
            "b4",
            "Среднее — не одно событие.",
            100
        ),
        1
    );
    assert_eq!(
        select(
            &mut store,
            "op-1",
            window::ALT_T,
            "b4",
            "Среднее — не одно событие.",
            101
        ),
        0,
        "one event, one row"
    );
    select(&mut store, "op-2", window::ALT_W, "b2", a, 102);
    select(&mut store, "op-3", window::CTRL_ALT_P, "b2", b, 103);

    let order: Vec<(String, String)> = store
        .interactions("l1")
        .unwrap()
        .into_iter()
        .map(|i| (i.block.unwrap(), i.window))
        .collect();
    assert_eq!(
        order,
        [
            ("b4".into(), "alt_t".into()),
            ("b2".into(), "alt_w".into()),
            ("b2".into(), "ctrl_alt_p".into())
        ]
    );

    let views = store.reconstruct("l1").unwrap();
    let ids: Vec<&str> = views.iter().map(|v| v.block.as_str()).collect();
    assert_eq!(ids, ["b1", "b2", "b3", "b4"], "b1…bmax, nothing beyond");
    let coverage: Vec<Coverage> = views.iter().map(|v| v.coverage).collect();
    assert_eq!(
        coverage,
        [
            Coverage::Unknown,
            Coverage::Partial,
            Coverage::Unknown,
            Coverage::Partial
        ]
    );
    assert_eq!(
        views[1].fragments,
        ["Каждый исход умножаем на его вероятность, затем произведения складываем."]
    );
    assert!(views[0].fragments.is_empty() && views[0].role.is_none());
    // b4 says nothing about b1 and b3: reading order is not assumed.
    let statuses: Vec<&str> = views.iter().map(|v| v.status.as_str()).collect();
    assert_eq!(
        statuses,
        [
            "unknown",
            "observed_interaction",
            "unknown",
            "observed_interaction"
        ]
    );
    assert!(views.iter().all(|v| v.status != BlockStatus::NotRead));
    assert_eq!(views[1].interactions, 2);
    assert!(store.reconstruct("other").unwrap().is_empty());
}

#[test]
fn a_registered_text_makes_its_blocks_known() {
    let mut store = Materials::open_in_memory().unwrap();
    store.register(Some("l1"), None, &sample(), 100).unwrap();
    store
        .record_selection(
            "l1",
            "op-1",
            window::ALT_T,
            &[
                part(Some("b3"), "E(L)"),
                part(Some("b6"), "Блок из другого места."),
            ],
            None,
            110,
        )
        .unwrap();
    let views = store.reconstruct("l1").unwrap();
    let ids: Vec<&str> = views.iter().map(|v| v.block.as_str()).collect();
    assert_eq!(ids, ["b1", "b2", "b3", "b4", "b5", "b6"]);
    assert_eq!(views[1].coverage, Coverage::Known);
    assert_eq!(views[1].role.as_deref(), Some("механизм"));
    assert_eq!(views[2].fragments, ["E(L) = 0,3·25 + 0,7·0 = 7,5."]);
    assert_eq!(views[4].coverage, Coverage::Unknown);
    assert_eq!(views[5].coverage, Coverage::Partial);
    assert_eq!(views[2].status, BlockStatus::ObservedInteraction);
    assert_eq!(views[0].status, BlockStatus::Unknown);
}

/// A selection of `text` with its marks, attributed and recorded.
fn select_marked(store: &mut Materials, op: &str, text: &str) -> Vec<Part> {
    let (clean, marks) = cognitive_exoskeleton_domain::marks::positions(text);
    let parts = store.attribute("l1", &clean, &marks).unwrap();
    store
        .record_selection("l1", op, window::ALT_W, &parts, None, 200)
        .unwrap();
    parts
}

#[test]
fn a_mid_sentence_selection_is_found_in_the_registered_text() {
    let mut store = Materials::open_in_memory().unwrap();
    store.register(Some("l1"), None, &sample(), 100).unwrap();
    // No mark: the selection starts after «Каждый ».
    let parts = select_marked(&mut store, "op-1", "исход   умножаем\u{00AD} на его");
    assert_eq!(parts, [part(Some("b2"), "исход   умножаем\u{00AD} на его")]);
    let interactions = store.interactions("l1").unwrap();
    assert_eq!(interactions[0].block.as_deref(), Some("b2"));
}

#[test]
fn a_mid_sentence_selection_is_found_in_the_reconstructed_fragments() {
    let mut store = Materials::open_in_memory().unwrap();
    let earlier = format!("{}Каждый исход умножаем на его вероятность.", tagged("b2"));
    select_marked(&mut store, "op-1", &earlier);
    let parts = select_marked(&mut store, "op-2", "умножаем на его");
    assert_eq!(parts[0].block.as_deref(), Some("b2"));
}

#[test]
fn a_selection_across_a_sentence_boundary_has_two_blocks() {
    let mut store = Materials::open_in_memory().unwrap();
    store.register(Some("l1"), None, &sample(), 100).unwrap();
    // The end of b1's last sentence, then b2's first one with its mark.
    let text = format!(
        "нужно для цены.\n\n⟦b2 · Механизм⟧\n{}Каждый исход",
        tagged("b2")
    );
    let parts = select_marked(&mut store, "op-1", &text);
    let blocks: Vec<Option<&str>> = parts.iter().map(|p| p.block.as_deref()).collect();
    assert_eq!(blocks, [Some("b1"), Some("b2")]);
    assert_eq!(parts[1].text, "Каждый исход");
    let views = store.reconstruct("l1").unwrap();
    assert_eq!(views[0].status, BlockStatus::ObservedInteraction);
    assert_eq!(views[1].status, BlockStatus::ObservedInteraction);
}

#[test]
fn without_a_unique_match_the_block_stays_unknown() {
    let mut store = Materials::open_in_memory().unwrap();
    store.register(Some("l1"), None, &sample(), 100).unwrap();
    // Nowhere in the text.
    let parts = select_marked(&mut store, "op-1", "совсем другой текст");
    assert_eq!(parts, [part(None, "совсем другой текст")]);
    // In two blocks («полис» is in b1 and b4).
    let parts = select_marked(&mut store, "op-2", "полис");
    assert_eq!(parts[0].block, None);
    let interactions = store.interactions("l1").unwrap();
    assert_eq!(interactions.len(), 2);
    assert!(
        interactions.iter().all(|i| i.block.is_none()),
        "the gap is kept"
    );
    // Nothing is stored as a fragment and nothing changes in the blocks.
    let views = store.reconstruct("l1").unwrap();
    assert!(views.iter().all(|v| v.status == BlockStatus::Unknown));
}

#[test]
fn split_cuts_at_marks_and_joins_one_block() {
    let text = format!(
        "хвост. {}Один. {}Два. {}Три.",
        tagged("b1"),
        tagged("b1"),
        tagged("b2")
    );
    let (clean, marks) = cognitive_exoskeleton_domain::marks::positions(&text);
    assert_eq!(
        split(&clean, &marks),
        [
            part(None, "хвост."),
            part(Some("b1"), "Один. Два."),
            part(Some("b2"), "Три.")
        ]
    );
    assert_eq!(normalize(" a\u{00AD}b \n\t c "), "ab c");
}
