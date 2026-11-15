//! Why the Settings mic meter shows no level (A13.0/A13.2, 2026-11-13; shared vectors testvectors/meterhint.json = the
//! web's meterHint.ts). It used to say "unplugged or in use" for every failure, also when the meter itself couldn't run
//! (no noise-filter element) or the device was watching a call.

pub(crate) fn meter_hint(reason: &str) -> &'static str {
    match reason {
        "blocked" => "Tin Hat isn't allowed to use the microphone. Allow it in the browser's site settings for this page, then reload.",
        "unavailable" => "The level meter can't run on this computer (a noise-filter part is missing). Your microphone can still work in calls.",
        "watching" => "This device is watching a call, so it doesn't open its microphone. Choose Talk here to use it.",
        _ => "No signal from this microphone — it may be unplugged or in use by another app. Pick another one above.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const V: &str = include_str!("testvectors/meterhint.json");

    #[test]
    fn the_shared_vectors() {
        let v: serde_json::Value = serde_json::from_str(V).unwrap();
        let cases = v["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 6);
        for c in cases {
            assert_eq!(meter_hint(c["reason"].as_str().unwrap()), c["hint"].as_str().unwrap(), "{c}");
        }
    }

    #[test]
    fn the_vectors_are_the_webs() {
        let Ok(dir) = std::env::var("TINHAT_WEB_DIR") else { return };
        let web = std::fs::read_to_string(format!("{dir}/src/features/settings/meterhint.json")).unwrap();
        assert_eq!(web, V, "byte-identical with the web's copy");
    }

    /// A13.3: an unplugged saved device stays in the mic / speaker / camera lists as itself (= the web's withSaved).
    #[test]
    fn saved_device_stays_visible() {
        let sw = include_str!("../qml/SettingsWindow.qml");
        assert!(sw.contains("function withSaved(model, saved)") && sw.contains("label: \"Your saved device (not connected)\""));
        for k in ["win.app.mInputs), win.app.mInputName)", "win.app.mOutputs), win.app.mOutputName)", "win.app.mCameras), win.app.mCameraName)"] {
            assert!(sw.contains(k), "{k}");
        }
    }

    /// The streamer says why a start failed and the Settings row shows the matching line.
    #[test]
    fn wired() {
        let st = include_str!("../../streamer/src/call.rs");
        let mm: String = st[st.find("pub fn start_mic_monitor(").unwrap()..].chars().take(4000).collect();
        assert!(mm.contains("set_mic_monitor_why(\"watching\")") && mm.contains("set_mic_monitor_why(\"unavailable\")"));
        let sw = include_str!("../qml/SettingsWindow.qml");
        assert!(sw.contains("win.app.callctl.micMeterHint()"));
    }
}
