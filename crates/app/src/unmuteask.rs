//! "Ask to be unmuted" (12.1.17, D04.6): the pure rules, no Qt / LiveKit / Matrix.
//! Web src/features/calls/unmuteask.ts has the same rules; the shared vectors (testvectors/unmuteask.json = web
//! src/features/calls/unmuteask.vectors.json) are byte-identical. The vectors' `_about` is the spec.
//!
//! Wire: a reliable LiveKit data packet on its OWN topic (12.1.15 and older have no branch for it and drop it),
//! payload exactly [`ask_payload`], sent by a server-muted person to everyone. The asker is the packet's SFU sender
//! identity. A receiver never unmutes anyone because of an ask: it only shows a hand + a toast whose "Unmute" is the
//! moderator's own click through the normal moderation path (the server checks rank again).

pub const UNMUTE_ASK_TOPIC: &str = tinhat_streamer::UNMUTE_ASK_TOPIC;
pub const ASK_COOLDOWN_MS: i64 = 30_000;
pub const TOAST_GAP_MS: i64 = 30_000;
pub const HAND_MS: i64 = 60_000;
pub const UNDO_MS: i64 = 5_000;
pub const MAX_PAYLOAD_BYTES: usize = 256;
/// 12.1.17 Stage B (#3): how long an ask that beat the mute's announcement is held (the vectors' `_aboutHold`).
pub const ASK_HOLD_MS: i64 = 10_000;

/// The visible strings (same in both apps). `{name}` is the one placeholder.
pub const WORDS: [(&str, &str); 10] = [
    ("askButton", "Ask to be unmuted"),
    ("askSent", "Asked to be unmuted"),
    ("askToast", "{name} asks to be unmuted"),
    ("askToastAction", "Unmute"),
    ("handTip", "{name} asks to be unmuted"),
    ("muteToast", "Muted {name} for everyone"),
    ("muteToastAction", "Undo"),
    ("toastStaleCall", "That call is over"),
    ("toastStaleRev", "{name} was muted again in the meantime, so nothing changed"),
    ("toastStaleUnmuted", "{name} isn't muted any more"),
];

/// A wording string with `{name}` filled in ("" for an unknown key).
pub fn words(key: &str, name: &str) -> String {
    WORDS.iter().find(|(k, _)| *k == key).map(|(_, v)| v.replace("{name}", name)).unwrap_or_default()
}

/// The payload, byte for byte.
pub fn ask_payload() -> String {
    tinhat_streamer::UNMUTE_ASK_PAYLOAD.to_string()
}

/// Is this packet text an ask? At most [`MAX_PAYLOAD_BYTES`] of UTF-8, a JSON object whose `v` is a finite whole
/// number >= 1 (other keys ignored).
pub fn parse_ask(text: &str) -> bool {
    tinhat_streamer::parse_unmute_ask(text)
}

/// ms until the asker may ask again (0 = now). A clock that went backwards never locks the button.
pub fn cooldown_left(last_ask_ms: Option<i64>, now_ms: i64) -> i64 {
    let Some(last) = last_ask_ms else { return 0 };
    let d = now_ms - last;
    if !(0..ASK_COOLDOWN_MS).contains(&d) {
        return 0;
    }
    ASK_COOLDOWN_MS - d
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerdictIn {
    /// the asker is my own account (another device of mine)
    pub sender_is_me: bool,
    /// the asker is server-muted right now (as far as I know)
    pub sender_server_muted: bool,
    /// I hold moderate_calls here
    pub i_moderate: bool,
    /// ranks in the call room; None = unknown (fail closed)
    pub my_rank: Option<i64>,
    pub sender_rank: Option<i64>,
    /// when I last toasted this asker's ask (None = never)
    pub last_toast_ms: Option<i64>,
    pub now_ms: i64,
}

/// What a receiver does with an ask (the vectors' order). Never "unmute".
pub fn ask_verdict(i: &VerdictIn) -> &'static str {
    if i.sender_is_me {
        return "ignore:self";
    }
    if !i.i_moderate {
        return "ignore:not-moderator";
    }
    let (Some(mine), Some(theirs)) = (i.my_rank, i.sender_rank) else { return "ignore:unknown-rank" };
    if mine <= theirs {
        return "ignore:rank";
    }
    if !i.sender_server_muted {
        return "ignore:not-muted";
    }
    if let Some(last) = i.last_toast_ms {
        if (0..TOAST_GAP_MS).contains(&(i.now_ms - last)) {
            return "hand";
        }
    }
    "toast"
}

