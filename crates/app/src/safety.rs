//! safety.rs — account safety: ignore a person, report a person or a message, delete my account (docs/ACCOUNT-SAFETY.md
//! in Tin Hat Web). SHARED with Tin Hat Web's src/core/safety.ts, line for line: both replay the vectors in
//! `src/testvectors/account-safety.json` (byte copy of the web's testvectors/account-safety/vectors.json) — the
//! m.ignored_user_list edits, the abuse report's body for the server's /api/reports, the Matrix report requests (the
//! fallback every Matrix server answers), who may not delete their account, the deactivate requests and every word the
//! two apps show. No Qt here: the faces live in settings.rs (SettingsController: ignore list, report, delete) and QML.
//!
//! Ignore = standard Matrix (`m.ignored_user_list` account data): the server stops sending their events and invites to
//! every device of the account; the app also hides what is already on screen (room.rs).
//! Report = to the people who run the server: the Tin Hat Server's /api/reports (`kind: "abuse"`, the owner's
//! dashboard) when it offers reports, else — and when that fails — the Matrix report APIs.
//! Delete = POST /account/deactivate with `erase: true` (user-interactive auth with the account password, password.rs's
//! two steps), then the app signs out locally and forgets the account (bridge.rs finish_sign_out). Never for the
//! server's own system account or its owner (the well-known's `dev.tinhat.server.account.owner`).
use std::time::Duration;

use serde_json::{json, Map, Value};

pub const IGNORED_TYPE: &str = "m.ignored_user_list";

/// The web's `/^@[^:\s]+:[^\s]+$/`.
pub fn is_mxid(s: &str) -> bool {
    let Some(rest) = s.strip_prefix('@') else { return false };
    let Some((local, server)) = rest.split_once(':') else { return false };
    let sp = |c: char| c.is_whitespace() || c == '\u{feff}';
    !local.is_empty() && !server.is_empty() && !local.contains(sp) && !server.contains(sp)
}

// ---- the words (both apps, the same) -----------------------------------------------------------------------------

/// (key, text) — the web's WORDS, in its order (the test compares the two through the vectors).
pub const WORDS: &[(&str, &str)] = &[
    ("ignore", "Ignore…"),
    ("unignore", "Stop ignoring"),
    ("ignoreTitle", "Ignore {name}?"),
    ("ignoreText", "You won't see their messages or invites anywhere. They aren't told. Settings → Privacy → Ignored people lists everyone you ignore."),
    ("ignoreConfirm", "Ignore"),
    ("ignoredToast", "You're ignoring {name}."),
    ("unignoredToast", "You're no longer ignoring {name}. Their new messages show again."),
    ("ignoreSelf", "You can't ignore yourself."),
    ("ignoreFailed", "Couldn't change your ignored list. Try again in a moment."),
    ("ignoredSection", "Ignored people"),
    ("ignoredSectionHint", "Their messages and invites are hidden on every device you use."),
    ("ignoredEmpty", "You aren't ignoring anyone."),
    ("report", "Report…"),
    ("reportPersonTitle", "Report {name}"),
    ("reportMessageTitle", "Report this message"),
    ("reportIntro", "Reports go to the people who run {server}. The person you report isn't told who sent it."),
    ("reportWhat", "What's wrong?"),
    ("reportDetails", "Anything else they should know (optional)"),
    ("reportAlsoIgnore", "Also ignore them"),
    ("reportSend", "Send report"),
    ("reportSent", "Report sent. Thank you."),
    ("reportFailed", "Couldn't send the report. Try again in a moment."),
    ("deleteRow", "Delete my account"),
    ("deleteRowHint", "Close this account for good"),
    ("deleteTitle", "Delete your account?"),
    ("deletePassword", "Enter your account password to confirm."),
    ("deleteConfirm", "Delete my account"),
    ("deleteDone", "Your account has been deleted."),
    ("deleteWrong", "That isn't your password."),
    ("deleteUnsupported", "This server wants another way to confirm it's you: delete your account where you signed up."),
    ("deleteFailedHttp", "The server didn't delete the account (HTTP {status})."),
    ("deleteSystem", "This is the server's own system account. It can't be deleted from the app."),
    ("deleteOwner", "You own this server, so this account can't be deleted from the app: the server needs it. Manage it from the server's dashboard."),
    ("deleteGuest", "Guest accounts are deleted by the server when the visit ends."),
    ("unreachable", "Couldn't reach your server. Try again in a moment."),
];

