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

#[test]
fn interactions_are_observed_once_per_operation_and_block() {
    let mut store = Materials::open_in_memory().unwrap();
    let (material, _) = store.register(Some("l1"), None, &sample(), 100).unwrap();
    let marks = vec!["b4".to_string(), "b9".to_string()];
    let n = store
        .record_interaction("l1", "op-1", window::ALT_T, &marks, Some(7), 110)
        .unwrap();
    assert_eq!(n, 1, "b9 is not in the material");
    let again = store
        .record_interaction("l1", "op-1", window::ALT_T, &marks, Some(7), 111)
        .unwrap();
    assert_eq!(again, 0, "one event, one row");
    let elsewhere = store
        .record_interaction("other", "op-2", window::ALT_W, &marks, None, 112)
        .unwrap();
    assert_eq!(elsewhere, 0, "a lesson without a material");
    store
        .record_interaction("l1", "op-3", window::ALT_W, &["b2".into()], Some(9), 110)
        .unwrap();

    let interactions = store.interactions("l1").unwrap();
    let order: Vec<(&str, &str)> = interactions
        .iter()
        .map(|i| (i.block.as_str(), i.window.as_str()))
        .collect();
    assert_eq!(
        order,
        [("b4", "alt_t"), ("b2", "alt_w")],
        "time, then sequence"
    );

    let states = block_states(&material, &interactions);
    let statuses: Vec<&str> = states.iter().map(|s| s.status.as_str()).collect();
    // b4 says nothing about b1 and b3: reading order is not assumed.
    assert_eq!(
        statuses,
        [
            "unknown",
            "observed_interaction",
            "unknown",
            "observed_interaction"
        ]
    );
    assert!(states.iter().all(|s| s.status != BlockStatus::NotRead));
}
