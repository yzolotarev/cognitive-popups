//! Alt+W «помочь понять» (S7, F-02…F-10): one key, five kinds of help chosen by
//! weight. The model writes the help; the code checks its shape, keeps quotes
//! verbatim, compares the reader's own four words and never grades anyone.

mod store;
mod words;

use cognitive_exoskeleton_ai::{AiError, Client, Priority, PromptStore, Request, Transport};
use serde::Serialize;
use serde_json::Value;

pub use store::{Store, StoreError};
pub use words::{Comparison, compare, own_words, stem};

/// The five kinds of help (F-02).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mechanic {
    Words,
    Explain,
    Example,
    Ladder,
    Character,
}

impl Mechanic {
    pub const ALL: [Mechanic; 5] = [
        Mechanic::Words,
        Mechanic::Explain,
        Mechanic::Example,
        Mechanic::Ladder,
        Mechanic::Character,
    ];

    /// Weights of v1, kept by the owner (Q-04): 70/10/10/5/5.
    pub fn weight(self) -> u32 {
        match self {
            Mechanic::Words => 70,
            Mechanic::Explain | Mechanic::Example => 10,
            Mechanic::Ladder | Mechanic::Character => 5,
        }
    }

    /// The window title (F-71: an error is titled by its function).
    pub fn title(self) -> &'static str {
        match self {
            Mechanic::Words => "4 слова",
            Mechanic::Explain => "Объяснение",
            Mechanic::Example => "Покажи на примере",
            Mechanic::Ladder => "Ступенька",
            Mechanic::Character => "Мужичок",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Mechanic::Words => "words",
            Mechanic::Explain => "explain",
            Mechanic::Example => "example",
            Mechanic::Ladder => "ladder",
            Mechanic::Character => "character",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.name() == name)
    }
}

/// Picks a mechanic by weight among the ones not switched off; `roll` is
/// uniform in `[0, 1)`. Without text nothing is available (`no_material`).
pub fn pick(off: &[Mechanic], has_text: bool, roll: f64) -> Option<Mechanic> {
    if !has_text {
        return None;
    }
    let open: Vec<Mechanic> = Mechanic::ALL
        .into_iter()
        .filter(|m| !off.contains(m))
        .collect();
    let total: u32 = open.iter().map(|m| m.weight()).sum();
    if total == 0 {
        return None;
    }
    let mut point = roll.clamp(0.0, 0.999_999) * f64::from(total);
    for mechanic in &open {
        let weight = f64::from(mechanic.weight());
        if point < weight {
            return Some(*mechanic);
        }
        point -= weight;
    }
    open.last().copied()
}

/// A roll from the system's randomness, without a dependency.
pub fn roll() -> f64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    (hasher.finish() >> 11) as f64 / (1u64 << 53) as f64
}

#[derive(Debug, thiserror::Error)]
pub enum UnderstandError {
    #[error(transparent)]
    Ai(#[from] AiError),
    #[error("prompt: {0}")]
    Prompt(String),
    /// F-02: «Выдели материал для помощи».
    #[error("нет материала")]
    NoMaterial,
}

impl UnderstandError {
    /// One line for the window: what failed and one thing to do (F-71).
    pub fn user_message(&self, mechanic: Mechanic, provider: &str) -> String {
        match self {
            UnderstandError::NoMaterial => match mechanic {
                Mechanic::Ladder => "Выдели трудное место.".into(),
                _ => "Выдели материал для помощи.".into(),
            },
            UnderstandError::Ai(AiError::NoKey) => AiError::NoKey.user_message(provider),
            UnderstandError::Ai(AiError::InvalidJson(_)) if mechanic == Mechanic::Words => {
                "Сбой системы: не удалось собрать четыре слова. Попробуйте ещё раз.".into()
            }
            UnderstandError::Ai(error) => error.user_message(provider),
            UnderstandError::Prompt(error) => format!("Промпт не найден: {error}"),
        }
    }
}

pub const SEED_PROMPT: &str = "seed_system";
pub const CLARIFY_PROMPT: &str = "clarify_system";
pub const QUESTION_PROMPT: &str = "clarify_question_system";
pub const EXAMPLE_PROMPT: &str = "example_system";
pub const LADDER_PROMPT: &str = "ladder_system";
pub const CHARACTER_PROMPT: &str = "character_system";

/// v1 `MAX_CLARIFY_TERMS`, `CLARIFY_WORDS`, `LADDER_KNOWN`.
pub const MAX_TERMS: usize = 4;
pub const TERM_WORDS: usize = 80;
pub const LADDER_KNOWN: usize = 8;

fn system<'a>(prompts: &'a PromptStore, key: &str) -> Result<&'a str, UnderstandError> {
    prompts
        .get(key)
        .map_err(|e| UnderstandError::Prompt(e.to_string()))
}

