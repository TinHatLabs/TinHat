pragma Singleton
// LinkGate.qml — the one door every link and attachment the app hands to the OS goes through (security review
// 2026-10-26, H2). Qt.openUrlExternally passes a URL to ShellExecute (Windows) / xdg-open (Linux), which run whatever
// handler the scheme has: file:, smb:, search-ms:, ms-msdt:, ms-settings:, any app's custom scheme. Nothing in QML calls
// Qt.openUrlExternally except this file (scripts/qml-textformat-lint.py fails the build otherwise).
//   open(url)        — http / https (with a host) / mailto, and steam:// after "Open in Steam?" (`ask`, AppCore);
//                      anything else is refused (logged + `refused` signal, the main window toasts it). matrix.to mention links are handled in-app before they get here
//                      (ChatBubble.mentionTarget → the profile flyout).
//   openMedia(url)   — a decrypted attachment in the media cache: a file:// URL whose extension is a viewer type
//                      (viewer.rs OPEN_EXTS; Rust only writes those after checking the bytes).
//   openLocal(url)   — a file this app wrote itself (the diagnostics report): file:// + .txt only.
//   imageSrc(url)    — a picture someone else named (Media Party art): https:// only, else "".
//   tipText(s)       — someone's text for a tooltip: as typed once AppCore made the shared tooltip PlainText
//                      (tooltipsPlain), else escaped inside <html> (it fails closed).
// SHARED LOGIC: the rule between the markers is richtext.rs link_verdict, word for word; tools/link-gate-test.mjs runs
// it on the shared vectors (testvectors/member-content.json, section `links`) and checks OPEN_EXTS against viewer.rs.
import QtQuick