/// The explainer's lines (the web's WORDS.deleteLines; `{mxid}` filled by delete_lines).
pub const DELETE_LINES: &[&str] = &[
    "This closes {mxid} for good.",
    "You're signed out on every device.",
    "You leave every room, and your name and photo are removed.",
    "Your messages are hidden from anyone who joins a room later; people who already saw them may keep their copies.",
    "Nobody can sign up with this name again.",
    "This can't be undone.",
];

/// A word by key ("" for an unknown key).
pub fn word(key: &str) -> &'static str {
    WORDS.iter().find(|(k, _)| *k == key).map(|(_, v)| *v).unwrap_or("")
}

/// Every word as the JSON object QML reads (`settings.safetyWords`) and the vectors' `words`.
pub fn words_json() -> Value {
    let mut m = Map::new();
    for (k, v) in WORDS {
        m.insert((*k).into(), Value::String((*v).into()));
    }
    m.insert("deleteLines".into(), json!(DELETE_LINES));
    Value::Object(m)
}

/// A template's `{key}` filled in (unknown keys stay as written).
pub fn fill(t: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(t.len());
    let mut rest = t;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('}') {
            Some(j) if !after[..j].is_empty() && after[..j].chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => {
                let k = &after[..j];
                match vars.iter().find(|(n, _)| *n == k) {
                    Some((_, v)) => out.push_str(v),
                    None => {
                        out.push('{');
                        out.push_str(k);
                        out.push('}');
                    }
                }
                rest = &after[j + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

// ---- ignore ---------------------------------------------------------------------------------------------------------

/// The people an `m.ignored_user_list` content ignores: valid Matrix ids only, sorted, each once.
pub fn ignored_users(content: &Value) -> Vec<String> {
    let Some(m) = content.get("ignored_users").and_then(Value::as_object) else { return vec![] };
    let mut v: Vec<String> = m.keys().filter(|k| is_mxid(k)).cloned().collect();
    v.sort();
    v
}

/// The content after ignoring (`ignore` true) or no longer ignoring `mxid`. Every other entry and key stays; a new
/// entry is `{}`; an entry that isn't an object becomes `{}`.
pub fn with_ignored(content: &Value, mxid: &str, ignore: bool) -> Value {
    let mut out = content.as_object().cloned().unwrap_or_default();
    let mut m = out.get("ignored_users").and_then(Value::as_object).cloned().unwrap_or_default();
    if ignore {
        if !m.get(mxid).is_some_and(Value::is_object) {
            m.insert(mxid.to_string(), json!({}));
        }
    } else {
        m.remove(mxid);
    }
    out.insert("ignored_users".into(), Value::Object(m));
    Value::Object(out)
}

/// Why `me` can't ignore `target` ("" = they can).
pub fn ignore_refusal(me: &str, target: &str) -> &'static str {
    if !is_mxid(target) {
        return "That isn't an account.";
    }
    if target == me {
        return word("ignoreSelf");
    }
    ""
}

// ---- report ---------------------------------------------------------------------------------------------------------

pub const REPORT_CATEGORIES: &[(&str, &str)] = &[
    ("spam", "Spam or a scam"),
    ("harassment", "Harassment or hate"),
    ("sexual", "Sexual content"),
    ("violence", "Violence or threats"),
    ("other", "Something else"),
];
/// The longest "anything else" text sent (UTF-16 units, as the web's `length`).
pub const DETAILS_MAX: usize = 1000;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReportTarget {
    pub user: String,
    pub room: String,
    pub event: String,
}

fn category_id(c: &str) -> &'static str {
    REPORT_CATEGORIES.iter().find(|(id, _)| *id == c).map(|(id, _)| *id).unwrap_or("other")
}
fn category_label(c: &str) -> &'static str {
    let id = category_id(c);
    REPORT_CATEGORIES.iter().find(|(i, _)| *i == id).map(|(_, l)| *l).unwrap_or("Something else")
}

/// JS `String.prototype.trim` whitespace: Unicode White_Space + U+FEFF, minus U+0085 (not JS whitespace).
fn js_trim(s: &str) -> &str {
    s.trim_matches(|c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}')
}

/// The details as sent: trimmed, at most DETAILS_MAX UTF-16 units without splitting a surrogate pair (the web's
/// cleanDetails), trimmed again.
pub fn clean_details(s: &str) -> String {
    let t = js_trim(s);
    let mut n = 0;
    let mut end = 0;
    for (i, c) in t.char_indices() {
        if n + c.len_utf16() > DETAILS_MAX {
            break;
        }
        n += c.len_utf16();
        end = i + c.len_utf8();
    }
    js_trim(&t[..end]).to_string()
}

/// "Send report" is enabled: a category is chosen ("other" needs words).
pub fn report_ready(category: &str, details: &str) -> bool {
    if !REPORT_CATEGORIES.iter().any(|(id, _)| *id == category) {
        return false;
    }
    category != "other" || !clean_details(details).is_empty()
}

/// The body POSTed to the Tin Hat Server's /api/reports (the same fields as the web's; key order is not part of the
/// contract — this serde_json sorts them).
pub fn abuse_report(reporter: &str, target: &ReportTarget, category: &str, details: &str, platform: &str, version: &str) -> Value {
    let id = category_id(category);
    let mut r = Map::new();
    r.insert("kind".into(), json!("abuse"));
    r.insert("type".into(), json!("abuse"));
    r.insert("title".into(), json!(format!("[report] {}: {}", category_label(id), target.user)));
    r.insert("category".into(), json!(id));
    r.insert("details".into(), json!(clean_details(details)));
    r.insert("reported_user".into(), json!(target.user));
    if !target.room.is_empty() {
        r.insert("room_id".into(), json!(target.room));
    }
    if !target.event.is_empty() {
        r.insert("event_id".into(), json!(target.event));
    }
    r.insert("reporter".into(), json!(reporter));
    r.insert("platform".into(), json!(platform));
    r.insert("version".into(), json!(version));
    Value::Object(r)
}

/// The `reason` of a Matrix report: the category's words, then the details.
pub fn matrix_reason(category: &str, details: &str) -> String {
    let d = clean_details(details);
    let l = category_label(category);
    if d.is_empty() {
        l.to_string()
    } else {
        format!("{l}: {d}")
    }
}

/// encodeURIComponent: everything but `A-Z a-z 0-9 - _ . ! ~ * ' ( )` as UTF-8 %XX.
pub fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReportRequest {
    /// "tinhat" (under the server's API base) | "matrix" (under /_matrix/client/v3)
    pub via: &'static str,
    pub path: String,
    pub body: Value,
}

/// The requests that send a report, in order: the first one that succeeds ends it (the web's reportPlan).
/// The room-report fallback's reason: the room report itself names no one, so the moderator reads who here
/// (web: safety.ts roomReportReason).
pub fn room_report_reason(reason: &str, user: &str) -> String {
    format!("{reason} (reported: {user})")
}

pub fn report_plan(tinhat: bool, report: &Value, target: &ReportTarget, category: &str, details: &str) -> Vec<ReportRequest> {
    let mut out = Vec::new();
    if tinhat {
        out.push(ReportRequest { via: "tinhat", path: "/reports".into(), body: report.clone() });
    }
    let reason = matrix_reason(category, details);
    let body = json!({ "reason": reason });
    if !target.event.is_empty() && !target.room.is_empty() {
        out.push(ReportRequest {
            via: "matrix",
            path: format!("/rooms/{}/report/{}", encode_component(&target.room), encode_component(&target.event)),
            body,
        });
    } else {
        out.push(ReportRequest { via: "matrix", path: format!("/users/{}/report", encode_component(&target.user)), body: body.clone() });
        if !target.room.is_empty() {
            // a room report doesn't name anyone: the reason carries who (12.0.4, assembly-035 review W4)
            let body = json!({ "reason": room_report_reason(&reason, &target.user) });
            out.push(ReportRequest { via: "matrix", path: format!("/rooms/{}/report", encode_component(&target.room)), body });
        }
    }
    out
}

/// Send a report: each planned request until one answers 2xx. `homeserver` = the account's base URL, `api` = the
/// server's Tin Hat API base ("" = none). true = someone has it.
pub async fn send_report(homeserver: &str, api: &str, token: &str, plan: &[ReportRequest]) -> bool {
    let http = crate::proxy::http(Some(Duration::from_secs(20)));
    for r in plan {
        let url = match r.via {
            "tinhat" if !api.is_empty() => format!("{}{}", api.trim_end_matches('/'), r.path),
            "tinhat" => continue,
            _ => format!("{}/_matrix/client/v3{}", homeserver.trim_end_matches('/'), r.path),
        };
        match http.post(&url).bearer_auth(token).header("Content-Type", "application/json").body(r.body.to_string()).send().await {
            Ok(a) if a.status().is_success() => {
                eprintln!("[safety] report: taken ({})", r.via);
                return true;
            }
            Ok(a) => eprintln!("[safety] report: {} said HTTP {}", r.via, a.status().as_u16()),
            Err(e) => eprintln!("[safety] report: {} no answer ({})", r.via, e.without_url()),
        }
    }
    false
}

// ---- delete my account ------------------------------------------------------------------------------------------------

pub const SYSTEM_LOCALPART: &str = "tinhat-system";

/// Why this account can't be deleted from the app ("" = it can).
pub fn delete_refusal(me: &str, owner: &str, guest: bool) -> &'static str {
    let local = me.trim_start_matches('@').split(':').next().unwrap_or("");
    if local == SYSTEM_LOCALPART {
        return word("deleteSystem");
    }
    if !owner.is_empty() && me == owner {
        return word("deleteOwner");
    }
    if guest {
        return word("deleteGuest");
    }
    ""
}