/// Is the hand still up? (while they're muted, for [`HAND_MS`])
pub fn hand_shown(asked_at_ms: i64, now_ms: i64, still_muted: bool) -> bool {
    still_muted && now_ms - asked_at_ms < HAND_MS
}

/// The asker's button text for the time left on the cooldown.
pub fn ask_label(left_ms: i64) -> String {
    if left_ms <= 0 {
        return words("askButton", "");
    }
    format!("Asked — ask again in {} s", (left_ms + 999) / 1000)
}

// ── 12.1.17 Stage B: a toast acts only on the mute it was made for (the vectors' `_aboutToast`) ─────────────

/// The mute record after a mute / unmute of one identity (the streamer keeps it with its server-mute map).
pub use tinhat_streamer::next_mute_episode;

/// May an Undo / ask toast's button act? `revs`: every identity the toast kept (`at` = its kept revision) plus every
/// identity the press would act on now that it didn't keep (`at` = None); `now` = its revision now (None = not muted).
/// Only "act" unmutes.
pub fn toast_may_act(same_call: bool, revs: &[(Option<u64>, Option<u64>)]) -> &'static str {
    if !same_call {
        return "stale:call";
    }
    if revs.iter().any(|(at, now)| now.is_some() && now != at) {
        return "stale:rev";
    }
    if !revs.iter().any(|(at, now)| at.is_some() && now == at) {
        return "stale:not-muted";
    }
    "act"
}

/// Stage B (#3): hold `id`'s ask at `now_ms` (drops every entry no longer live first; refuses a NEW asker when `max` are
/// held; an asker already held gets the new time). true = held.
pub fn hold_put(held: &mut std::collections::HashMap<String, i64>, id: &str, now_ms: i64, max: usize) -> bool {
    held.retain(|_, at| held_ask_live(*at, now_ms));
    if !held.contains_key(id) && held.len() >= max {
        return false;
    }
    held.insert(id.to_string(), now_ms);
    true
}

/// Stage B (#3): `id`'s mute record arrived: take its held ask (removed either way). true = still live: decide it now.
pub fn hold_take(held: &mut std::collections::HashMap<String, i64>, id: &str, now_ms: i64) -> bool {
    held.remove(id).is_some_and(|at| held_ask_live(at, now_ms))
}

/// Stage B (#3): is a held ask still live? (0 <= now - heldAt < ASK_HOLD_MS; a clock that went backwards drops it)
pub fn held_ask_live(held_at_ms: i64, now_ms: i64) -> bool {
    (0..ASK_HOLD_MS).contains(&(now_ms - held_at_ms))
}

/// Round 2: offer the Undo for one identity my late mute answer records? `replaced` = the revision of the record it
/// replaced (None = none), `since` = the highest revision when my request went out. A newer mute gets no Undo.
pub fn undo_offered(replaced: Option<u64>, since: u64) -> bool {
    replaced.is_none_or(|r| r <= since)
}

