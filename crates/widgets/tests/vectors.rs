//! The shared widget vectors (contract `testvectors/widgets/**`, byte-equal copies in `testvectors/widgets/`) against the
//! port — the same dispatch as the web's `src/widgets/widgets.test.ts`: every file, every case, compared as canonical
//! JSON. Plus the contract copies (widgets.json + every vector file) byte-equal to the shared contract directory when
//! WIDGETS_CONTRACT_DIR names one.

use std::path::{Path, PathBuf};

use serde_json::Value;
use tinhat_theme::canon::canon_json;
use tinhat_theme::json::parse_json;
use tinhat_widgets::history::{compute_history, HistoryInput};
use tinhat_widgets::quality::{
    band_of, codec_name, cq_band, fmt_loss, fmt_rate, rate_fill_pm, res_chip, rx_numbers, spark_points, worst_band, Band,
    RxSample, TrackNumbers,
};
use tinhat_widgets::registry::{normalize_items, settings_of, size_of};
use tinhat_widgets::people::{call_people, fmt_rate_pair, group_around, CallPerson};
use tinhat_widgets::json::{parse_strict_json, parse_strict_json_bytes};
use tinhat_widgets::rxq::{decode_rxq, decode_rxq_bytes, decoded_json, encode_rxq, receive_rxq, RxqGate};
use tinhat_widgets::recent::{pin_order, pin_summary, snippet_of, when_label, when_of};
use tinhat_widgets::clocks::{zone_choice_text, valid_zone, zone_choices, zones_of, clock_rows, fmt_offset, is_night, local_minutes, spread_label, tz_auto_write, zone_label, ClockPerson};
use tinhat_widgets::note::{note_counter_json, note_first_line, note_of, note_segments_json, note_to_save_json};
use tinhat_widgets::mentions::{add_mention, mark_seen, mention_summary, mentions_json, normalize_mentions, unseen_count, Mention, SeenSel};
use tinhat_widgets::time::{fmt_ago, fmt_dur, fmt_until, ring_of, select_up_next, UpNextItem};
use tinhat_widgets::num;
use tinhat_widgets::gameservers as gs;
use tinhat_widgets::dice;
use tinhat_widgets::polls;

/// The published contract (`WIDGETS_CONTRACT_DIR`; a branch that changes the contract points it at its private copy until
/// the merge publishes it). Unset: the comparison is skipped.
fn contract() -> PathBuf {
    PathBuf::from(std::env::var("WIDGETS_CONTRACT_DIR").unwrap_or_default())
}

fn here() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn walk(d: &Path, base: &Path, out: &mut Vec<String>) {
    for e in std::fs::read_dir(d).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            walk(&p, base, out);
        } else {
            out.push(p.strip_prefix(base).unwrap().to_string_lossy().to_string());
        }
    }
}

fn vec_dir() -> PathBuf {
    here().join("testvectors").join("widgets")
}

fn files() -> Vec<String> {
    let mut out = Vec::new();
    walk(&vec_dir(), &vec_dir(), &mut out);
    out.sort();
    out
}

/// A vector string may hold a lone UTF-16 surrogate (a JS string can; a Rust one cannot). Such an escape in the file
/// becomes this marker; the harness then feeds the case's text as WTF-8 BYTES (the surrogate as ED A0 80 …) to the
/// app's byte-level entry, which must refuse it (strict UTF-8) — what the app does with such a payload.
const LONE: &str = "\u{e000}\u{e0ff}";