/// The explainer, filled in.
pub fn delete_lines(mxid: &str) -> Vec<String> {
    DELETE_LINES.iter().map(|l| fill(l, &[("mxid", mxid)])).collect()
}

/// The deactivate body: the first POST (`auth` None) and the second (user, password, the challenge's session).
pub fn deactivate_body(auth: Option<(&str, &str, Option<&str>)>) -> Value {
    let mut b = Map::new();
    b.insert("erase".into(), Value::Bool(true));
    if let Some((user, password, session)) = auth {
        let mut a = Map::new();
        a.insert("type".into(), json!("m.login.password"));
        a.insert("identifier".into(), json!({ "type": "m.id.user", "user": user }));
        a.insert("password".into(), json!(password));
        if let Some(s) = session {
            a.insert("session".into(), json!(s));
        }
        b.insert("auth".into(), Value::Object(a));
    }
    Value::Object(b)
}

/// The server's `error`, or deleteFailedHttp.
pub fn delete_failure_text(status: u16, body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(Value::as_str).map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fill(word("deleteFailedHttp"), &[("status", &status.to_string())]))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeleteFirst {
    Done,
    Password { session: Option<String> },
    Failed(String),
}

/// JS truthiness of a JSON value (the web's `v.flows`).
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

/// The first answer (password.rs first_answer's rule, the delete's words).
pub fn delete_first_answer(status: u16, body: &str) -> DeleteFirst {
    if (200..300).contains(&status) {
        return DeleteFirst::Done;
    }
    let v = serde_json::from_str::<Value>(body).ok();
    if status == 401 {
        if let Some(v) = v.as_ref().filter(|v| truthy(v.get("flows"))) {
            let ok = v["flows"].as_array().is_some_and(|fs| {
                fs.iter().any(|f| f.get("stages").and_then(Value::as_array).is_some_and(|st| st.len() == 1 && st[0].as_str() == Some("m.login.password")))
            });
            if !ok {
                return DeleteFirst::Failed(word("deleteUnsupported").to_string());
            }
            return DeleteFirst::Password { session: v.get("session").and_then(Value::as_str).map(str::to_owned) };
        }
    }
    DeleteFirst::Failed(delete_failure_text(status, body))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeleteOutcome {
    Deleted,
    Wrong,
    Failed(String),
}

impl DeleteOutcome {
    /// What QML reads (`deleteOutcome`): "deleted" | "wrong" | "failed".
    pub fn kind(&self) -> &'static str {
        match self {
            DeleteOutcome::Deleted => "deleted",
            DeleteOutcome::Wrong => "wrong",
            DeleteOutcome::Failed(_) => "failed",
        }
    }
    pub fn text(&self) -> String {
        match self {
            DeleteOutcome::Deleted => word("deleteDone").to_string(),
            DeleteOutcome::Wrong => word("deleteWrong").to_string(),
            DeleteOutcome::Failed(t) => t.clone(),
        }
    }
}

