//! Prompts are data (B35): Markdown files whose first ```text block is the prompt.
//! The file name without `.md` is the key; editing a file changes the prompt
//! without a rebuild. This reads the format of `docs/v2/port/prompts/`.

use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum PromptError {
    #[error("cannot read prompts: {0}")]
    Io(#[from] std::io::Error),
    #[error("prompt {0} not found")]
    Missing(String),
}

/// How formulas are written in answers a person reads (B53).
pub const FORMULA_RULE: &str = "NEW_formula_rule";

#[derive(Debug, Default, Clone)]
pub struct PromptStore {
    prompts: BTreeMap<String, String>,
}

impl PromptStore {
    pub fn from_dir(dir: impl AsRef<Path>) -> Result<Self, PromptError> {
        let mut prompts = BTreeMap::new();
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Some(key) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if let Some(text) = first_text_block(&std::fs::read_to_string(&path)?) {
                prompts.insert(key.to_string(), text);
            }
        }
        Ok(Self { prompts })
    }

    pub fn get(&self, key: &str) -> Result<&str, PromptError> {
        self.prompts
            .get(key)
            .map(String::as_str)
            .ok_or_else(|| PromptError::Missing(key.into()))
    }

    /// The prompt with the formula rule appended (B53): for answers a person
    /// reads. Without the rule file the prompt is used as it is.
    pub fn with_formulas(&self, key: &str) -> Result<String, PromptError> {
        let prompt = self.get(key)?;
        Ok(match self.get(FORMULA_RULE) {
            Ok(rule) => format!("{prompt}\n\n{rule}"),
            Err(_) => prompt.to_string(),
        })
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.prompts.keys().map(String::as_str)
    }
}

/// Line endings are normalised first: a Windows checkout turns `\n` into `\r\n`.
fn first_text_block(markdown: &str) -> Option<String> {
    let markdown = &markdown.replace("\r\n", "\n");
    let start = markdown.find("```text\n")? + "```text\n".len();
    let rest = &markdown[start..];
    let end = rest.find("\n```")?;
    let text = rest[..end].trim();
    (!text.is_empty()).then(|| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_text_block_is_the_prompt() {
        let md = "# T\n\n- meta\n\n```text\nСистема.\nВторая строка.\n```\n\n```python\nx\n```\n";
        assert_eq!(first_text_block(md).unwrap(), "Система.\nВторая строка.");
    }

    #[test]
    fn windows_line_endings_are_accepted() {
        let md = "# T\r\n\r\n```text\r\nСистема.\r\nВторая.\r\n```\r\n";
        assert_eq!(first_text_block(md).unwrap(), "Система.\nВторая.");
    }

    #[test]
    fn files_without_a_text_block_are_not_prompts() {
        assert!(first_text_block("# README\n\n| a | b |\n").is_none());
    }

    #[test]
    fn readable_answers_get_the_formula_rule() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/v2/port/prompts");
        let store = PromptStore::from_dir(dir).unwrap();
        let with = store.with_formulas("clarify_system").unwrap();
        assert!(with.contains("\\( … \\)"));
        assert!(with.starts_with(store.get("clarify_system").unwrap()));
        let bare = PromptStore::default();
        assert!(bare.with_formulas("x").is_err());
    }

    #[test]
    fn the_ported_v1_prompts_load() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/v2/port/prompts");
        let store = PromptStore::from_dir(dir).unwrap();
        let seed = store.get("seed_system").unwrap();
        assert!(seed.contains("\"cues\""));
        assert!(store.get("NEW_units_extract").unwrap().contains("REGISTRY"));
        assert!(store.get("README").is_err());
    }
}
