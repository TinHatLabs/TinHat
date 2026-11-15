pragma Singleton
// InviteLink.qml — the invite link (selfhost/firstrun §4; the web's src/app/invite.ts + router.ts parseHash, the same
// rule). A Tin Hat Server invite is https://<server>/#/register?token=CODE (or `invite=`, `#register?…`, %-encoded).
// The desktop has no address bar: a link pasted into the sign-in screen's Invite code (or Server) field puts its CODE,
// minus the "TH-" the field draws itself, in the Invite code field.
// SHARED LOGIC: tools/invite-link-test.mjs runs the functions between the markers with the web's invite.test.ts cases.
import QtQuick

QtObject {
    id: links

    // BEGIN pure (tools/invite-link-test.mjs)
    /// One query value as URLSearchParams.get() reads it: "+" is a space, %-escapes decoded (a broken one kept as is).
    function _queryValue(query, key) {
        var parts = String(query || "").split("&")
        for (var i = 0; i < parts.length; i++) {
            if (parts[i].length === 0) continue
            var eq = parts[i].indexOf("=")
            var k = eq >= 0 ? parts[i].slice(0, eq) : parts[i]
            var v = eq >= 0 ? parts[i].slice(eq + 1) : ""
            var dk = k.replace(/\+/g, " "), dv = v.replace(/\+/g, " ")
            try { dk = decodeURIComponent(dk) } catch (e) {}
            try { dv = decodeURIComponent(dv) } catch (e) {}
            if (dk === key) return dv
        }
        return null
    }
    /// The web's parseHash, as far as the invite goes: {name: "register", token} for the register route, else
    /// {name: "other"}. "#/register?token=X", "#register?…" and "/register?…" all count.
    function parseHash(hash) {
        var h = String(hash || "")
        if (h.charAt(0) === "#") h = h.slice(1)
        if (h.length === 0 || h === "/") return { name: "other" }
        if (h.charAt(0) !== "/") h = "/" + h
        var q = h.indexOf("?")
        var path = q >= 0 ? h.slice(0, q) : h
        var query = q >= 0 ? h.slice(q + 1) : ""
        var head = path.split("/")[1] || ""
        if (head !== "register") return { name: "other" }
        var token = links._queryValue(query, "token")
        if (token === null) token = links._queryValue(query, "invite")
        return { name: "register", token: token === null ? undefined : token }
    }
    /// The register route's code as the invite field holds it: trimmed, without the "TH-" the field draws itself.
    function inviteFieldValue(token) {
        return String(token === undefined || token === null ? "" : token).trim().replace(/^th-/i, "")
    }
    /// What pasting `text` into a sign-in field means: a whole invite link (https://<server>/#/register?token=CODE,
    /// or just its "#/register?…" part) → the code for the invite field; null when it isn't one.
    function codeFromPaste(text) {
        var t = String(text === undefined || text === null ? "" : text).trim()
        var hash
        if (/^https?:\/\/\S+$/i.test(t)) {
            var i = t.indexOf("#")
            if (i < 0) return null
            hash = t.slice(i)
        } else if (t.charAt(0) === "#") {
            hash = t
        } else {
            return null
        }
        var r = links.parseHash(hash)
        return r.name === "register" ? links.inviteFieldValue(r.token) : null
    }
    /// A14.3: the server a pasted invite link names — its host ("maplehouse.tinhat.me", "chat.example.com:8443"),
    /// lower-cased; null when `text` isn't a whole https?:// invite link (codeFromPaste gives its code).
    function hostFromPaste(text) {
        var t = String(text === undefined || text === null ? "" : text).trim()
        if (links.codeFromPaste(t) === null) return null
        var m = /^https?:\/\/([^\/?#@]+)(?=[\/?#]|$)/i.exec(t)
        if (!m) return null
        var h = m[1].toLowerCase()
        return /^[a-z0-9[][a-z0-9.:\[\]-]*$/.test(h) ? h : null
    }
    /// What a paste into the Server or Invite code field means, in one piece (A14.3; the web's invite.ts
    /// inviteFromPaste): {code, host} for a whole invite link, {code, host: null} for just its "#/register?…" part,
    /// null otherwise — also for an https?:// link whose host can't be used (a user@ part, a non-ASCII name): its code
    /// must never go to whatever server the field held before.
    function inviteFromPaste(text) {
        var code = links.codeFromPaste(text)
        if (code === null) return null
        if (!/^https?:\/\//i.test(String(text === undefined || text === null ? "" : text).trim())) return { code: code, host: null }
        var host = links.hostFromPaste(text)
        return host === null ? null : { code: code, host: host }
    }
    /// The one-tap invite link a code is handed out as (A14.0): `<web app>/#/register?token=CODE` — the web app opens
    /// its create-account form with the code filled in; pasted into the desktop's sign-in screen it fills the code
    /// (codeFromPaste above) and names the server. `webUrl` is the server's web app ("https://host/").
    function registerLink(webUrl, code) {
        var base = String(webUrl === undefined || webUrl === null ? "" : webUrl)
        if (base.length > 0 && base.charAt(base.length - 1) !== "/") base += "/"
        return base + "#/register?token=" + encodeURIComponent(String(code === undefined || code === null ? "" : code).trim())
    }
    /// The ready-to-paste invite message (A14.0; the web's spaces/invitesModel.ts inviteMessageText, the same text —
    /// testvectors/invite-message/vectors.json). It leads with the one-tap link; the manual steps stay under it.
    /// `until` = the expiry day already formatted ("December 1"), "" when the code never expires; `installUrl` = where
    /// the app is downloaded ("" on a Tin Hat Server, whose message leads with its own web address).
    function inviteMessageText(server, webUrl, code, until, installUrl) {
        var c = String(code === undefined || code === null ? "" : code)
        var blip = /^TH-/i.test(c) ? c.substring(3) : c
        var u = until ? " It's good until " + until + "." : ""
        var steps = installUrl
            ? "1. Install Tin Hat: " + installUrl + "\n" + "2. Open it and pick \"Create an account\" under the sign-in button.\n"
            : "1. Open " + webUrl + " in your browser (or install the Tin Hat app).\n" + "2. Pick \"Create an account\" under the sign-in button.\n"
        return "You're invited to " + server + " on Tin Hat.\n"
             + "Join here: " + links.registerLink(webUrl, c) + "\n"
             + "Or set it up by hand:\n"
             + steps + "3. Server: " + server + " — invite code: " + blip + "." + u
             + (installUrl ? "\nOr open it in your browser: " + webUrl : "")
    }
    // END pure
}