QtObject {
    id: gate

    /// AppCore._plainTooltips found the shared tooltip's text item(s) and made them PlainText. Until it has (or when it
    /// couldn't — another style), tipText() hands the tooltip rich text with everything in it escaped, so a name with
    /// markup in it shows literally and never renders (launch-gate's review #7: fail closed).
    property bool tooltipsPlain: false
    function tipText(s) {
        var t = String(s === undefined || s === null ? "" : s)
        if (gate.tooltipsPlain) return t
        return "<html>" + t.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;") + "</html>"
    }

    /// A link or a file was refused: `url` as given, `why` = no-scheme / scheme / no-host / bad-char / userinfo /
    /// not-a-viewer-file.
    signal refused(string url, string why)
    /// An allowed link that needs a yes first: `kind` "steam" = "Open in Steam?" (AppCore.qml).
    signal ask(string url, string kind)

    // BEGIN pure (tools/link-gate-test.mjs)
    /// The link rule (richtext.rs link_verdict; Tin Hat Web's safeExternalUrl says yes to the same links): {open, url,
    /// why}. `url` = what may be opened (spaces and controls trimmed from both ends).
    function verdict(raw) {
        var s = String(raw === undefined || raw === null ? "" : raw)
        s = s.replace(/^[\u0000-\u0020]+/, "").replace(/[\u0000-\u0020]+$/, "")
        var no = function (why) { return { open: false, url: s, why: why } }
        if (/[\u0000-\u001f\u007f]/.test(s)) return no("bad-char")
        var colon = s.indexOf(":")
        if (colon < 0) return no("no-scheme")
        var scheme = s.slice(0, colon)
        if (!/^[A-Za-z][A-Za-z0-9+.\-]*$/.test(scheme)) return no("no-scheme")
        scheme = scheme.toLowerCase()
        if (scheme !== "http" && scheme !== "https" && scheme !== "mailto" && scheme !== "steam") return no("scheme")
        // Qt's QUrl and a browser read a backslash differently (WHATWG: a slash; QUrl: data) — never guess
        if (s.indexOf("\\") >= 0) return no("bad-char")
        // Decided 2026-10-26: steam:// (join a server, run a game) opens after "Open in Steam?" — `ask` (richtext.rs link_asks)
        if (scheme === "steam") {
            var srest = s.slice(colon + 1)
            if (srest.indexOf("//") !== 0 || srest.length <= 2) return no("no-host")
            if (srest.indexOf(" ") >= 0) return no("bad-char")
            return { open: true, url: s, why: "", ask: "steam" }
        }
        if (scheme !== "mailto") {
            var rest = s.slice(colon + 1)
            if (rest.indexOf("//") !== 0) return no("no-host")
            var authority = rest.slice(2).split(/[\/?#]/)[0]
            if (authority.indexOf(" ") >= 0) return no("bad-char")
            if (authority.indexOf("@") >= 0) return no("userinfo")
            var host = authority
            var c = authority.lastIndexOf(":")
            if (c >= 0 && authority.charAt(authority.length - 1) !== "]" && /^[0-9]*$/.test(authority.slice(c + 1)))
                host = authority.slice(0, c)
            if (host.length === 0) return no("no-host")
            return { open: true, url: s, why: "" }
        }
        return { open: true, url: mailtoKept(s), why: "" }
    }
    /// A mailto link with only its subject / body / cc fields (richtext.rs mailto_kept: `attach=` and the like never
    /// reach the mail client); the fragment goes.
    function mailtoKept(s) {
        s = s.split("#")[0]
        var q = s.indexOf("?")
        if (q < 0) return s
        var kept = s.slice(q + 1).split("&").filter(function (p) {
            var k = p.split("=")[0].toLowerCase()
            return k === "subject" || k === "body" || k === "cc"
        })
        return kept.length > 0 ? s.slice(0, q) + "?" + kept.join("&") : s.slice(0, q)
    }
    /// Where an allowed link goes, for a hover hint: the host of an http(s) link, the address of a mailto, else "".
    function describe(raw) {
        var v = verdict(raw)
        if (!v.open) return ""
        if (v.ask === "steam") return "Steam"
        var m = /^mailto:([^?]*)/i.exec(v.url)
        if (m) return m[1]
        var a = /^[A-Za-z]+:\/\/([^\/?#]*)/.exec(v.url)
        if (!a) return ""
        return a[1].toLowerCase()
    }
    /// The extensions a cached attachment may be opened with (viewer.rs OPEN_EXTS).
    function openExtList() {
        return ["png", "jpg", "gif", "webp", "avif", "mp4", "webm", "mp3", "ogg", "opus", "wav", "flac", "m4a", "aac", "weba", "pdf"]   // no "txt": Save as only (viewer.rs SAVE_ONLY_EXTS)
    }
    /// A local file:/// URL (no host: never a share) whose name ends in one of `exts` (no query, no fragment).
    function fileOk(raw, exts) {
        var s = String(raw === undefined || raw === null ? "" : raw)
        if (!/^file:\/\/\//i.test(s) || /[?#\\\u0000-\u001f\u007f]/.test(s)) return false
        var m = /\.([A-Za-z0-9]+)$/.exec(s)
        return !!m && exts.indexOf(m[1].toLowerCase()) >= 0
    }
    /// Someone's text on a control the style draws (a MenuItem, a Button, a CheckBox…): the style's label is AutoText
    /// and can't be told otherwise, so every "<" becomes "‹" — nothing in the string can read as markup any more
    /// (Qt::mightBeRichText needs a "<"). scripts/qml-textformat-lint.py asks for it on every bound control text.
    function plain(s) {
        return String(s === undefined || s === null ? "" : s).replace(/</g, "\u2039")
    }
    /// A picture someone else named (Media Party art and thumbnails; launch-gate's review #5): drawn only from an
    /// https:// address. A file:// or UNC one would make Windows contact that host on its own (handing it the user's
    /// NTLM hash), and any other scheme goes straight to Qt's image decoders. "" = draw the placeholder instead.
    function imageSrc(s) {
        var t = String(s === undefined || s === null ? "" : s).trim()
        return (t.length <= 4096 && /^https:\/\/[^\/\s\\]/i.test(t) && !/[\s\\]/.test(t)) ? t : ""
    }
    // END pure

    /// Open a link with the OS — only if the rule allows it. A link the rule asks about first (steam://) is handed to
    /// `ask` (AppCore shows "Open in Steam?") and opens through openAsked() on Open.
    function open(raw) {
        var v = gate.verdict(raw)
        if (v.open && v.ask) {
            gate.ask(v.url, v.ask)
            return false
        }
        if (v.open) {
            Qt.openUrlExternally(v.url)
            return true
        }
        console.warn("[links] refused (" + v.why + "): " + String(raw).slice(0, 200))
        gate.refused(String(raw), v.why)
        return false
    }
    /// The person said Open to the question `ask` raised: open it (the rule checked again).
    function openAsked(raw) {
        var v = gate.verdict(raw)
        if (v.open) {
            Qt.openUrlExternally(v.url)
            return true
        }
        gate.refused(String(raw), v.why)
        return false
    }
    /// Open a decrypted attachment from the media cache (RoomController.mediaOpened).
    function openMedia(raw) {
        if (gate.fileOk(raw, gate.openExtList())) {
            Qt.openUrlExternally(String(raw))
            return true
        }
        console.warn("[links] refused attachment: " + String(raw).slice(0, 200))
        gate.refused(String(raw), "not-a-viewer-file")
        return false
    }
    /// Open a file this app wrote itself (the diagnostics report).
    function openLocal(raw) {
        if (gate.fileOk(raw, ["txt"])) {
            Qt.openUrlExternally(String(raw))
            return true
        }
        console.warn("[links] refused local file: " + String(raw).slice(0, 200))
        gate.refused(String(raw), "not-a-viewer-file")
        return false
    }
}
