//! Trails (S5 rule 8, F-22): «Выбор метода» and «Различи методы». The reader
//! names the method to start with and the feature of the condition that shows
//! it; nothing is solved to the end. The criterion — acceptable methods and the
//! deciding feature — is fixed before the answer. A code match against it is a
//! verified success; anything else the reader grades by comparing (B27), since
//! a synonym is not a mistake. The model never grades (INV-10).

use cognitive_exoskeleton_ai::{Client, Priority, PromptStore, Request, Transport};
use serde::Serialize;
use serde_json::Value;

use crate::PracticeError;

pub const METHOD_PROMPT: &str = "NEW_method_choice";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrailKind {
    SelectMethod,
    ContrastMethods,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Situation {
    pub condition: String,
    pub acceptable: Vec<String>,
    /// Verbatim part of the condition that decides the method.
    pub feature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MethodItem {
    pub kind: TrailKind,
    pub title: String,
    pub main: Situation,
    /// Hidden until [Выбрать из вариантов]; opening them makes the attempt
    /// not independent.
    pub options: Vec<String>,
    pub why: String,
    /// Shown after the answer: «а если условие изменилось так…».
    pub variation: Situation,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MethodSet {
    pub items: Vec<MethodItem>,
    pub not_applicable: bool,
    pub notes: Vec<String>,
}

/// How much help the reader saw before answering (F-22).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Help {
    None,
    OptionsRevealed,
    SolutionRevealed,
}

impl Help {
    pub fn independent(self) -> bool {
        self == Help::None
    }
}

/// The code side of the criterion: the answer names one of the acceptable
/// methods. `false` means «not recognised», not «wrong».
pub fn names_acceptable(answer: &str, acceptable: &[String]) -> bool {
    let answer = normalize(answer);
    acceptable.iter().any(|method| {
        let method = normalize(method);
        !method.is_empty()
            && (answer.contains(&method)
                || significant(&method)
                    .iter()
                    .all(|w| answer.contains(&stem(w))))
    })
}

pub fn schema_check(value: &Value) -> Result<(), String> {
    match value["status"].as_str() {
        Some("not_applicable") => return Ok(()),
        Some("ready") => {}
        _ => return Err("status — ready или not_applicable".into()),
    }
    let items = value["items"].as_array().ok_or("нет массива items")?;
    for (i, item) in items.iter().enumerate() {
        for field in ["kind", "title", "condition", "feature", "why"] {
            if !item[field].is_string() {
                return Err(format!("items[{i}].{field} должен быть строкой"));
            }
        }
        if !item["options"].is_array() || !item["acceptable"].is_array() {
            return Err(format!("items[{i}]: options и acceptable — массивы"));
        }
        if !item["variation"]["condition"].is_string() {
            return Err(format!("items[{i}]: нет variation.condition"));
        }
    }
    Ok(())
}

pub fn validate(raw: &Value, material: &str) -> MethodSet {
    let mut set = MethodSet {
        not_applicable: raw["status"] == "not_applicable",
        ..Default::default()
    };
    for item in raw["items"].as_array().into_iter().flatten() {
        match accept(item, material) {
            Ok(item) => set.items.push(item),
            Err(reason) => set.notes.push(reason),
        }
    }
    set
}

fn accept(item: &Value, material: &str) -> Result<MethodItem, String> {
    let title = text(&item["title"]);
    let kind = match item["kind"].as_str() {
        Some("select_method") => TrailKind::SelectMethod,
        Some("contrast_methods") => TrailKind::ContrastMethods,
        other => return Err(format!("«{title}»: неизвестный вид {other:?}")),
    };
    let options = strings(&item["options"]);
    if !(3..=4).contains(&options.len()) {
        return Err(format!("«{title}»: вариантов должно быть 3–4"));
    }
    let main = situation(item, &item["acceptable"], &title)?;
    let variation = situation(&item["variation"], &item["variation"]["acceptable"], &title)?;
    for s in [&main, &variation] {
        if s.acceptable.len() >= options.len() {
            return Err(format!("«{title}»: верны все варианты — различать нечего"));
        }
    }
    for method in main.acceptable.iter().chain(&variation.acceptable) {
        if !options.iter().any(|o| normalize(o) == normalize(method)) {
            return Err(format!("«{title}»: верного «{method}» нет среди вариантов"));
        }
    }
    if !main
        .acceptable
        .iter()
        .any(|m| names_acceptable(material, std::slice::from_ref(m)))
    {
        return Err(format!(
            "«{title}»: метода {:?} нет в материале",
            main.acceptable
        ));
    }
    for (label, s) in [("условие", &main), ("вариация", &variation)] {
        if let Some(leak) = options
            .iter()
            .find(|o| names_acceptable(&s.condition, std::slice::from_ref(o)))
        {
            return Err(format!("«{title}»: {label} называет метод «{leak}»"));
        }
    }
    if kind == TrailKind::ContrastMethods
        && main.acceptable.iter().any(|m| {
            variation
                .acceptable
                .iter()
                .any(|v| normalize(v) == normalize(m))
        })
    {
        return Err(format!(
            "«{title}»: в «Различи методы» вариации нужен другой метод"
        ));
    }
    Ok(MethodItem {
        kind,
        title,
        main,
        options,
        why: text(&item["why"]),
        variation,
    })
}

fn situation(value: &Value, acceptable: &Value, title: &str) -> Result<Situation, String> {
    let condition = text(&value["condition"]);
    let feature = text(&value["feature"]);
    let acceptable = strings(acceptable);
    if condition.is_empty() || acceptable.is_empty() {
        return Err(format!("«{title}»: пустое условие или нет верного метода"));
    }
    if feature.is_empty() || !normalize(&condition).contains(&normalize(&feature)) {
        return Err(format!(
            "«{title}»: признак «{feature}» не взят дословно из условия"
        ));
    }
    Ok(Situation {
        condition,
        acceptable,
        feature,
    })
}

pub fn method_trails<T: Transport>(
    client: &Client<T>,
    prompts: &PromptStore,
    material: &str,
    priority: Priority,
) -> Result<MethodSet, PracticeError> {
    let material = material.trim();
    if material.is_empty() {
        return Err(PracticeError::EmptyText);
    }
    let request = Request {
        system: prompts
            .get(METHOD_PROMPT)
            .map_err(|e| PracticeError::Prompt(e.to_string()))?
            .to_string(),
        input: format!("МАТЕРИАЛ:\n{material}"),
        max_tokens: 2000,
        json: true,
        priority,
    };
    let raw = client.complete_json(&request, schema_check)?;
    Ok(validate(&raw, material))
}

/// Words that name the method itself: «метод», «правило», «способ» and the
/// like say nothing about which one.
fn significant(s: &str) -> Vec<String> {
    const GENERIC: [&str; 12] = [
        "метод",
        "метода",
        "методом",
        "правило",
        "правилу",
        "правила",
        "способ",
        "способом",
        "приём",
        "прием",
        "формула",
        "теорема",
    ];
    let words: Vec<String> = s
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() > 3 && !GENERIC.contains(w))
        .map(str::to_string)
        .collect();
    if words.is_empty() {
        vec![s.to_string()]
    } else {
        words
    }
}

/// A crude Russian stem: the first five letters cover «Гаусса/Гауссом/Гаусс».
fn stem(word: &str) -> String {
    word.chars().take(5).collect()
}

fn normalize(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('ё', "е")
        .to_lowercase()
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const MATERIAL: &str = "Систему решают методом Гаусса. Если матрица квадратная и определитель не равен нулю, можно применить правило Крамера.";

    fn item(kind: &str, condition: &str, variation_ok: &str) -> Value {
        json!({"kind":kind,"title":"Система 2×2","condition":condition,
            "options":["метод Гаусса","правило Крамера","подбор","графический способ"],
            "acceptable":["правило Крамера"],"feature":"определитель равен 5",
            "why":"квадратная матрица с ненулевым определителем",
            "variation":{"condition":"Система 3×4: уравнений меньше, чем неизвестных.",
                         "acceptable":[variation_ok],"feature":"уравнений меньше, чем неизвестных"}})
    }

    #[test]
    fn a_sound_contrast_item_is_kept() {
        let raw = json!({"status":"ready","items":[item("contrast_methods",
            "Система 2×2, определитель равен 5. С чего начать?","метод Гаусса")]});
        let set = validate(&raw, MATERIAL);
        assert_eq!(set.items.len(), 1, "{:?}", set.notes);
        assert_eq!(set.items[0].kind, TrailKind::ContrastMethods);
    }

    #[test]
    fn leaks_foreign_methods_and_same_method_contrasts_are_dropped() {
        let leak = item(
            "select_method",
            "Решите по правилу Крамера: определитель равен 5.",
            "метод Гаусса",
        );
        let same = item(
            "contrast_methods",
            "Система 2×2, определитель равен 5.",
            "правило Крамера",
        );
        let mut foreign = item(
            "select_method",
            "Система 2×2, определитель равен 5.",
            "метод Гаусса",
        );
        foreign["options"] = json!([
            "метод Ньютона",
            "метод Гаусса",
            "подбор",
            "графический способ"
        ]);
        foreign["acceptable"] = json!(["метод Ньютона"]);
        let raw = json!({"status":"ready","items":[leak, same, foreign]});
        let set = validate(&raw, MATERIAL);
        assert!(set.items.is_empty());
        assert!(set.notes[0].contains("называет метод"), "{:?}", set.notes);
        assert!(set.notes[1].contains("другой метод"));
        assert!(set.notes[2].contains("нет в материале"));
    }

    #[test]
    fn an_item_where_every_option_is_right_trains_nothing() {
        let mut all = item(
            "select_method",
            "Система 2×2, определитель равен 5.",
            "метод Гаусса",
        );
        all["variation"]["acceptable"] = json!([
            "метод Гаусса",
            "правило Крамера",
            "подбор",
            "графический способ"
        ]);
        let set = validate(&json!({"status":"ready","items":[all]}), MATERIAL);
        assert!(set.notes[0].contains("различать нечего"), "{:?}", set.notes);
    }

    #[test]
    fn the_code_recognises_a_named_method_in_any_case_form() {
        let acceptable = vec!["метод Гаусса".to_string()];
        assert!(names_acceptable(
            "Гауссом, потом смотреть на a",
            &acceptable
        ));
        assert!(names_acceptable("начну с метода Гаусса", &acceptable));
        assert!(!names_acceptable("Крамер", &acceptable));
        assert!(Help::None.independent() && !Help::OptionsRevealed.independent());
    }
}
