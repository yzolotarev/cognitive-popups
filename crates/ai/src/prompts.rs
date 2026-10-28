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

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.prompts.keys().map(String::as_str)
    }
}

fn first_text_block(markdown: &str) -> Option<String> {
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
    fn files_without_a_text_block_are_not_prompts() {
        assert!(first_text_block("# README\n\n| a | b |\n").is_none());
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
