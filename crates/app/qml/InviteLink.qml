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
    // END pure
}
