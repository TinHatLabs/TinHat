//! Per-application audio share sources (0.8.22) — the list behind "Share audio
//! from <app>" in the stream popover.
//!
//!   * Linux: PipeWire `Stream/Output/Audio` nodes read from `pw-dump` (id
//!     `app:<object.serial>`; the streamer captures that node's OUTPUT ports with
//!     `pipewiresrc target-object=<serial>` — what `pw-record --target` does, proven in
//!     on the test rig with a pw-play stream). One program's sound, never the mix.
//!   * Windows: WASAPI audio sessions (id `pid:<process id>`; captured with wasapi2src
//!     process loopback, include-process-tree) — see `audioapps_win.rs`.
//!
//! Our own process is never listed: capturing ourselves is exactly the self-capture echo.
use serde_json::Value;

#[cfg(windows)]
#[path = "audioapps_win.rs"]
mod audioapps_win;

pub struct AudioApp {
    pub id: String,
    pub label: String,
    pub detail: String,
    pub active: bool,
}

pub fn list_audio_apps() -> Vec<AudioApp> {
    #[cfg(windows)]
    {
        return list_windows();
    }
    #[allow(unreachable_code)]
    list_pipewire()
}

/// JSON `[{id,label,detail,active}]` for the QML list.
pub fn list_audio_apps_json() -> String {
    let arr: Vec<Value> = list_audio_apps()
        .into_iter()
        .map(|a| serde_json::json!({ "id": a.id, "label": a.label, "detail": a.detail, "active": a.active }))
        .collect();
    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".to_string())
}

#[cfg_attr(windows, allow(dead_code))]
fn list_pipewire() -> Vec<AudioApp> {
    let out = match std::process::Command::new("pw-dump").output() {
        Ok(o) if o.status.success() => o.stdout,
        _ => return Vec::new(),
    };
    let v: Value = match serde_json::from_slice(&out) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let Some(objs) = v.as_array() else { return Vec::new() };
    let me = std::process::id().to_string();
    let mut sinks: std::collections::HashMap<u64, String> = std::collections::HashMap::new();
    let mut links: Vec<(u64, u64)> = Vec::new();
    // (node id, serial, app name, media name)
    let mut streams: Vec<(u64, String, String, String)> = Vec::new();
    let prop = |props: &Value, k: &str| -> String {
        match props.get(k) {
            Some(Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
            None => String::new(),
        }
    };
    for o in objs {
        let ty = o.get("type").and_then(Value::as_str).unwrap_or("");
        let id = o.get("id").and_then(Value::as_u64).unwrap_or(0);
        let info = o.get("info");
        if ty == "PipeWire:Interface:Node" {
            let Some(props) = info.and_then(|i| i.get("props")) else { continue };
            match prop(props, "media.class").as_str() {
                "Audio/Sink" => {
                    let d = [prop(props, "node.description"), prop(props, "node.nick"), prop(props, "node.name")]
                        .into_iter()
                        .find(|x| !x.is_empty())
                        .unwrap_or_default();
                    sinks.insert(id, d);
                }
                "Stream/Output/Audio" => {
                    if prop(props, "application.process.id") == me {
                        continue;
                    }
                    let serial = prop(props, "object.serial");
                    if serial.is_empty() {
                        continue;
                    }
                    let app = [
                        prop(props, "application.name"),
                        prop(props, "application.process.binary"),
                        prop(props, "node.name"),
                    ]
                    .into_iter()
                    .find(|x| !x.is_empty())
                    .unwrap_or_else(|| "Unknown app".to_string());
                    streams.push((id, serial, app, prop(props, "media.name")));
                }
                _ => {}
            }
        } else if ty == "PipeWire:Interface:Link" {
            if let Some(i) = info {
                let on = i.get("output-node-id").and_then(Value::as_u64);
                let inn = i.get("input-node-id").and_then(Value::as_u64);
                if let (Some(a), Some(b)) = (on, inn) {
                    links.push((a, b));
                }
            }
        }
    }
    let mut apps: Vec<AudioApp> = streams
        .into_iter()
        .map(|(id, serial, app, media)| {
            let sink = links
                .iter()
                .find(|(a, _)| *a == id)
                .and_then(|(_, b)| sinks.get(b))
                .cloned()
                .unwrap_or_default();
            let mut detail = String::new();
            if !media.is_empty() && media != app {
                detail.push_str(&media);
            }
            if !sink.is_empty() {
                if !detail.is_empty() {
                    detail.push_str(" · ");
                }
                detail.push_str("on ");
                detail.push_str(&sink);
            }
            AudioApp { id: format!("app:{serial}"), label: app, detail, active: true }
        })
        .collect();
    apps.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
    apps
}

#[cfg(windows)]
fn list_windows() -> Vec<AudioApp> {
    fn title_case(exe: &str) -> String {
        let mut c = exe.chars();
        match c.next() {
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            None => String::new(),
        }
    }
    let mut apps: Vec<AudioApp> = audioapps_win::list_sessions()
        .into_iter()
        .map(|s| {
            // Apps rarely set a session display name (and some set a resource
            // reference like "@%SystemRoot%\..."); fall back to the exe name.
            let dn = s.display_name.trim().to_string();
            let label = if !dn.is_empty() && !dn.starts_with('@') { dn } else { title_case(&s.exe) };
            let detail = if s.active { "playing now".to_string() } else { s.exe.clone() };
            AudioApp { id: format!("pid:{}", s.pid), label, detail, active: s.active }
        })
        .collect();
    apps.sort_by(|a, b| b.active.cmp(&a.active).then(a.label.to_lowercase().cmp(&b.label.to_lowercase())));
    apps
}
