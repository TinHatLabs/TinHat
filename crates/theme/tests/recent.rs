//! THEMES-UI.md §12, the ☰ Theme menu's recent themes — the web's `src/theme/recent.test.ts`, case for case, same names:
//! seed_before_any_history, first_pick_keeps_the_seed, pick_goes_first, repick_moves_to_the_front, at_most_five,
//! current_from_elsewhere_is_shown_first, current_keeps_its_place, gone_themes_are_dropped, nothing_left_reseeds,
//! shown_theme_that_cannot_be_offered_is_not_kept, read_tolerates_junk, write_is_compact_json.

use tinhat_theme::recent::{read_recent, recent_menu, record_pick, write_recent, RECENT_MAX};

fn all(_: &str) -> bool {
    true
}

fn v(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|s| s.to_string()).collect()
}

#[test]
fn seed_before_any_history() {
    assert_eq!(recent_menu(&[], "nord", &all), v(&["nord", "oled", "goose"]));
    assert_eq!(recent_menu(&[], "oled", &all), v(&["oled", "goose"]));
    assert_eq!(recent_menu(&[], "goose", &all), v(&["goose", "oled"]));
}

#[test]
fn first_pick_keeps_the_seed() {
    assert_eq!(record_pick(&[], "oled", "nord", &all), v(&["nord", "oled", "goose"]));
    assert_eq!(record_pick(&[], "goose", "oled", &all), v(&["oled", "goose"]));
    assert_eq!(record_pick(&[], "dracula", "nord", &all), v(&["nord", "dracula", "oled", "goose"]));
}

#[test]
fn pick_goes_first() {
    let h = v(&["nord", "oled", "goose"]);
    assert_eq!(record_pick(&h, "nord", "dracula", &all), v(&["dracula", "nord", "oled", "goose"]));
    assert_eq!(recent_menu(&record_pick(&h, "nord", "dracula", &all), "dracula", &all), v(&["dracula", "nord", "oled", "goose"]));
}

#[test]
fn repick_moves_to_the_front() {
    assert_eq!(record_pick(&v(&["a", "b", "c"]), "a", "c", &all), v(&["c", "a", "b"]));
    assert_eq!(record_pick(&v(&["a", "b", "c"]), "a", "a", &all), v(&["a", "b", "c"]));
}

#[test]
fn at_most_five() {
    assert_eq!(RECENT_MAX, 5);
    assert_eq!(record_pick(&v(&["a", "b", "c", "d", "e"]), "a", "f", &all), v(&["f", "a", "b", "c", "d"]));
    assert_eq!(read_recent(Some(r#"["a","b","c","d","e","f","g"]"#)), v(&["a", "b", "c", "d", "e"]));
    assert_eq!(recent_menu(&v(&["a", "b", "c", "d", "e", "f"]), "a", &all), v(&["a", "b", "c", "d", "e"]));
}

#[test]
fn current_from_elsewhere_is_shown_first() {
    // another device chose "z": shown (first, checked), but not a pick — the history itself is untouched
    let h = v(&["a", "b", "c", "d", "e"]);
    assert_eq!(recent_menu(&h, "z", &all), v(&["z", "a", "b", "c", "d"]));
    assert_eq!(h, v(&["a", "b", "c", "d", "e"]));
    // the next pick starts from the history, not from what was on screen
    assert_eq!(record_pick(&v(&["a", "b"]), "z", "c", &all), v(&["c", "a", "b"]));
}

#[test]
fn current_keeps_its_place() {
    assert_eq!(recent_menu(&v(&["a", "b", "c"]), "b", &all), v(&["a", "b", "c"]));
}

#[test]
fn gone_themes_are_dropped() {
    let ex = |id: &str| id != "mine-x";
    assert_eq!(recent_menu(&v(&["a", "mine-x", "b"]), "a", &ex), v(&["a", "b"]));
    assert_eq!(record_pick(&v(&["a", "mine-x", "b"]), "a", "c", &ex), v(&["c", "a", "b"]));
}

#[test]
fn nothing_left_reseeds() {
    let ex = |id: &str| id != "gone";
    assert_eq!(recent_menu(&v(&["gone"]), "nord", &ex), v(&["nord", "oled", "goose"]));
    assert_eq!(record_pick(&v(&["gone"]), "nord", "dracula", &ex), v(&["dracula", "nord", "oled", "goose"]));
}

#[test]
fn shown_theme_that_cannot_be_offered_is_not_kept() {
    // e.g. a theme another device applied with its own document (not in the library, not one of My themes)
    let ex = |id: &str| id != "kept";
    assert_eq!(record_pick(&[], "kept", "nord", &ex), v(&["nord", "oled", "goose"]));
    assert_eq!(recent_menu(&[], "kept", &ex), v(&["kept", "oled", "goose"]));
    assert_eq!(recent_menu(&v(&["nord", "oled"]), "kept", &ex), v(&["kept", "nord", "oled"]));
}

#[test]
fn read_tolerates_junk() {
    assert_eq!(read_recent(None), v(&[]));
    assert_eq!(read_recent(Some("")), v(&[]));
    assert_eq!(read_recent(Some("nope")), v(&[]));
    assert_eq!(read_recent(Some(r#"{"0":"oled"}"#)), v(&[]));
    assert_eq!(read_recent(Some(r#""oled""#)), v(&[]));
    assert_eq!(read_recent(Some(r#"["oled","oled","Bad Id",3,null,{},"goose"]"#)), v(&["oled", "goose"]));
    let (long, max) = ("x".repeat(65), "x".repeat(64));
    assert_eq!(read_recent(Some(&format!(r#"["{long}","{max}"]"#))), vec![max]);
}

#[test]
fn write_is_compact_json() {
    assert_eq!(write_recent(&v(&["nord", "oled", "goose"])), r#"["nord","oled","goose"]"#);
    assert_eq!(write_recent(&v(&["nord", "nord", "oled"])), r#"["nord","oled"]"#);
    assert_eq!(write_recent(&[]), "[]");
    let h = v(&["catppuccin-latte", "my-theme-2", "oled"]);
    assert_eq!(read_recent(Some(&write_recent(&h))), h);
}
