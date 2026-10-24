//! The ☰ Theme menu's recent themes (THEMES-UI.md §12) — the port of the web's `src/theme/recent.ts`, with the same
//! unit-test case names (`tests/recent.rs` ↔ `src/theme/recent.test.ts`).
//!
//! Per device, never synced: the desktop's ui pref file `ui-theme-recent` (the web: pref `theme-recent`) — the JSON text
//! of an array of theme ids, most recent first, at most [`RECENT_MAX`], no repeats. Pure: data in, data out (the app's
//! theme store does the reading, the writing and the "does this theme still exist" lookups).

use serde_json::Value;

use crate::json::parse_json;
use crate::limits::is_theme_id;

/// How many themes the menu lists (and the history keeps).
pub const RECENT_MAX: usize = 5;

/// What the list starts from before there is any history: the theme on screen, then these two built-ins.
pub const RECENT_SEED: [&str; 2] = ["oled", "goose"];

/// Theme ids only, first occurrence wins, those `keep` accepts, at most [`RECENT_MAX`] — the shape of every list here.
fn tidy<'a>(ids: impl IntoIterator<Item = &'a str>, keep: impl Fn(&str) -> bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for x in ids {
        if out.len() == RECENT_MAX {
            break;
        }
        if !is_theme_id(x) || out.iter().any(|o| o == x) || !keep(x) {
            continue;
        }
        out.push(x.to_string());
    }
    out
}

/// The stored text → the history, most recent first. Anything that is not the JSON text of an array reads as no
/// history; entries that are not theme ids are skipped, repeats dropped, at most [`RECENT_MAX`] kept.
pub fn read_recent(text: Option<&str>) -> Vec<String> {
    let Some(text) = text else { return Vec::new() };
    match parse_json(text) {
        Ok(Value::Array(a)) => tidy(a.iter().filter_map(Value::as_str), |_| true),
        _ => Vec::new(),
    }
}

/// The history as it is stored: compact JSON (`["nord","oled","goose"]`) — `JSON.stringify` of the list (theme ids
/// need no escaping).
pub fn write_recent(list: &[String]) -> String {
    let ids = tidy(list.iter().map(String::as_str), |_| true);
    Value::from(ids).to_string()
}

/// The list a menu or a pick starts from: the history's themes that still exist, or — none left — the seed: `shown`
/// (the theme on screen), OLED, Goose.
fn start_list(history: &[String], shown: &str, exists: &dyn Fn(&str) -> bool) -> Vec<String> {
    let known = tidy(history.iter().map(String::as_str), exists);
    if !known.is_empty() {
        return known;
    }
    tidy(std::iter::once(shown).chain(RECENT_SEED), |id| id == shown || exists(id))
}

/// The history after a pick: a person applied `picked` on this device while `shown` was on screen (a ☰ Theme row, a
/// card, My themes → Use, import "Save and use", Undo, "Keep ‹old› here"). `picked` goes first, then the start list
/// (the history, or the seed when there is none); themes that no longer exist are dropped; no repeats; at most 5.
pub fn record_pick(history: &[String], shown: &str, picked: &str, exists: &dyn Fn(&str) -> bool) -> Vec<String> {
    let start = start_list(history, shown, exists);
    tidy(std::iter::once(picked).chain(start.iter().map(String::as_str)), |id| id == picked || exists(id))
}

/// The menu's rows (theme ids, top to bottom; the menu checks `current`): the start list for the theme on screen, with
/// `current` put first when it is not in it (a theme chosen on another device is shown, but is not a pick); no
/// repeats; at most 5. `current` always counts as existing.
pub fn recent_menu(history: &[String], current: &str, exists: &dyn Fn(&str) -> bool) -> Vec<String> {
    let list = start_list(history, current, exists);
    let lead = (!list.iter().any(|x| x == current)).then_some(current);
    tidy(lead.into_iter().chain(list.iter().map(String::as_str)), |id| id == current || exists(id))
}