fn material(text: &str) -> Result<&str, UnderstandError> {
    let text = text.trim();
    if text.is_empty() {
        Err(UnderstandError::NoMaterial)
    } else {
        Ok(text)
    }
}

fn request(system: &str, input: String, max_tokens: u32) -> Request {
    Request {
        system: system.to_string(),
        input,
        max_tokens,
        json: true,
        priority: Priority::Foreground,
    }
}

// ---------- W1: four words ----------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Cue {
    pub simple: String,
    pub term: String,
    pub meaning: String,
}

pub fn cues_check(value: &Value) -> Result<(), String> {
    let cues = value["cues"].as_array().ok_or("нет массива cues")?;
    if cues.len() != 4 {
        return Err(format!("нужно ровно 4 объекта, пришло {}", cues.len()));
    }
    for (i, cue) in cues.iter().enumerate() {
        let field = |name: &str| {
            cue[name]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or(format!("cues[{i}].{name} пусто"))
        };
        let simple = field("simple")?;
        field("term")?;
        let meaning = field("meaning")?;
        if simple.split_whitespace().count() != 1 {
            return Err(format!("cues[{i}].simple должно быть ровно одним словом"));
        }
        if meaning.split_whitespace().count() > 12 {
            return Err(format!("cues[{i}].meaning длиннее двенадцати слов"));
        }
    }
    Ok(())
}

/// F-03: exactly four `{simple, term, meaning}`; one repair with the reason.
pub fn words<T: Transport>(
    client: &Client<T>,
    prompts: &PromptStore,
    text: &str,
    priority: Priority,
) -> Result<Vec<Cue>, UnderstandError> {
    let text = material(text)?;
    let mut request = request(system(prompts, SEED_PROMPT)?, text.to_string(), 500);
    request.priority = priority;
    let value = client.complete_json(&request, cues_check)?;
    serde_json::from_value(value["cues"].clone())
        .map_err(|e| UnderstandError::Ai(AiError::InvalidJson(e.to_string())))
}

// ---------- W2: explain words or answer a question ----------

/// v1 `looks_like_question` (F-07).
pub fn looks_like_question(input: &str) -> bool {
    let text = input.trim().to_lowercase();
    const STARTS: [&str; 17] = [
        "что",
        "почему",
        "зачем",
        "как",
        "когда",
        "где",
        "куда",
        "откуда",
        "кто",
        "какой",
        "какая",
        "какое",
        "какие",
        "сколько",
        "чем",
        "ли",
        "разве",
    ];
    const PHRASES: [&str; 5] = ["если бы", "можно ли", "стоит ли", "правда ли", "что такое"];
    let first = text
        .split(|c: char| !c.is_alphanumeric())
        .find(|w| !w.is_empty())
        .unwrap_or("");
    text.ends_with('?')
        || PHRASES.iter().any(|p| text.contains(p))
        || (STARTS.contains(&first) && !text.contains(','))
}

