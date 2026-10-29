//! Rules of the clock and the bridge (F-25, F-26, F-28), in code.

use serde::Serialize;

/// v1 `CYCLE_GAP_SECONDS`: a pause this long offers a new path (1/3/5/∞).
pub const CYCLE_GAP: i64 = 900;
/// v1 `SESSION_GAP_SECONDS`: after three hours a new session begins.
pub const SESSION_GAP: i64 = 3 * 3600;
pub const DEFAULT_MINUTES: u32 = 15;
pub const MAX_MINUTES: u32 = 180;
/// An empty step is reading: no mark is asked at its end (F-27).
pub const READING: &str = "читаю";

/// «25 разберу X» → 25 minutes, «разберу X». A number alone is the text; more
/// than 180 minutes is not a duration; an empty input is reading for 15.
pub fn parse_step(input: &str) -> (u32, String) {
    let input = input.trim();
    if input.is_empty() {
        return (DEFAULT_MINUTES, READING.to_string());
    }
    if let Some((first, rest)) = input.split_once(char::is_whitespace)
        && let Ok(minutes) = first.parse::<u32>()
        && (1..=MAX_MINUTES).contains(&minutes)
        && !rest.trim().is_empty()
    {
        return (minutes, rest.trim().to_string());
    }
    (DEFAULT_MINUTES, input.to_string())
}

/// «Что ты оставил» (F-28): the reader's own words above the step field.
/// Nothing is ever made up: no words, no bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Bridge {
    pub label: String,
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minutes_come_first_and_only_in_range() {
        assert_eq!(parse_step("25 разберу X"), (25, "разберу X".into()));
        assert_eq!(parse_step("  "), (15, "читаю".into()));
        assert_eq!(parse_step("25"), (15, "25".into()));
        assert_eq!(
            parse_step("200 прочитаю главу"),
            (15, "200 прочитаю главу".into())
        );
        assert_eq!(parse_step("объясню X"), (15, "объясню X".into()));
    }
}