/// The second answer: 2xx = deleted; 401 + M_FORBIDDEN = not the password; anything else = the server's words.
pub fn delete_second_answer(status: u16, body: &str) -> DeleteOutcome {
    if (200..300).contains(&status) {
        return DeleteOutcome::Deleted;
    }
    let errcode = serde_json::from_str::<Value>(body).ok().and_then(|v| v.get("errcode").and_then(Value::as_str).map(str::to_owned));
    if status == 401 && errcode.as_deref() == Some("M_FORBIDDEN") {
        return DeleteOutcome::Wrong;
    }
    DeleteOutcome::Failed(delete_failure_text(status, body))
}

/// The two steps over any transport (`post(body)` → Some((status, body)), None = no answer). The password goes out
/// only in the second body, and only for a password-only flow.
pub async fn delete_with<F, Fut>(mut post: F, user: &str, password: &str) -> DeleteOutcome
where
    F: FnMut(Value) -> Fut,
    Fut: std::future::Future<Output = Option<(u16, String)>>,
{
    let Some((status, text)) = post(deactivate_body(None)).await else {
        return DeleteOutcome::Failed(word("unreachable").to_string());
    };
    let session = match delete_first_answer(status, &text) {
        DeleteFirst::Done => return DeleteOutcome::Deleted,
        DeleteFirst::Failed(t) => {
            eprintln!("[safety] delete: refused at the first step (HTTP {status})");
            return DeleteOutcome::Failed(t);
        }
        DeleteFirst::Password { session } => session,
    };
    // from here a rejected token may be this deletion's doing (bridge.rs's sync loop words)
    set_deleting(true);
    let Some((status, text)) = post(deactivate_body(Some((user, password, session.as_deref())))).await else {
        return DeleteOutcome::Failed(word("unreachable").to_string());
    };
    let out = delete_second_answer(status, &text);
    eprintln!("[safety] delete: {} (HTTP {status})", out.kind());
    out
}