/// Terms separated by commas: the first four, and the rest as «Упущено — …».
pub fn split_terms(input: &str) -> (Vec<String>, Vec<String>) {
    let mut terms: Vec<String> = Vec::new();
    for term in input.split([',', ';', '\n']) {
        let term = term.trim();
        if !term.is_empty() && !terms.iter().any(|t| t.eq_ignore_ascii_case(term)) {
            terms.push(term.to_string());
        }
    }
    let omitted = terms.split_off(terms.len().min(MAX_TERMS));
    (terms, omitted)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TermLine {
    pub term: String,
    pub explanation: String,
    /// Cut to 80 words by the code: «Сокращено до 80 слов — …».
    pub shortened: bool,
    /// The term is not in the text: «Опора для объяснения не найдена в тексте.»
    pub in_text: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Explanation {
    pub lines: Vec<TermLine>,
    pub omitted: Vec<String>,
}

fn clarify_check(terms: &[String]) -> impl Fn(&Value) -> Result<(), String> + '_ {
    move |value| {
        let lines = value["lines"].as_array().ok_or("нет массива lines")?;
        if lines.is_empty() {
            return Err("lines пусто".into());
        }
        for (i, line) in lines.iter().enumerate() {
            let term = line["term"]
                .as_str()
                .ok_or(format!("lines[{i}].term нет"))?;
            if !terms
                .iter()
                .any(|t| t.trim().eq_ignore_ascii_case(term.trim()))
            {
                return Err(format!("lines[{i}].term «{term}» нет в UNKNOWN_TERMS"));
            }
            let explanation = line["explanation"].as_str().unwrap_or("").trim();
            if explanation.is_empty() {
                return Err(format!("lines[{i}].explanation пусто"));
            }
        }
        Ok(())
    }
}

pub fn explain_terms<T: Transport>(
    client: &Client<T>,
    prompts: &PromptStore,
    text: &str,
    input: &str,
) -> Result<Explanation, UnderstandError> {
    let text = material(text)?;
    let (terms, omitted) = split_terms(input);
    if terms.is_empty() {
        return Err(UnderstandError::NoMaterial);
    }
    let input_text = format!(
        "<SOURCE_BUFFER>\n{text}\n</SOURCE_BUFFER>\n<UNKNOWN_TERMS>\n{}\n</UNKNOWN_TERMS>",
        terms.join(", ")
    );
    let value = client.complete_json(
        &request(system(prompts, CLARIFY_PROMPT)?, input_text, 800),
        clarify_check(&terms),
    )?;
    let mut lines = Vec::new();
    for line in value["lines"].as_array().into_iter().flatten() {
        let term = line["term"].as_str().unwrap_or_default().trim().to_string();
        if lines
            .iter()
            .any(|l: &TermLine| l.term.eq_ignore_ascii_case(&term))
        {
            continue;
        }
        let words: Vec<&str> = line["explanation"]
            .as_str()
            .unwrap_or_default()
            .split_whitespace()
            .collect();
        let shortened = words.len() > TERM_WORDS;
        lines.push(TermLine {
            in_text: words::appears_in(&term, text),
            explanation: words[..words.len().min(TERM_WORDS)].join(" "),
            shortened,
            term,
        });
    }
    // A term the model skipped is said, not hidden.
    let mut omitted = omitted;
    for term in &terms {
        if !lines.iter().any(|l| l.term.eq_ignore_ascii_case(term)) {
            omitted.insert(0, term.clone());
        }
    }
    Ok(Explanation { lines, omitted })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Grounding {
    InText,
    PartlyInText,
    NotInText,
}

impl Grounding {
    /// The support mark the answer starts with (F-07).
    pub fn label(self) -> &'static str {
        match self {
            Grounding::InText => "Из текста.",
            Grounding::PartlyInText => "Часть — из текста, часть — общий вывод.",
            Grounding::NotInText => "Этого в тексте нет: общий ответ.",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Answer {
    pub grounding: Grounding,
    pub answer: String,
    /// Verbatim from the text, or nothing: the code drops a paraphrase.
    pub basis: Option<String>,
}

fn question_check(value: &Value) -> Result<(), String> {
    match value["grounding"].as_str() {
        Some("in_text" | "partly_in_text" | "not_in_text") => {}
        _ => return Err("grounding: in_text|partly_in_text|not_in_text".into()),
    }
    if value["answer"].as_str().unwrap_or("").trim().is_empty() {
        return Err("answer пусто".into());
    }
    Ok(())
}

pub fn answer_question<T: Transport>(
    client: &Client<T>,
    prompts: &PromptStore,
    text: &str,
    question: &str,
) -> Result<Answer, UnderstandError> {
    let text = material(text)?;
    let input = format!(
        "<SOURCE_BUFFER>\n{text}\n</SOURCE_BUFFER>\n<READER_QUESTION>\n{}\n</READER_QUESTION>",
        question.trim()
    );
    let value = client.complete_json(
        &request(system(prompts, QUESTION_PROMPT)?, input, 800),
        question_check,
    )?;
    let basis = value["basis"]
        .as_str()
        .and_then(|b| verbatim(b, text))
        .filter(|b| !b.is_empty());
    let grounding = match value["grounding"].as_str() {
        Some("in_text") if basis.is_some() => Grounding::InText,
        // «In the text» with no quote that is really there is not in the text.
        Some("in_text" | "partly_in_text") => Grounding::PartlyInText,
        _ => Grounding::NotInText,
    };
    Ok(Answer {
        grounding,
        answer: value["answer"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string(),
        basis,
    })
}

/// The quote as it stands in the text, if it is there (spaces and outer
/// quotation marks aside).
pub fn verbatim(quote: &str, text: &str) -> Option<String> {
    let squash = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let quote = squash(quote.trim().trim_matches(['«', '»', '"', '“', '”']));
    if quote.is_empty() {
        return None;
    }
    squash(text).contains(&quote).then_some(quote)
}

// ---------- W3: example ----------

pub fn example<T: Transport>(
    client: &Client<T>,
    prompts: &PromptStore,
    text: &str,
    user_request: Option<&str>,
) -> Result<String, UnderstandError> {
    let user_request = user_request.map(str::trim).filter(|r| !r.is_empty());
    if text.trim().is_empty() && user_request.is_none() {
        return Err(UnderstandError::NoMaterial);
    }
    let mut input = format!("<MATERIAL>\n{}\n</MATERIAL>", text.trim());
    if let Some(r) = user_request {
        input.push_str(&format!("\n<USER_REQUEST>\n{r}\n</USER_REQUEST>"));
    }
    let mut request = request(system(prompts, EXAMPLE_PROMPT)?, input, 800);
    request.json = false;
    let answer = client.complete(&request)?;
    let answer = answer.trim();
    if answer.is_empty() {
        return Err(UnderstandError::Ai(AiError::Shape(
            "empty model response".into(),
        )));
    }
    Ok(answer.to_string())
}

// ---------- W4: ladder ----------

/// What already holds for the reader, with its mark (F-09).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Known {
    pub mark: &'static str,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Rung {
    pub from: String,
    pub to: String,
    pub text: String,
}

fn ladder_check(value: &Value) -> Result<(), String> {
    for field in ["from", "to", "text"] {
        if value[field].as_str().unwrap_or("").trim().is_empty() {
            return Err(format!("{field} пусто"));
        }
    }
    Ok(())
}

pub fn ladder<T: Transport>(
    client: &Client<T>,
    prompts: &PromptStore,
    target: &str,
    around: &str,
    known: &[Known],
) -> Result<Rung, UnderstandError> {
    let target = material(target)?;
    let known: Vec<String> = known
        .iter()
        .take(LADDER_KNOWN)
        .map(|k| format!("[{}] {}", k.mark, k.text.replace('\n', " ")))
        .collect();
    let input = format!(
        "<TARGET>\n{target}\n</TARGET>\n<AROUND>\n{}\n</AROUND>\n<KNOWN>\n{}\n</KNOWN>",
        around.trim(),
        known.join("\n")
    );
    let value = client.complete_json(
        &request(system(prompts, LADDER_PROMPT)?, input, 600),
        ladder_check,
    )?;
    let field = |n: &str| value[n].as_str().unwrap_or_default().trim().to_string();
    Ok(Rung {
        from: field("from"),
        to: field("to"),
        text: field("text"),
    })
}

// ---------- W5: the character ----------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Monologue {
    pub role: String,
    pub monologue: String,
    /// Only quotes that are really in the material.
    pub facts: Vec<String>,
    pub fiction: Vec<String>,
}

fn character_check(value: &Value) -> Result<(), String> {
    for field in ["role", "monologue"] {
        if value[field].as_str().unwrap_or("").trim().is_empty() {
            return Err(format!("{field} пусто"));
        }
    }
    if !value["facts"].is_array() || !value["fiction"].is_array() {
        return Err("facts и fiction — массивы".into());
    }
    Ok(())
}

pub fn character<T: Transport>(
    client: &Client<T>,
    prompts: &PromptStore,
    text: &str,
) -> Result<Monologue, UnderstandError> {
    let text = material(text)?;
    let input = format!("<MATERIAL>\n{text}\n</MATERIAL>");
    let value = client.complete_json(
        &request(system(prompts, CHARACTER_PROMPT)?, input, 900),
        character_check,
    )?;
    let strings = |n: &str| -> Vec<String> {
        value[n]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    };
    Ok(Monologue {
        role: value["role"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string(),
        monologue: value["monologue"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .trim_matches(['«', '»', '"'])
            .to_string(),
        facts: strings("facts")
            .iter()
            .filter_map(|f| verbatim(f, text))
            .collect(),
        fiction: strings("fiction"),
    })
}

// ---------- cache ----------

/// Answers already received, by mechanic and input (F-03, F-08): the same text
/// twice asks the model once. v1 kept 32 and cleared all at the 33rd.
#[derive(Debug, Default)]
pub struct Cache {
    entries: std::collections::HashMap<(Mechanic, String), Value>,
}

impl Cache {
    pub const LIMIT: usize = 32;

    pub fn get(&self, mechanic: Mechanic, input: &str) -> Option<&Value> {
        self.entries.get(&(mechanic, input.trim().to_string()))
    }

    pub fn put(&mut self, mechanic: Mechanic, input: &str, value: Value) {
        if self.entries.len() >= Self::LIMIT {
            self.entries.clear();
        }
        self.entries
            .insert((mechanic, input.trim().to_string()), value);
    }
}

#[cfg(test)]
mod tests;
