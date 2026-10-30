//! B76: invisible block marks in a study text. An outside AI puts the block id
//! (`b2`) spelled in Unicode tag characters (U+E0020–U+E007E) before every
//! sentence. One decoder for the capture (`capture::read_marks`) and the
//! material registry (`materials::parse`).

/// A run of visible text and the mark that came before it (`None` before the
/// first mark).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub mark: Option<String>,
    pub text: String,
}

/// The text cut at every mark, with all tag characters removed.
pub fn spans(text: &str) -> Vec<Span> {
    let mut spans = vec![Span {
        mark: None,
        text: String::new(),
    }];
    let mut current = String::new();
    let close = |current: &mut String, spans: &mut Vec<Span>| {
        if !current.is_empty() {
            spans.push(Span {
                mark: Some(std::mem::take(current)),
                text: String::new(),
            });
        }
    };
    for c in text.chars() {
        match c as u32 {
            0xE0020..=0xE007E => current.push(char::from((c as u32 - 0xE0000) as u8)),
            0xE0000..=0xE007F => close(&mut current, &mut spans),
            _ => {
                close(&mut current, &mut spans);
                if let Some(last) = spans.last_mut() {
                    last.text.push(c);
                }
            }
        }
    }
    close(&mut current, &mut spans);
    if spans[0].text.is_empty() {
        spans.remove(0);
    }
    spans
}

/// The text without any tag characters and the distinct marks in order of
/// appearance.
pub fn read(text: &str) -> (String, Vec<String>) {
    let mut clean = String::with_capacity(text.len());
    let mut marks: Vec<String> = Vec::new();
    for span in spans(text) {
        clean.push_str(&span.text);
        if let Some(mark) = span.mark
            && !marks.contains(&mark)
        {
            marks.push(mark);
        }
    }
    (clean, marks)
}

/// The clean text and where each mark starts in it, as `(character offset,
/// mark)`. A mark sits at the start of a sentence, so the text before the
/// first one (a selection begun mid-sentence) has no mark of its own (B78).
pub fn positions(text: &str) -> (String, Vec<(usize, String)>) {
    let mut clean = String::with_capacity(text.len());
    let mut chars = 0;
    let mut at = Vec::new();
    for span in spans(text) {
        if let Some(mark) = span.mark {
            at.push((chars, mark));
        }
        chars += span.text.chars().count();
        clean.push_str(&span.text);
    }
    (clean, at)
}

/// `mark` spelled in tag characters (tests, demo data).
pub fn tagged(mark: &str) -> String {
    mark.chars()
        .filter(|c| (' '..='~').contains(c))
        .filter_map(|c| char::from_u32(0xE0000 + c as u32))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_follow_the_marks() {
        let text = format!("Заголовок {}Один. {}Два.", tagged("b1"), tagged("b2"));
        let spans = spans(&text);
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].mark, None);
        assert_eq!(spans[1].mark.as_deref(), Some("b1"));
        assert_eq!(spans[1].text, "Один. ");
        assert_eq!(spans[2].text, "Два.");
        assert_eq!(
            read(&text),
            (
                "Заголовок Один. Два.".into(),
                vec!["b1".into(), "b2".into()]
            )
        );
        assert_eq!(read("без меток"), ("без меток".into(), Vec::new()));
    }

    #[test]
    fn positions_are_character_offsets_in_the_clean_text() {
        let text = format!("ёж шёл. {}Один. {}Два.", tagged("b1"), tagged("b2"));
        let (clean, at) = positions(&text);
        assert_eq!(clean, "ёж шёл. Один. Два.");
        assert_eq!(at, [(8, "b1".to_string()), (14, "b2".to_string())]);
        assert_eq!(positions("без меток"), ("без меток".into(), Vec::new()));
    }
}