static DELETING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A deletion is on its way (bridge.rs: a token rejected meanwhile was killed by it — the sign-in card says so).
pub fn deleting() -> bool {
    DELETING.load(std::sync::atomic::Ordering::SeqCst)
}
pub fn set_deleting(on: bool) {
    DELETING.store(on, std::sync::atomic::Ordering::SeqCst);
}

/// The two-step deactivate (erase) against the signed-in account's homeserver. Nothing of the password is logged.
/// `deleting()` holds while it runs and after a deletion (until LoginController.signOutDeleted).
pub async fn delete_account(homeserver: &str, token: &str, user: &str, password: &str) -> DeleteOutcome {
    let out = delete_account_inner(homeserver, token, user, password).await;
    if out == DeleteOutcome::Deleted {
        *deleted_user().lock().unwrap() = user.to_string();
    } else {
        set_deleting(false);
    }
    out
}

/// The account the last deletion closed ("" = none): signOutDeleted only ends THAT account's session.
pub fn deleted_user() -> &'static std::sync::Mutex<String> {
    static U: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
    U.get_or_init(|| std::sync::Mutex::new(String::new()))
}

async fn delete_account_inner(homeserver: &str, token: &str, user: &str, password: &str) -> DeleteOutcome {
    let url = format!("{}/_matrix/client/v3/account/deactivate", homeserver.trim_end_matches('/'));
    let http = crate::proxy::http(Some(Duration::from_secs(60)));
    let post = |body: Value| {
        let req = http.post(&url).bearer_auth(token).header("Content-Type", "application/json").body(body.to_string());
        async move {
            match req.send().await {
                Ok(r) => {
                    let status = r.status().as_u16();
                    Some((status, r.text().await.unwrap_or_default()))
                }
                Err(e) => {
                    eprintln!("[safety] delete: no answer ({})", e.without_url());
                    None
                }
            }
        }
    };
    delete_with(post, user, password).await
}

// ---- the ignored list over HTTP (account data) --------------------------------------------------------------------------

fn account_data_url(homeserver: &str, user: &str) -> String {
    format!("{}/_matrix/client/v3/user/{}/account_data/{}", homeserver.trim_end_matches('/'), encode_component(user), IGNORED_TYPE)
}

/// The account's m.ignored_user_list content as the server has it now (404 = none yet → `{}`). Err = no answer.
pub async fn fetch_ignored(homeserver: &str, token: &str, user: &str) -> Result<Value, String> {
    let http = crate::proxy::http(Some(Duration::from_secs(20)));
    let r = http.get(account_data_url(homeserver, user)).bearer_auth(token).send().await.map_err(|e| e.without_url().to_string())?;
    match r.status().as_u16() {
        404 => Ok(json!({})),
        s if (200..300).contains(&s) => {
            let t = r.text().await.map_err(|e| e.without_url().to_string())?;
            Ok(serde_json::from_str::<Value>(&t).unwrap_or_else(|_| json!({})))
        }
        s => Err(format!("HTTP {s}")),
    }
}