fn mark_lone_surrogates(text: &str) -> (String, bool) {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut in_str = false;
    let mut found = false;
    let hex = |s: &[u8]| -> Option<u32> { std::str::from_utf8(s).ok().and_then(|h| u32::from_str_radix(h, 16).ok()) };
    while i < b.len() {
        let c = b[i];
        if !in_str {
            if c == b'"' {
                in_str = true;
            }
            out.push(c as char);
            i += 1;
            continue;
        }
        if c == b'"' {
            in_str = false;
            out.push('"');
            i += 1;
        } else if c == b'\\' && b.get(i + 1) == Some(&b'u') && i + 6 <= b.len() {
            let u = hex(&b[i + 2..i + 6]).unwrap_or(0);
            let pair = (0xd800..=0xdbff).contains(&u)
                && b.get(i + 6) == Some(&b'\\')
                && b.get(i + 7) == Some(&b'u')
                && i + 12 <= b.len()
                && (0xdc00..=0xdfff).contains(&hex(&b[i + 8..i + 12]).unwrap_or(0));
            if pair {
                out.push_str(&text[i..i + 12]);
                i += 12;
            } else if (0xd800..=0xdfff).contains(&u) {
                out.push_str(&format!("{LONE}{:04x}", u).replace(LONE, "\\ue000\\ue0ff"));
                found = true;
                i += 6;
            } else {
                out.push_str(&text[i..i + 6]);
                i += 6;
            }
        } else if c == b'\\' {
            out.push_str(&text[i..i + 2]);
            i += 2;
        } else {
            let ch = text[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    (out, found)
}

/// The text of a case as the bytes an app would receive: a lone-surrogate marker (+ its 4 hex digits) → the WTF-8
/// encoding of that surrogate.
fn wtf8(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(p) = rest.find(LONE) {
        out.extend_from_slice(rest[..p].as_bytes());
        let u = u32::from_str_radix(&rest[p + LONE.len()..p + LONE.len() + 4], 16).unwrap();
        out.extend_from_slice(&[0xe0 | (u >> 12) as u8, 0x80 | ((u >> 6) & 0x3f) as u8, 0x80 | (u & 0x3f) as u8]);
        rest = &rest[p + LONE.len() + 4..];
    }
    out.extend_from_slice(rest.as_bytes());
    out
}

fn read(rel: &str) -> Value {
    let text = std::fs::read_to_string(vec_dir().join(rel)).unwrap();
    let (text, _) = mark_lone_surrogates(&text);
    parse_json(&text).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn band_json(b: Option<Band>) -> Value {
    b.map(|x| Value::String(x.as_str().into())).unwrap_or(Value::Null)
}

fn opt_num(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64)
}

struct Checker {
    cases: usize,
    fails: Vec<String>,
}

impl Checker {
    fn check(&mut self, file: &str, i: &str, got: Value, expect: &Value) {
        self.cases += 1;
        let (g, e) = (canon_json(&got), canon_json(expect));
        if g != e {
            self.fails.push(format!(
                "{file} #{i}\n  got    {}\n  expect {}",
                g.chars().take(400).collect::<String>(),
                e.chars().take(400).collect::<String>()
            ));
        }
    }
}

fn cases(v: &Value) -> &Vec<Value> {
    v.get("cases").and_then(Value::as_array).expect("cases")
}

fn list<'a>(v: &'a Value, k: &str) -> &'a Vec<Value> {
    v.get(k).and_then(Value::as_array).unwrap_or_else(|| panic!("no list {k}"))
}

#[test]
fn every_vector_matches_the_reference() {
    let files = files();
    assert!(files.len() > 20, "{} vector files", files.len());
    let mut ck = Checker { cases: 0, fails: Vec::new() };
    for f in &files {
        let v = read(f);
        if f.starts_with("history/hand-") {
            let got = compute_history(&HistoryInput::from_json(&v["input"])).to_json();
            ck.check(f, "0", got, &v["expect"]);
        } else if f.starts_with("history/random-") {
            for (i, c) in cases(&v).iter().enumerate() {
                let got = compute_history(&HistoryInput::from_json(&c["input"])).to_json();
                ck.check(f, &i.to_string(), got, &c["expect"]);
            }
        } else if f == "rxq/encode.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let entries: Vec<TrackNumbers> = list(c, "entries").iter().filter_map(TrackNumbers::from_json).collect();
                ck.check(f, &i.to_string(), Value::String(encode_rxq(&entries)), &c["expect"]);
            }
        } else if f == "rxq/decode.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let text = c["text"].as_str().expect("text");
                let got = if text.contains(LONE) { decode_rxq_bytes(&wtf8(text)) } else { decode_rxq(text) };
                ck.check(f, &i.to_string(), decoded_json(&got), &c["expect"]);
            }
        } else if f == "rxq/gate.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let mut g = RxqGate::new();
                let valid = r#"{"v":1,"q":"good","t":[]}"#;
                let got: Vec<Value> = list(c, "steps")
                    .iter()
                    .map(|s| {
                        let at = match &s[1] {
                            Value::String(x) => x.parse::<f64>().unwrap_or(f64::NAN), // "Infinity" / "NaN"
                            x => x.as_f64().unwrap_or(f64::NAN),
                        };
                        let ok = s.get(2).and_then(Value::as_bool).unwrap_or(false);
                        Value::String(receive_rxq(&mut g, s[0].as_str().unwrap(), at, if ok { valid } else { "nope" }).as_str().into())
                    })
                    .collect();
                ck.check(f, &i.to_string(), Value::Array(got), &c["expect"]);
            }
        } else if f == "json/strict.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let text = c["text"].as_str().expect("text");
                let depth = c["maxDepth"].as_f64().unwrap() as usize;
                let r = if text.contains(LONE) { parse_strict_json_bytes(&wtf8(text), depth) } else { parse_strict_json(text, depth) };
                let got = match r {
                    Some(value) => serde_json::json!({ "ok": true, "value": value }),
                    None => serde_json::json!({ "ok": false }),
                };
                ck.check(f, &i.to_string(), got, &c["expect"]);
            }
        } else if f == "quality/bands.json" {
            for (i, c) in list(&v, "band").iter().enumerate() {
                let n = &c["n"];
                ck.check(f, &format!("band {i}"), band_json(band_of(opt_num(n, "loss"), opt_num(n, "jit"), opt_num(n, "rtt"))), &c["expect"]);
            }
            for (i, c) in list(&v, "worst").iter().enumerate() {
                let bands = list(c, "bands").iter().map(|b| b.as_str().and_then(Band::parse));
                ck.check(f, &format!("worst {i}"), band_json(worst_band(bands)), &c["expect"]);
            }
            for (i, c) in list(&v, "cq").iter().enumerate() {
                ck.check(f, &format!("cq {i}"), band_json(cq_band(c["q"].as_str())), &c["expect"]);
            }
        } else if f == "quality/rx.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let prev = RxSample::from_json(&c["prev"]);
                let cur = RxSample::from_json(&c["cur"]).expect("cur");
                let got = rx_numbers(prev.as_ref(), &cur).map(|n| n.to_json()).unwrap_or(Value::Null);
                ck.check(f, &i.to_string(), got, &c["expect"]);
            }
        } else if f == "quality/format.json" {
            for (i, c) in list(&v, "codec").iter().enumerate() {
                ck.check(f, &format!("codec {i}"), Value::String(codec_name(c["mime"].as_str())), &c["expect"]);
            }
            for (i, c) in list(&v, "res").iter().enumerate() {
                let got = res_chip(opt_num(c, "w"), opt_num(c, "h"), opt_num(c, "fps"));
                ck.check(f, &format!("res {i}"), Value::String(got), &c["expect"]);
            }
            for (i, c) in list(&v, "rate").iter().enumerate() {
                ck.check(f, &format!("rate {i}"), Value::String(fmt_rate(c["kbps"].as_f64().unwrap())), &c["expect"]);
            }
            for (i, c) in list(&v, "loss").iter().enumerate() {
                ck.check(f, &format!("loss {i}"), Value::String(fmt_loss(c["pm"].as_f64().unwrap())), &c["expect"]);
            }
            for (i, c) in list(&v, "fill").iter().enumerate() {
                ck.check(f, &format!("fill {i}"), num(rate_fill_pm(c["kbps"].as_f64().unwrap())), &c["expect"]);
            }
        } else if f == "quality/spark.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let vals: Vec<Option<f64>> = list(c, "values").iter().map(Value::as_f64).collect();
                let pts = spark_points(&vals, c["w"].as_f64().unwrap(), c["h"].as_f64().unwrap());
                let got = Value::Array(pts.into_iter().map(|(x, y)| Value::Array(vec![num(x), num(y)])).collect());
                ck.check(f, &i.to_string(), got, &c["expect"]);
            }
        } else if f == "time/ring.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let r = ring_of(c["start"].as_f64().unwrap(), c["end"].as_f64().unwrap(), c["now"].as_f64().unwrap());
                ck.check(f, &i.to_string(), r.to_json(), &c["expect"]);
            }
        } else if f == "time/labels.json" {
            for (i, c) in list(&v, "dur").iter().enumerate() {
                ck.check(f, &format!("dur {i}"), Value::String(fmt_dur(c["ms"].as_f64().unwrap())), &c["expect"]);
            }
            for (i, c) in list(&v, "until").iter().enumerate() {
                ck.check(f, &format!("until {i}"), Value::String(fmt_until(c["ms"].as_f64().unwrap())), &c["expect"]);
            }
            for (i, c) in list(&v, "ago").iter().enumerate() {
                ck.check(f, &format!("ago {i}"), Value::String(fmt_ago(c["ms"].as_f64().unwrap())), &c["expect"]);
            }
        } else if f == "time/upnext.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let items: Vec<UpNextItem> = list(c, "items").iter().map(UpNextItem::from_json).collect();
                let n = |k: &str| c.get(k).and_then(Value::as_f64).unwrap_or(f64::NAN);
                let got: Vec<Value> = select_up_next(&items, n("now"), n("days"), n("max"))
                    .into_iter()
                    .map(|e| Value::String(format!("{}/{}", e.container, e.id)))
                    .collect();
                ck.check(f, &i.to_string(), Value::Array(got), &c["expect"]);
            }
        } else if f == "registry/items.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let items = normalize_items(&c["value"]);
                let mut o = serde_json::Map::new();
                match &items {
                    None => {
                        o.insert("items".into(), Value::Null);
                        o.insert("sizes".into(), Value::Null);
                        o.insert("settings".into(), Value::Null);
                    }
                    Some(list) => {
                        o.insert("items".into(), Value::Array(list.iter().cloned().map(Value::Object).collect()));
                        o.insert("sizes".into(), Value::Array(list.iter().map(|it| Value::String(size_of(it))).collect()));
                        o.insert("settings".into(), Value::Array(list.iter().map(|it| Value::Object(settings_of(it))).collect()));
                    }
                }
                ck.check(f, &i.to_string(), Value::Object(o), &c["expect"]);
            }
        } else if f == "people/call.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let people: Vec<CallPerson> = list(c, "people").iter().map(CallPerson::from_json).collect();
                ck.check(f, &i.to_string(), call_people(&people).to_json(), &c["expect"]);
            }
        } else if f == "people/pair.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let got = fmt_rate_pair(c["down"].as_f64().unwrap(), c["up"].as_f64().unwrap());
                ck.check(f, &i.to_string(), Value::String(got), &c["expect"]);
            }
        } else if f == "people/around.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                ck.check(f, &i.to_string(), group_around(&c["input"]).to_json(), &c["expect"]);
            }
        } else if f == "gameservers/read.json" {
            // 11.1 (docs/GAMESERVERS.md §3)
            for (i, c) in cases(&v).iter().enumerate() {
                let got = gs::read_status(&c["value"]).map(|s| s.to_json()).unwrap_or(Value::Null);
                ck.check(f, &i.to_string(), got, &c["expect"]);
            }
        } else if f == "gameservers/server.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let st = gs::read_status(&c["status"]).expect("a status");
                let got: Vec<Value> = st
                    .servers
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "id": s.id, "up": gs::is_up(s), "chip": gs::chip_json(s), "players": gs::players_line(s), "who": gs::who_line(s),
                            "uptime": gs::uptime_text(s.uptime_ms), "cpuPm": num(gs::cpu_pm(s.cpu_used, s.cpu_limit)),
                            "cpuText": gs::cpu_text(s.cpu_used, s.cpu_limit), "memPm": num(gs::mem_pm(s.mem_used, s.mem_limit)),
                            "memText": gs::mem_text(s.mem_used, s.mem_limit), "size": gs::size_text(s.mem_limit),
                        })
                    })
                    .collect();
                ck.check(f, &i.to_string(), Value::Array(got), &c["expect"]);
            }
        } else if f == "gameservers/summary.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let st = gs::read_status(&c["status"]).expect("a status");
                ck.check(f, &i.to_string(), Value::String(gs::summary_of(&st)), &c["expect"]);
            }
        } else if f == "gameservers/plan.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let st = gs::read_status(&c["status"]).expect("a status");
                let target = c["target"].as_str().unwrap_or("");
                let plan = match c["first"].as_str() {
                    None => gs::plan_start(&st, target),
                    Some(fi) => gs::plan_switch(&st, target, fi),
                };
                ck.check(f, &i.to_string(), plan.to_json(), &c["expect"]);
            }
        } else if f == "gameservers/confirm.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let st = gs::read_status(&c["status"]).expect("a status");
                let got = gs::confirm_of(&st, c["action"].as_str().unwrap_or(""), c["id"].as_str().unwrap_or(""), c["first"].as_str());
                ck.check(f, &i.to_string(), got.map(|x| x.to_json()).unwrap_or(Value::Null), &c["expect"]);
            }
        } else if f == "gameservers/notice.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let got = gs::notice_text(c["actor"].as_str().unwrap_or(""), c["action"].as_str().unwrap_or(""), c["name"].as_str().unwrap_or(""), c["first"].as_str());
                ck.check(f, &i.to_string(), Value::String(got), &c["expect"]);
            }
        } else if f == "gameservers/link.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let st = gs::read_status(&c["status"]).expect("a status");
                let got = gs::link_server(&c["event"], &st).map(|s| Value::String(s.id.clone())).unwrap_or(Value::Null);
                ck.check(f, &i.to_string(), got, &c["expect"]);
            }
        } else if f == "gameservers/next.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let st = gs::read_status(&c["status"]).expect("a status");
                let evs = list(c, "events");
                let got = gs::next_event_for(c["id"].as_str().unwrap_or(""), evs, c["now"].as_f64().unwrap(), &st)
                    .map(|k| Value::String(format!("{}/{}", evs[k]["container"].as_str().unwrap_or(""), evs[k]["id"].as_str().unwrap_or(""))))
                    .unwrap_or(Value::Null);
                ck.check(f, &i.to_string(), got, &c["expect"]);
            }
        } else if f == "recent/pins.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let got = Value::Array(pin_order(&c["content"]).into_iter().map(Value::String).collect());
                ck.check(f, &i.to_string(), got, &c["expect"]);
            }
        } else if f == "recent/snippet.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                ck.check(f, &i.to_string(), snippet_of(&c["event"]).to_json(), &c["expect"]);
            }
        } else if f == "recent/summary.json" {
            for (i, c) in list(&v, "pins").iter().enumerate() {
                let got = pin_summary(c["count"].as_f64().unwrap(), c["latest"].as_str().unwrap());
                ck.check(f, &format!("pins {i}"), Value::String(got), &c["expect"]);
            }
        } else if f == "recent/when.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let w = when_of(c["ts"].as_f64().unwrap(), c["now"].as_f64().unwrap(), c["offsetMin"].as_f64().unwrap());
                let mut got = w.to_json();
                got["label"] = Value::String(when_label(&w, "CLOCK"));
                ck.check(f, &i.to_string(), got, &c["expect"]);
            }
        } else if f == "mentions/normalize.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                ck.check(f, &i.to_string(), mentions_json(&normalize_mentions(&c["value"])), &c["expect"]);
            }
        } else if f == "mentions/ops.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let mut l = normalize_mentions(&c["start"]);
                let got: Vec<Value> = list(c, "ops")
                    .iter()
                    .map(|op| {
                        let arg = &op[1];
                        l = if op[0].as_str() == Some("add") {
                            let m = normalize_mentions(&Value::Array(vec![arg.clone()]));
                            match m.into_iter().next() {
                                Some(m) => add_mention(&l, m),
                                None => l.clone(),
                            }
                        } else {
                            mark_seen(&l, &SeenSel::from_json(arg))
                        };
                        serde_json::json!({ "list": mentions_json(&l), "unseen": unseen_count(&l) })
                    })
                    .collect();
                ck.check(f, &i.to_string(), Value::Array(got), &c["expect"]);
            }
        } else if f == "mentions/summary.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let rooms = c["rooms"].clone();
                let l: Vec<Mention> = normalize_mentions(&c["list"]);
                let got = mention_summary(&l, |r| rooms.get(r).and_then(Value::as_str).unwrap_or("").to_string());
                ck.check(f, &i.to_string(), Value::String(got), &c["expect"]);
            }
        } else if f == "note/of.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let got = note_of(&c["content"]).map(Value::String).unwrap_or(Value::Null);
                ck.check(f, &i.to_string(), got, &c["expect"]);
            }
        } else if f == "note/save.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                ck.check(f, &i.to_string(), note_to_save_json(c["input"].as_str().unwrap()), &c["expect"]);
            }
        } else if f == "note/counter.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                ck.check(f, &i.to_string(), note_counter_json(c["input"].as_str().unwrap()), &c["expect"]);
            }
        } else if f == "note/segments.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                ck.check(f, &i.to_string(), note_segments_json(c["text"].as_str().unwrap()), &c["expect"]);
            }
        } else if f == "note/firstline.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                ck.check(f, &i.to_string(), Value::String(note_first_line(c["text"].as_str())), &c["expect"]);
            }
        } else if f == "clocks/labels.json" {
            for (i, c) in v["zones"].as_array().unwrap().iter().enumerate() {
                ck.check(f, &format!("zone {i}"), Value::String(zone_label(c["tz"].as_str().unwrap())), &c["expect"]);
            }
            for (i, c) in v["offsets"].as_array().unwrap().iter().enumerate() {
                ck.check(f, &format!("offset {i}"), Value::String(fmt_offset(c["diff"].as_f64().unwrap())), &c["expect"]);
            }
            for (i, c) in v["choices"].as_array().unwrap().iter().enumerate() {
                ck.check(f, &format!("choice {i}"), Value::String(zone_choice_text(c["tz"].as_str().unwrap())), &c["expect"]);
            }
        } else if f == "clocks/time.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let minutes = local_minutes(c["now"].as_f64().unwrap(), c["offsetMin"].as_f64().unwrap());
                let got = serde_json::json!({ "minutes": tinhat_widgets::num(minutes), "night": is_night(minutes) });
                ck.check(f, &i.to_string(), got, &c["expect"]);
            }
        } else if f == "clocks/rows.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let people: Vec<ClockPerson> = c["people"].as_array().unwrap().iter().map(ClockPerson::from_json).collect();
                let my = c["myOffsetMin"].as_f64().unwrap();
                let rows = clock_rows(&people, c["me"].as_str().unwrap(), my, c["now"].as_f64().unwrap());
                let got = serde_json::json!({
                    "rows": rows.iter().map(|r| r.to_json()).collect::<Vec<_>>(),
                    "spread": spread_label(&rows, c["myTz"].as_str().unwrap(), my),
                });
                ck.check(f, &i.to_string(), got, &serde_json::json!({ "rows": c["expect"], "spread": c["spread"] }));
            }
        } else if f == "clocks/zones.json" {
            for (i, c) in v["valid"].as_array().unwrap().iter().enumerate() {
                let got = c["tz"].as_str().is_some_and(valid_zone);
                ck.check(f, &format!("valid {i}"), Value::Bool(got), &c["expect"]);
            }
            for (i, c) in v["bodies"].as_array().unwrap().iter().enumerate() {
                let got = serde_json::to_value(zones_of(&c["body"])).unwrap();
                ck.check(f, &format!("body {i}"), got, &c["expect"]);
            }
            ck.check(f, "choices", serde_json::to_value(zone_choices()).unwrap(), &v["choices"]);
        } else if f == "clocks/auto.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let got = tz_auto_write(Some(&c["stored"]), c["deviceTz"].as_str().unwrap(), c["now"].as_f64().unwrap(), c["lastWriteAt"].as_f64())
                    .map(|p| p.to_json())
                    .unwrap_or(Value::Null);
                ck.check(f, &i.to_string(), got, &c["expect"]);
            }
        // widgets W3: the Dice roller (WIDGETS.md §16)
        } else if f == "dice/parse.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                ck.check(f, &i.to_string(), dice::parse_json(&dice::parse_dice(c["input"].as_str().expect("input"))), &c["expect"]);
            }
        } else if f == "dice/eval.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let p = dice::parse_dice(c["expr"].as_str().unwrap()).expect("parses");
                let got = dice::evaluate_dice(&p, list(c, "raw")).map(|r| r.to_json()).unwrap_or(Value::Null);
                ck.check(f, &i.to_string(), got, &c["expect"]);
            }
        } else if f == "dice/random.json" {
            for (i, c) in list(&v, "die").iter().enumerate() {
                let got = dice::die_from_u32(c["u"].as_f64().unwrap() as u64, c["sides"].as_f64().unwrap() as u32).map(|x| serde_json::json!(x)).unwrap_or(Value::Null);
                ck.check(f, &format!("die {i}"), got, &c["expect"]);
            }
            for (i, c) in list(&v, "stream").iter().enumerate() {
                let p = dice::parse_dice(c["expr"].as_str().unwrap()).expect("parses");
                let stream: Vec<u64> = list(c, "stream").iter().map(|x| x.as_f64().unwrap() as u64).collect();
                let got = dice::raw_from_stream(&p, &stream).map(|r| serde_json::json!(r)).unwrap_or(Value::Null);
                ck.check(f, &format!("stream {i}"), got, &c["expect"]);
            }
        } else if f == "dice/message.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let p = dice::parse_dice(c["expr"].as_str().unwrap()).expect("parses");
                let r = dice::evaluate_dice(&p, list(c, "raw")).expect("fits");
                let got = serde_json::json!({ "body": dice::dice_body(&r), "html": dice::dice_html(&r), "content": dice::dice_content(&r),
                    "faces": dice::faces_of(&r), "modifier": dice::modifier_of(&r) });
                ck.check(f, &i.to_string(), got, &c["expect"]);
            }
        } else if f == "dice/read.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                ck.check(f, &i.to_string(), dice::read_dice(&c["content"]).map(|r| r.to_json()).unwrap_or(Value::Null), &c["expect"]);
            }
        } else if f == "dice/adv.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let got = dice::with_advantage(c["expr"].as_str().unwrap(), c["kind"].as_str() == Some("adv"));
                ck.check(f, &i.to_string(), Value::String(got), &c["expect"]);
            }
        } else if f == "dice/private.json" {
            for (i, c) in list(&v, "normalize").iter().enumerate() {
                ck.check(f, &format!("normalize {i}"), dice::private_json(&dice::normalize_private(&c["value"])), &c["expect"]);
            }
            for (i, c) in list(&v, "add").iter().enumerate() {
                let l = dice::normalize_private(&c["list"]);
                let roll = &c["roll"];
                let pr = dice::PrivateRoll {
                    id: roll["id"].as_str().unwrap().to_string(),
                    ts: roll["ts"].as_f64().unwrap(),
                    expr: roll["expr"].as_str().unwrap().to_string(),
                    rolls: roll["rolls"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as u32).collect(),
                    total: roll["total"].as_f64().unwrap() as i64,
                };
                ck.check(f, &format!("add {i}"), dice::private_json(&dice::add_private(&l, &pr)), &c["expect"]);
            }
        } else if f == "dice/history.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let events: Vec<Value> = c["events"].as_array().cloned().unwrap_or_default();
                let private = dice::normalize_private(&c["private"]);
                let limit = if c["limit"].is_number() { c["limit"].as_f64().unwrap() } else { f64::NAN };
                let got = Value::Array(dice::dice_history(&events, &private, limit).iter().map(dice::DiceEntry::to_json).collect());
                ck.check(f, &i.to_string(), got, &c["expect"]);
            }
        } else if f == "dice/samples.json" {
            ck.check(f, "0", dice::chat_samples_json(), &v["samples"]);
        } else if f == "dice/summary.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let e = &c["entry"];
                let entry = if e.is_null() {
                    None
                } else {
                    let r = dice::read_dice(&serde_json::json!({ "dev.tinhat.dice": { "v": 1, "expr": e["result"]["expr"], "rolls": e["result"]["rolls"], "total": e["result"]["total"] } })).expect("a roll");
                    Some(dice::DiceEntry { id: e["id"].as_str().unwrap().into(), ts: e["ts"].as_f64().unwrap(), sender: e["sender"].as_str().unwrap().into(), private: e["private"] == Value::Bool(true), result: r })
                };
                ck.check(f, &i.to_string(), Value::String(dice::dice_summary(entry.as_ref(), c["who"].as_str().unwrap())), &c["expect"]);
            }
        // widgets W3: Polls (WIDGETS.md §17)
        } else if f == "polls/text.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                ck.check(f, &i.to_string(), polls::text_of(&c["block"]).map(Value::String).unwrap_or(Value::Null), &c["expect"]);
            }
        } else if f == "polls/start.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let got = c["type"].as_str().and_then(|t| polls::read_start(t, &c["content"])).map(|p| p.to_json()).unwrap_or(Value::Null);
                ck.check(f, &i.to_string(), got, &c["expect"]);
            }
        } else if f == "polls/response.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let got = c["type"].as_str().and_then(|t| polls::read_response(t, &c["content"]))
                    .map(|(start, answers)| serde_json::json!({ "start": start, "answers": answers }))
                    .unwrap_or(Value::Null);
                ck.check(f, &i.to_string(), got, &c["expect"]);
            }
        } else if f == "polls/end.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let got = c["type"].as_str().and_then(|t| polls::read_end(t, &c["content"])).map(|start| serde_json::json!({ "start": start })).unwrap_or(Value::Null);
                ck.check(f, &i.to_string(), got, &c["expect"]);
            }
        } else if f == "polls/tally.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let poll = polls::read_start(c["type"].as_str().unwrap(), &c["content"]).expect("a poll");
                let enders: Vec<String> = list(c, "enders").iter().map(|x| x.as_str().unwrap().to_string()).collect();
                let events: Vec<Value> = list(c, "events").clone();
                let t = polls::tally(&poll, c["start"].as_str().unwrap(), c["creator"].as_str().unwrap(), &events, &enders, c["me"].as_str().unwrap());
                let shares: Vec<Value> = t.counts.iter().map(|(_, n)| serde_json::json!(polls::share_pm(*n as f64, t.votes as f64))).collect();
                let votes: Vec<Value> = t.counts.iter().map(|(_, n)| Value::String(polls::votes_text(*n))).collect();
                let got = serde_json::json!({ "tally": t.to_json(), "show": polls::show_results(&poll, &t), "status": polls::status_line(&poll, &t),
                    "state": polls::state_line(&poll, &t), "row": polls::row_line(&poll, &t), "endText": polls::end_text(&poll, &t),
                    "shares": shares, "votes": votes });
                ck.check(f, &format!("{i} {}", c["note"].as_str().unwrap_or("")), got, &c["expect"]);
            }
        } else if f == "polls/write.json" {
            let strs = |x: &Value| -> Vec<String> { x.as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string()).collect() };
            for (i, c) in list(&v, "start").iter().enumerate() {
                let got = match polls::start_content(c["question"].as_str().unwrap(), &strs(&c["answers"]), c["kind"].as_str() == Some("disclosed"), &strs(&c["ids"])) {
                    Ok(content) => serde_json::json!({ "ok": true, "content": content }),
                    Err((code, message)) => serde_json::json!({ "ok": false, "code": code, "message": message }),
                };
                ck.check(f, &format!("start {i}"), got, &c["expect"]);
            }
            for (i, c) in list(&v, "response").iter().enumerate() {
                ck.check(f, &format!("response {i}"), polls::response_content(c["start"].as_str().unwrap(), &strs(&c["answers"])), &c["expect"]);
            }
            for (i, c) in list(&v, "end").iter().enumerate() {
                ck.check(f, &format!("end {i}"), polls::end_content(c["start"].as_str().unwrap(), c["text"].as_str().unwrap()), &c["expect"]);
            }
        } else if f == "polls/choice.json" {
            for (i, c) in cases(&v).iter().enumerate() {
                let poll = polls::read_start("org.matrix.msc3381.poll.start", &c["content"]).expect("a poll");
                let mine: Vec<String> = list(c, "mine").iter().map(|s| s.as_str().unwrap().to_string()).collect();
                let got = polls::next_choice(&poll, &mine, c["clicked"].as_str().unwrap()).map(|x| serde_json::json!(x)).unwrap_or(Value::Null);
                ck.check(f, &i.to_string(), got, &c["expect"]);
            }
        } else if f == "polls/summary.json" {
            for (i, c) in list(&v, "summary").iter().enumerate() {
                ck.check(f, &format!("summary {i}"), Value::String(polls::polls_summary(c["open"].as_f64().unwrap(), c["latest"].as_str().unwrap())), &c["expect"]);
            }
            for (i, c) in list(&v, "join").iter().enumerate() {
                let names: Vec<String> = list(c, "names").iter().map(|s| s.as_str().unwrap().to_string()).collect();
                ck.check(f, &format!("join {i}"), Value::String(polls::join_names(&names)), &c["expect"]);
            }
            for (i, c) in list(&v, "share").iter().enumerate() {
                ck.check(f, &format!("share {i}"), serde_json::json!(polls::share_pm(c["n"].as_f64().unwrap(), c["votes"].as_f64().unwrap())), &c["expect"]);
            }
        } else if f == "polls/samples.json" {
            ck.check(f, "samples", polls::samples_json(polls::POLL_CHAT_SAMPLES), &v["samples"]);
            ck.check(f, "widget", polls::samples_json(polls::POLL_WIDGET_SAMPLES), &v["widget"]);
            for (i, c) in list(&v, "events").iter().enumerate() {
                let me = c["me"].as_str().unwrap();
                let day0 = c["day0"].as_f64().unwrap();
                let got = Value::Array(polls::POLL_CHAT_SAMPLES.iter().chain(polls::POLL_WIDGET_SAMPLES.iter()).map(|s| Value::Array(polls::sample_events(s, me, day0))).collect());
                ck.check(f, &format!("events {i}"), got, &c["expect"]);
            }
        } else if ["justme/", "memories/", "soundpad/", "gamenight/"].iter().any(|d| f.starts_with(d)) {
            // widgets W2: tests/vectors_w2.rs runs these files
        } else {
            ck.fails.push(format!("unknown vector file {f} (the port does not know how to run it)"));
        }
    }
    eprintln!("{} vector files, {} cases, {} failed", files.len(), ck.cases, ck.fails.len());
    assert!(ck.fails.is_empty(), "{} of {} cases differ:\n{}", ck.fails.len(), ck.cases, ck.fails.iter().take(20).cloned().collect::<Vec<_>>().join("\n"));
    assert!(ck.cases > 800, "{} vector cases", ck.cases);
}

#[test]
fn the_contract_copies_are_byte_equal() {
    let cdir = contract();
    let c = cdir.as_path();
    if c.as_os_str().is_empty() {
        eprintln!("SKIPPED: WIDGETS_CONTRACT_DIR is not set (the shared contract directory)");
        return;
    }
    if !c.exists() {
        eprintln!("skip: no {}", c.display());
        return;
    }
    let eq = |a: &Path, b: &Path| std::fs::read(a).ok().is_some_and(|x| std::fs::read(b).ok().is_some_and(|y| x == y));
    assert!(eq(&here().join("widgets.json"), &c.join("widgets.json")), "widgets.json differs from the contract (run sync-vectors.sh)");
    let theirs_dir = c.join("testvectors").join("widgets");
    let mut theirs = Vec::new();
    walk(&theirs_dir, &theirs_dir, &mut theirs);
    theirs.sort();
    assert_eq!(files(), theirs, "the vector file lists differ (run sync-vectors.sh)");
    for f in &theirs {
        assert!(eq(&vec_dir().join(f), &theirs_dir.join(f)), "{f} differs from the contract (run sync-vectors.sh)");
    }
}
