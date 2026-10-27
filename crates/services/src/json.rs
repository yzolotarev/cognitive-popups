//! Model answers regularly wrap the requested JSON object in a Markdown fence
//! or a sentence of prose. Removing that wrapper is not a relaxation of the
//! contract: the value is still parsed into a typed struct that rejects missing,
//! extra or empty fields. Only the surrounding noise is dropped.
use serde::de::DeserializeOwned;

pub fn extract<T: DeserializeOwned>(text: &str) -> Result<T, serde_json::Error> {
    let plain = serde_json::from_str(text);
    if plain.is_ok() {
        return plain;
    }
    if let (Some(start), Some(end)) = (text.find('{'), text.rfind('}'))
        && start < end
        && let Ok(value) = serde_json::from_str(&text[start..=end])
    {
        return Ok(value);
    }
    plain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, serde::Deserialize, PartialEq)]
    struct Anchors {
        cues: Vec<String>,
    }

    #[test]
    fn fenced_json_is_unwrapped_but_still_typed() {
        let text = "```json\n{\"cues\": [\"a\", \"b\", \"c\", \"d\"]}\n```";
        let value: Anchors = extract(text).unwrap();
        assert_eq!(value.cues.len(), 4);
    }

    #[test]
    fn json_preceded_by_prose_is_unwrapped() {
        let text = "Here is the plan:\n{\"cues\": [\"a\"]}\nHope it helps.";
        let value: Anchors = extract(text).unwrap();
        assert_eq!(value.cues, ["a"]);
    }

    #[test]
    fn json_with_a_wrong_shape_is_still_rejected() {
        assert!(extract::<Anchors>("```json\n{\"cues\": \"not-a-list\"}\n```").is_err());
        assert!(extract::<Anchors>("no json at all").is_err());
    }
}