/// Ignore / stop ignoring: read the content fresh (another device may have changed it), edit it, write it back.
pub async fn set_ignored(homeserver: &str, token: &str, user: &str, target: &str, ignore: bool) -> Result<Vec<String>, String> {
    let _one = ignore_lock().lock().await;
    let now = fetch_ignored(homeserver, token, user).await?;
    let next = with_ignored(&now, target, ignore);
    let http = crate::proxy::http(Some(Duration::from_secs(20)));
    let r = http
        .put(account_data_url(homeserver, user))
        .bearer_auth(token)
        .header("Content-Type", "application/json")
        .body(next.to_string())
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    if !r.status().is_success() {
        return Err(format!("HTTP {}", r.status().as_u16()));
    }
    Ok(ignored_users(&next))
}

// ---- this process's copy of the ignored list (QML: settings.ignoredJson; the room list's invite filter) -------------------

type Sink = cxx_qt::CxxQtThread<crate::settings::qobject::SettingsController>;

/// Ignore changes from this device go one at a time (the GET-edit-PUT of one would undo another's).
fn ignore_lock() -> &'static tokio::sync::Mutex<()> {
    static L: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    L.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn state() -> &'static std::sync::Mutex<(Vec<String>, Option<Sink>)> {
    static S: std::sync::OnceLock<std::sync::Mutex<(Vec<String>, Option<Sink>)>> = std::sync::OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new((Vec::new(), None)))
}

/// Is `mxid` on the ignored list (as last seen)?
pub fn is_ignored(mxid: &str) -> bool {
    state().lock().map(|s| s.0.iter().any(|m| m == mxid)).unwrap_or(false)
}

/// SettingsController.startSafety(): it hears every later change of the list (sync, another device, this one).
pub fn attach_sink(s: Sink) {
    let snap = {
        let Ok(mut st) = state().lock() else { return };
        st.1 = Some(s.clone());
        st.0.clone()
    };
    // the list may have come with the first sync before QML asked: hand it over now
    let json = Value::from(snap).to_string();
    let _ = s.queue(move |o| crate::settings::set_ignored_json(o, &json));
}

/// store(), only while the session `gen` is still the current one (a late answer for an account that signed out
/// never lands on the next one's list).
/// Every store() is a newer copy of the list (sync, an ignore here, another device). The first fetch at sign-in
/// (settings.rs start_safety) only lands while none came after it started: a slow GET that read `[]` must not undo an
/// ignore that sync delivered meanwhile (assembly-035 review, Astra desktop #3).
static REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn revision() -> u64 {
    REVISION.load(std::sync::atomic::Ordering::SeqCst)
}

/// The first fetch's result: kept only for the same session and when no newer copy was stored since `rev`.
pub fn store_initial(gen: u64, rev: u64, list: Vec<String>) {
    if crate::session::generation() == gen && revision() == rev {
        store(list);
    }
}

pub fn store_for(gen: u64, list: Vec<String>) {
    if crate::session::generation() == gen {
        store(list);
    }
}

/// The list changed: keep it, tell QML (settings.ignoredJson).
pub fn store(list: Vec<String>) {
    REVISION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let sink = {
        let Ok(mut st) = state().lock() else { return };
        if st.0 == list {
            return;
        }
        st.0 = list.clone();
        st.1.clone()
    };
    if let Some(qt) = sink {
        let json = Value::from(list).to_string();
        let _ = qt.queue(move |o| crate::settings::set_ignored_json(o, &json));
    }
}

/// Sign-out: nobody is ignored any more on this device's copy.
pub fn forget() {
    store(Vec::new());
}

