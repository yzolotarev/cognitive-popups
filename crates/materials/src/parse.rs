//! A study text with blocks (B75, B76): visible lines `⟦b2 · механизм⟧` and an
//! invisible mark before every sentence.

use cognitive_exoskeleton_domain::marks;
use serde::Serialize;

/// The roles the prompt `NEW_study_text` allows; anything else is «другое».
pub const ROLES: [&str; 6] = [
    "проблема",
    "определение",
    "механизм",
    "пример",
    "ошибка",
    "обобщение",
];
pub const OTHER_ROLE: &str = "другое";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Block {
    /// `b2`
    pub block: String,
    /// From 1, in the order of the text.
    pub position: i64,
    pub role: String,
    /// Clean text: no tag characters, no marker line, trimmed.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("В тексте нет меток блоков")]
    NoBlocks,
}

/// The text without tag characters, line ends normalised and trimmed: what
/// the material id is computed from.
pub fn clean(text: &str) -> String {
    marks::read(&text.replace("\r\n", "\n"))
        .0
        .trim()
        .to_string()
}

/// Blocks by the visible marker lines; without any, by the invisible marks
/// (role «другое»). Text before the first block (a title) belongs to none.
pub fn parse(text: &str) -> Result<Vec<Block>, ParseError> {
    let text = text.replace("\r\n", "\n");
    let mut blocks: Vec<Block> = Vec::new();
    let mut current: Option<usize> = None;
    let mut found = false;
    for line in text.split_inclusive('\n') {
        let (visible, _) = marks::read(line);
        if let Some((id, role)) = marker(&visible) {
            found = true;
            current = Some(slot(&mut blocks, &id, &role));
        } else if let Some(i) = current {
            blocks[i].text.push_str(&visible);
        }
    }
    if !found {
        for span in marks::spans(&text) {
            match span.mark.as_deref().and_then(block_id) {
                Some(id) => {
                    let i = slot(&mut blocks, &id, OTHER_ROLE);
                    blocks[i].text.push_str(&span.text);
                }
                None => {
                    if let Some(last) = blocks.last_mut() {
                        last.text.push_str(&span.text);
                    }
                }
            }
        }
    }
    for block in &mut blocks {
        block.text = block.text.trim().to_string();
    }
    if blocks.is_empty() {
        return Err(ParseError::NoBlocks);
    }
    Ok(blocks)
}

/// The block's index; a repeated id continues the earlier block.
fn slot(blocks: &mut Vec<Block>, id: &str, role: &str) -> usize {
    if let Some(i) = blocks.iter().position(|b| b.block == id) {
        return i;
    }
    blocks.push(Block {
        block: id.to_string(),
        position: blocks.len() as i64 + 1,
        role: role.to_string(),
        text: String::new(),
    });
    blocks.len() - 1
}

/// `⟦b2 · механизм⟧` → (`b2`, `механизм`). Markdown decoration around the
/// brackets (`**`, `#`) is tolerated; any other text on the line is not.
fn marker(line: &str) -> Option<(String, String)> {
    let start = line.find('⟦')?;
    let end = line.rfind('⟧')?;
    if end < start {
        return None;
    }
    let decoration = |s: &str| s.chars().all(|c| c.is_whitespace() || "#*_>".contains(c));
    if !decoration(&line[..start]) || !decoration(&line[end + '⟧'.len_utf8()..]) {
        return None;
    }
    let inside = line[start + '⟦'.len_utf8()..end].trim();
    let digits_end = inside
        .char_indices()
        .skip(1)
        .find(|(_, c)| !c.is_ascii_digit())
        .map_or(inside.len(), |(i, _)| i);
    let id = block_id(&inside[..digits_end])?;
    let rest = inside[digits_end..]
        .trim_start()
        .trim_start_matches(['·', '-', ':', '—', '–', '.', '•'])
        .trim()
        .to_lowercase();
    let role = if ROLES.contains(&rest.as_str()) {
        rest
    } else {
        OTHER_ROLE.to_string()
    };
    Some((id, role))
}

/// `b2` / `B2` → `b2`; anything else is not a block id.
fn block_id(text: &str) -> Option<String> {
    let text = text.trim();
    let digits = text.strip_prefix(['b', 'B'])?;
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n: u32 = digits.parse().ok()?;
    Some(format!("b{n}"))
}