/// What a stale press says (the toast closes; nothing else happens).
pub fn stale_text(verdict: &str, name: &str) -> String {
    match verdict {
        "stale:call" => words("toastStaleCall", ""),
        "stale:rev" => words("toastStaleRev", name),
        _ => words("toastStaleUnmuted", name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const WEB_COPY: &str = "src/features/calls/unmuteask.vectors.json";

    fn vectors() -> Value {
        serde_json::from_str(include_str!("testvectors/unmuteask.json")).expect("vectors")
    }
    fn opt(v: &Value) -> Option<i64> {
        v.as_i64()
    }

    #[test]
    fn constants_topic_payload_and_wording_match_the_vectors() {
        let v = vectors();
        assert_eq!(v["topic"].as_str().unwrap(), UNMUTE_ASK_TOPIC);
        assert_eq!(v["askPayload"].as_str().unwrap(), ask_payload());
        let c = &v["constants"];
        assert_eq!(c["askCooldownMs"].as_i64().unwrap(), ASK_COOLDOWN_MS);
        assert_eq!(c["toastGapMs"].as_i64().unwrap(), TOAST_GAP_MS);
        assert_eq!(c["handMs"].as_i64().unwrap(), HAND_MS);
        assert_eq!(c["undoMs"].as_i64().unwrap(), UNDO_MS);
        assert_eq!(c["maxPayloadBytes"].as_u64().unwrap() as usize, MAX_PAYLOAD_BYTES);
        let w = v["wording"].as_object().unwrap();
        assert_eq!(w.len(), WORDS.len());
        for (k, s) in WORDS {
            assert_eq!(w[k].as_str().unwrap(), s, "{k}");
        }
    }

    #[test]
    fn the_topic_is_not_one_12_1_15_handles() {
        // every data topic 12.1.15's streamer dispatches on (call.rs DataReceived)
        let old = [
            tinhat_streamer::MODERATION_TOPIC,
            "tinhat.recording_only",
            "tinhat.game_chat",
            "tinhat.corner",
            "tinhat.rx",
            "tinhat.echo",
            "tinhat.sameroom",
            "tinhat.sr_lvl",
            "tinhat.rxq",
        ];
        assert!(!old.contains(&UNMUTE_ASK_TOPIC));
    }

    #[test]
    fn parse_every_vector() {
        for c in vectors()["parse"].as_array().unwrap() {
            assert_eq!(parse_ask(c["in"].as_str().unwrap()), c["out"].as_bool().unwrap(), "{}", c["name"]);
        }
        assert!(parse_ask(&ask_payload()));
    }

    #[test]
    fn cooldown_every_vector() {
        for c in vectors()["cooldownLeft"].as_array().unwrap() {
            assert_eq!(
                cooldown_left(opt(&c["lastAskMs"]), c["nowMs"].as_i64().unwrap()),
                c["out"].as_i64().unwrap(),
                "{}",
                c["name"]
            );
        }
    }

    #[test]
    fn verdict_every_vector_and_none_unmutes() {
        let allowed = ["toast", "hand", "ignore:self", "ignore:not-moderator", "ignore:unknown-rank", "ignore:rank", "ignore:not-muted"];
        for c in vectors()["verdict"].as_array().unwrap() {
            let i: VerdictIn = serde_json::from_value(c["in"].clone()).unwrap();
            let out = ask_verdict(&i);
            assert_eq!(out, c["out"].as_str().unwrap(), "{}", c["name"]);
            assert!(allowed.contains(&out), "{}: {out}", c["name"]);
        }
    }

    #[test]
    fn hand_every_vector() {
        for c in vectors()["handShown"].as_array().unwrap() {
            assert_eq!(
                hand_shown(c["askedAtMs"].as_i64().unwrap(), c["nowMs"].as_i64().unwrap(), c["stillMuted"].as_bool().unwrap()),
                c["out"].as_bool().unwrap(),
                "{}",
                c["name"]
            );
        }
    }

    #[test]
    fn label_every_vector() {
        for c in vectors()["label"].as_array().unwrap() {
            assert_eq!(ask_label(c["leftMs"].as_i64().unwrap()), c["out"].as_str().unwrap(), "{}", c["leftMs"]);
        }
    }

    #[test]
    fn toast_next_mute_episode_every_vector() {
        let cases = vectors()["muteEpisode"].as_array().unwrap().clone();
        assert!(cases.len() >= 5);
        for c in cases {
            let prev = c["prev"].as_object().map(|p| (p["by"].as_str().unwrap().to_string(), p["rev"].as_u64().unwrap()));
            let got = next_mute_episode(
                prev.as_ref().map(|(b, r)| (b.as_str(), *r)),
                c["action"].as_str().unwrap(),
                c["by"].as_str().unwrap(),
                c["fresh"].as_u64().unwrap(),
            );
            let want = c["out"].as_object().map(|o| (o["by"].as_str().unwrap().to_string(), o["rev"].as_u64().unwrap()));
            assert_eq!(got, want, "{}", c["name"]);
        }
    }

    #[test]
    fn toast_may_act_every_vector() {
        let cases = vectors()["toastMayAct"].as_array().unwrap().clone();
        assert!(cases.len() >= 5);
        for c in cases {
            let revs: Vec<(Option<u64>, Option<u64>)> =
                c["revs"].as_array().unwrap().iter().map(|r| (r["at"].as_u64(), r["now"].as_u64())).collect();
            assert_eq!(toast_may_act(c["sameCall"].as_bool().unwrap(), &revs), c["out"].as_str().unwrap(), "{}", c["name"]);
        }
    }

    /// The review's case and its neighbours: an old toast never lifts a newer mute.
    #[test]
    fn toast_scenarios_an_old_toast_never_lifts_a_newer_mute() {
        let cases = vectors()["muteScenarios"].as_array().unwrap().clone();
        assert!(cases.iter().any(|c| c["out"] == "stale:rev"));
        for c in cases {
            let events = c["events"].as_array().unwrap();
            let after = c["toastAfter"].as_u64().unwrap() as usize;
            let mut ep: Option<(String, u64)> = None;
            let mut at: Option<u64> = None;
            for (i, e) in events.iter().enumerate() {
                if i == after {
                    at = ep.as_ref().map(|(_, r)| *r);
                }
                ep = next_mute_episode(
                    ep.as_ref().map(|(b, r)| (b.as_str(), *r)),
                    e[0].as_str().unwrap(),
                    e[1].as_str().unwrap(),
                    i as u64 + 1,
                );
            }
            if after >= events.len() {
                at = ep.as_ref().map(|(_, r)| *r);
            }
            let revs: Vec<(Option<u64>, Option<u64>)> = at.map(|a| vec![(Some(a), ep.as_ref().map(|(_, r)| *r))]).unwrap_or_default();
            assert_eq!(toast_may_act(c["sameCall"].as_bool().unwrap(), &revs), c["out"].as_str().unwrap(), "{}", c["name"]);
        }
    }

    #[test]
    fn held_ask_every_vector() {
        let v = vectors();
        assert_eq!(v["constants"]["askHoldMs"].as_i64().unwrap(), ASK_HOLD_MS);
        for c in v["heldAsk"].as_array().unwrap() {
            assert_eq!(
                held_ask_live(c["heldAtMs"].as_i64().unwrap(), c["nowMs"].as_i64().unwrap()),
                c["out"].as_bool().unwrap(),
                "{}",
                c["name"]
            );
        }
    }

    #[test]
    fn held_scenarios_every_vector() {
        let cases = vectors()["heldScenarios"].as_array().unwrap().clone();
        assert!(cases.len() >= 6);
        for c in cases {
            let max = c["max"].as_u64().unwrap() as usize;
            let mut held = std::collections::HashMap::new();
            for (i, op) in c["ops"].as_array().unwrap().iter().enumerate() {
                let (id, now) = (op[1].as_str().unwrap(), op[2].as_i64().unwrap());
                let got = if op[0] == "put" { hold_put(&mut held, id, now, max) } else { hold_take(&mut held, id, now) };
                assert_eq!(got, op[3].as_bool().unwrap(), "{}: op {i} {op}", c["name"]);
            }
        }
    }

    #[test]
    fn undo_offered_every_vector() {
        let cases = vectors()["undoOffered"].as_array().unwrap().clone();
        assert!(cases.iter().any(|c| c["out"] == false));
        for c in cases {
            assert_eq!(undo_offered(c["prevRev"].as_u64(), c["since"].as_u64().unwrap()), c["out"].as_bool().unwrap(), "{}", c["name"]);
        }
    }

    #[test]
    fn toast_stale_words() {
        let w = &vectors()["wording"];
        assert_eq!(stale_text("stale:call", "Bo"), "That call is over");
        assert_eq!(stale_text("stale:rev", "Bo"), w["toastStaleRev"].as_str().unwrap().replace("{name}", "Bo"));
        assert_eq!(stale_text("stale:not-muted", "Bo"), w["toastStaleUnmuted"].as_str().unwrap().replace("{name}", "Bo"));
    }

    #[test]
    fn the_webs_copy_of_the_vectors_is_byte_identical_when_present() {
        let mine = include_bytes!("testvectors/unmuteask.json");
        let dir = std::env::var("TINHAT_WEB_DIR").unwrap_or_else(|_| format!("{}/tinhat-web", std::env::var("HOME").unwrap_or_default()));
        if let Ok(b) = std::fs::read(format!("{dir}/{WEB_COPY}")) {
            assert_eq!(&b[..], &mine[..], "{dir}/{WEB_COPY}");
        }
    }

    /// The parity job: `TINHAT_WEB_DIR=… cargo test -p tinhat-app unmuteask -- --include-ignored`.
    #[test]
    #[ignore = "parity job: needs TINHAT_WEB_DIR (the web repo)"]
    fn the_webs_copy_of_the_vectors_must_be_byte_identical() {
        let dir = std::env::var("TINHAT_WEB_DIR").expect("TINHAT_WEB_DIR must name the web repo for the parity job");
        let b = std::fs::read(format!("{dir}/{WEB_COPY}")).expect("the web copy");
        assert_eq!(&b[..], &include_bytes!("testvectors/unmuteask.json")[..]);
    }
}