/// bridge.rs's sync loop: an m.ignored_user_list in this pass's account data replaces the list.
pub fn on_sync(account_data: &[matrix_sdk::ruma::serde::Raw<matrix_sdk::ruma::events::AnyGlobalAccountDataEvent>]) {
    for raw in account_data {
        let Ok(Some(ty)) = raw.get_field::<String>("type") else { continue };
        if ty != IGNORED_TYPE {
            continue;
        }
        let v: Value = serde_json::from_str(raw.json().get()).unwrap_or(Value::Null);
        store(ignored_users(&v["content"]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A slow first fetch that read an older list must not undo what sync (or an ignore) stored meanwhile.
    #[test]
    fn the_first_fetch_never_overwrites_a_newer_list() {
        let gen = crate::session::generation();
        let rev = revision();
        store(vec!["@alice:x.test".into()]); // sync: Alice ignored, while the GET is still on its way
        store_initial(gen, rev, vec![]); // the GET read [] before that
        assert_eq!(state().lock().unwrap().0, vec!["@alice:x.test".to_string()]);
        let rev = revision();
        store_initial(gen, rev, vec!["@bob:x.test".into()]); // nothing newer: it lands
        assert_eq!(state().lock().unwrap().0, vec!["@bob:x.test".to_string()]);
        store_initial(gen.wrapping_add(1), revision(), vec![]); // another session's answer: dropped
        assert_eq!(state().lock().unwrap().0, vec!["@bob:x.test".to_string()]);
        store(Vec::new());
    }

    fn vectors() -> Value {
        serde_json::from_str(include_str!("testvectors/account-safety.json")).unwrap()
    }
    fn s(v: &Value) -> &str {
        v.as_str().unwrap_or("")
    }
    fn target(v: &Value) -> ReportTarget {
        ReportTarget { user: s(&v["user"]).into(), room: s(&v["room"]).into(), event: s(&v["event"]).into() }
    }

    #[test]
    fn the_words_are_the_shared_words() {
        let v = vectors();
        assert_eq!(words_json(), v["words"]);
        let keys: Vec<&str> = WORDS.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys.len() + 1, v["words"].as_object().unwrap().len(), "every word, plus deleteLines");
        let cats: Vec<Value> = REPORT_CATEGORIES.iter().map(|(a, b)| json!([a, b])).collect();
        assert_eq!(Value::Array(cats), v["categories"]);
        assert_eq!(DETAILS_MAX, 1000);
    }

    #[test]
    fn fill_templates() {
        for c in vectors()["fill"].as_array().unwrap() {
            let vars: Vec<(String, String)> = c["vars"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), if v.is_string() { s(v).to_string() } else { v.to_string() }))
                .collect();
            let refs: Vec<(&str, &str)> = vars.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
            assert_eq!(fill(s(&c["template"]), &refs), s(&c["expect"]));
        }
    }

    #[test]
    fn ignored_users_and_edits() {
        let v = vectors();
        for c in v["ignored_users"].as_array().unwrap() {
            let want: Vec<String> = serde_json::from_value(c["expect"].clone()).unwrap();
            assert_eq!(ignored_users(&c["content"]), want, "{}", c["name"]);
        }
        for c in v["with_ignored"].as_array().unwrap() {
            assert_eq!(with_ignored(&c["content"], s(&c["mxid"]), c["ignore"].as_bool().unwrap()), c["expect"], "{}", c["name"]);
        }
        for c in v["ignore_refusal"].as_array().unwrap() {
            assert_eq!(ignore_refusal(s(&c["me"]), s(&c["target"])), s(&c["expect"]));
        }
    }

    #[test]
    fn report_rules() {
        let v = vectors();
        for c in v["report_ready"].as_array().unwrap() {
            assert_eq!(report_ready(s(&c["category"]), s(&c["details"])), c["expect"].as_bool().unwrap(), "{c}");
        }
        for c in v["clean_details"].as_array().unwrap() {
            assert_eq!(clean_details(s(&c["input"])), s(&c["expect"]));
        }
        for c in v["abuse_report"].as_array().unwrap() {
            let i = &c["input"];
            let got = abuse_report(s(&i["reporter"]), &target(&i["target"]), s(&i["category"]), s(&i["details"]), s(&i["platform"]), s(&i["version"]));
            assert_eq!(got, c["expect"], "{}", c["name"]);
        }
        for c in v["matrix_reason"].as_array().unwrap() {
            assert_eq!(matrix_reason(s(&c["category"]), s(&c["details"])), s(&c["expect"]));
        }
        for c in v["report_plan"].as_array().unwrap() {
            let report = json!({ "kind": "abuse" });
            let got = report_plan(c["tinhat"].as_bool().unwrap(), &report, &target(&c["target"]), s(&c["category"]), s(&c["details"]));
            let want: Vec<ReportRequest> = c["expect"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| ReportRequest {
                    via: if s(&r["via"]) == "tinhat" { "tinhat" } else { "matrix" },
                    path: s(&r["path"]).into(),
                    body: if r["body"].get("$report").is_some() { report.clone() } else { r["body"].clone() },
                })
                .collect();
            assert!(c["expect"].as_array().unwrap().iter().all(|r| r["method"] == "POST"));
            assert_eq!(got, want, "{}", c["name"]);
        }
    }

    #[test]
    fn delete_rules() {
        let v = vectors();
        for c in v["delete_refusal"].as_array().unwrap() {
            assert_eq!(delete_refusal(s(&c["me"]), s(&c["owner"]), c["guest"].as_bool().unwrap()), s(&c["expect"]), "{}", c["me"]);
        }
        let want: Vec<String> = serde_json::from_value(v["delete_lines_expect"].clone()).unwrap();
        assert_eq!(delete_lines(s(&v["delete_lines_for"])), want);
        for c in v["deactivate_body"].as_array().unwrap() {
            let a = &c["auth"];
            let got = if a.is_null() { deactivate_body(None) } else { deactivate_body(Some((s(&a["user"]), s(&a["password"]), a.get("session").and_then(Value::as_str)))) };
            assert_eq!(got, c["expect"]);
        }
        for c in v["delete_first"].as_array().unwrap() {
            let got = delete_first_answer(c["status"].as_u64().unwrap() as u16, s(&c["body"]));
            let e = &c["expect"];
            let want = match s(&e["step"]) {
                "done" => DeleteFirst::Done,
                "password" => DeleteFirst::Password { session: e.get("session").and_then(Value::as_str).map(str::to_owned) },
                _ => DeleteFirst::Failed(s(&e["text"]).into()),
            };
            assert_eq!(got, want, "{}", c["body"]);
        }
        for c in v["delete_second"].as_array().unwrap() {
            let got = delete_second_answer(c["status"].as_u64().unwrap() as u16, s(&c["body"]));
            let e = &c["expect"];
            assert_eq!(got.kind(), s(&e["kind"]), "{}", c["body"]);
            if got.kind() != "deleted" {
                assert_eq!(got.text(), s(&e["text"]));
            }
        }
    }

    fn run(answers: Vec<Option<(u16, String)>>) -> (DeleteOutcome, Vec<Value>) {
        let bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let answers = std::sync::Arc::new(std::sync::Mutex::new(answers.into_iter()));
        let b2 = bodies.clone();
        let post = move |b: Value| {
            b2.lock().unwrap().push(b);
            let a = answers.lock().unwrap().next().flatten();
            async move { a }
        };
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let out = rt.block_on(delete_with(post, "@me:x", "pw"));
        let v = bodies.lock().unwrap().clone();
        (out, v)
    }

    #[test]
    fn the_whole_delete_over_a_recording_transport() {
        let ch = Some((401, json!({ "flows": [{ "stages": ["m.login.password"] }], "session": "S" }).to_string()));
        let (out, bodies) = run(vec![ch.clone(), Some((200, "{}".into()))]);
        assert_eq!(out, DeleteOutcome::Deleted);
        assert_eq!(bodies.len(), 2);
        assert!(bodies[0].get("auth").is_none(), "no password in the first body");
        assert_eq!(bodies[1]["auth"]["password"], "pw");
        assert_eq!(bodies[1]["auth"]["session"], "S");
        assert_eq!(bodies[1]["erase"], true);
        let (out, bodies) = run(vec![Some((401, json!({ "flows": [{ "stages": ["m.login.sso"] }] }).to_string()))]);
        assert_eq!(out, DeleteOutcome::Failed(word("deleteUnsupported").into()));
        assert_eq!(bodies.len(), 1, "the password is never sent for another flow");
        let (out, _) = run(vec![None]);
        assert_eq!(out.text(), word("unreachable"));
        let (out, _) = run(vec![ch.clone(), None]);
        assert_eq!(out.text(), word("unreachable"));
        let (out, _) = run(vec![ch, Some((401, json!({ "errcode": "M_FORBIDDEN" }).to_string()))]);
        assert_eq!(out, DeleteOutcome::Wrong);
    }

    #[test]
    fn encode_component_is_encode_uri_component() {
        assert_eq!(encode_component("!r:x"), "!r%3Ax");
        assert_eq!(encode_component("$e/1"), "%24e%2F1");
        assert_eq!(encode_component("@b'(*)~:x"), "%40b'(*)~%3Ax");
        assert_eq!(encode_component("é"), "%C3%A9");
    }

    #[test]
    fn is_mxid_is_the_webs_regex() {
        for (s, ok) in [("@a:x", true), ("@a:x:8448", true), ("a:x", false), ("@:x", false), ("@a:", false), ("@ a:x", false), ("@a", false), ("@a:x y", false)] {
            assert_eq!(is_mxid(s), ok, "{s}");
        }
    }
}
