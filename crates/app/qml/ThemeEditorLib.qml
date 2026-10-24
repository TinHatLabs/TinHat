pragma Singleton
// ThemeEditorLib.qml — the theme editor's pure helpers (11.0 step 5, contract THEMES-UI.md §13.7–§13.9; the web's
// features/settings/themes/editor/targets.ts + colour.ts, the same rules): what a swatch / row edits (a target =
// {kind: "background" | "accent" | "palette" | "role" | "token", id}), its stored value in the working document, its
// resolved colour, the base's value (Reset), the op that sets it, whether it changed — and the colour popover's maths
// (HSV ↔ RGB with the standard six-sector formula in doubles, channels round(x × 255); the engine's canonical text).
// No state, no Qt objects: documents are the parsed JSON of ThemeEditorController.stateJson.
import QtQuick

QtObject {
    id: lib

    function isObj(v) { return v !== null && typeof v === "object" && !Array.isArray(v) }
    function has(o, k) { return Object.prototype.hasOwnProperty.call(o, k) }
    function hasPalette(doc) { return !!doc && lib.isObj(doc.palette) }

    /// The value the document stores for a target (a literal or an expression), or null when it has none of its own.
    function storedValue(doc, t) {
        if (!doc) return null
        switch (t.kind) {
        case "background":
            return lib.hasPalette(doc) ? String(doc.palette.bg) : lib.storedValue(doc, { kind: "role", id: "surface.island" })
        case "accent":
            return lib.hasPalette(doc) ? (typeof doc.accent === "string" ? doc.accent : null) : lib.storedValue(doc, { kind: "role", id: "accent.base" })
        case "palette": {
            var p = doc.palette
            if (!lib.isObj(p)) return null
            var m = /^c(\d{1,2})$/.exec(t.id)
            if (m) {
                var a = p.ansi
                return Array.isArray(a) && typeof a[Number(m[1])] === "string" ? a[Number(m[1])] : null
            }
            return lib.has(p, t.id) && typeof p[t.id] === "string" ? p[t.id] : null
        }
        case "role":
        case "token": {
            var o = doc[t.kind === "role" ? "roles" : "tokens"]
            return lib.isObj(o) && lib.has(o, t.id) && typeof o[t.id] === "string" ? o[t.id] : null
        }
        }
        return null
    }

    /// The resolved (canonical) colour a target paints with in the working document (`st` = the parsed stateJson).
    function resolvedValue(st, doc, t) {
        switch (t.kind) {
        case "background": return st.roles["surface.island"]
        case "accent": return st.roles["accent.base"]
        case "palette": { var v = lib.storedValue(doc, t); return v === null ? "#000000" : v }
        case "role": return st.roles[t.id]
        case "token": return st.tokens[t.id]
        }
        return "#000000"
    }

    /// The op that sets a target (null clears it where that is allowed).
    function opFor(t, value) {
        switch (t.kind) {
        case "background": return { op: "background", value: value === null ? "" : value }
        case "accent": return { op: "accent", value: value === null ? "" : value }
        case "palette": return { op: "palette", slot: t.id, value: value }
        case "role": return { op: "role", id: t.id, value: value }
        case "token": return { op: "token", id: t.id, value: value }
        }
        return null
    }

    /// Reset (§13.9): back to the base's value — its own, or none (then the default / the derivation).
    function resetOp(base, t) { return lib.opFor(t, lib.storedValue(base, t)) }
    function isChanged(doc, base, t) { return lib.storedValue(doc, t) !== lib.storedValue(base, t) }
    /// "Use default" is offered for an override of a token, or of a role of a palette theme.
    function canUseDefault(doc, t) {
        return (t.kind === "token" || (t.kind === "role" && lib.hasPalette(doc))) && lib.storedValue(doc, t) !== null
    }
    /// Palette slots, the background and the accent must be opaque; roles and tokens may be translucent.
    function alphaAllowed(t) { return t.kind === "role" || t.kind === "token" }
    function targetId(t) { return t.kind === "background" ? "palette bg" : (t.kind === "accent" ? "accent" : t.id) }
    function targetKey(t) { return t.kind + ":" + (t.id !== undefined ? t.id : "") }
    /// The label of a target (`ctl` = ThemeEditorController: editor-labels.json).
    function targetLabel(ctl, t) {
        switch (t.kind) {
        case "background": return "Background"
        case "accent": return "Accent"
        case "palette": return ctl.label("palette", t.id)
        case "role": return ctl.label("role", t.id)
        case "token": return ctl.label("token", t.id)
        }
        return ""
    }

    // ---- colours --------------------------------------------------------------------------------------------------
    /// A canonical colour (`#rrggbb` or `#rrggbb/A`, the engine's text) → {r, g, b, a}; null for anything else.
    function literalOf(text) {
        var m = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})(?:\/(0|1|0?\.\d{1,4}))?$/.exec(String(text || ""))
        if (!m) return null
        return { r: parseInt(m[1], 16), g: parseInt(m[2], 16), b: parseInt(m[3], 16), a: m[4] === undefined ? 1 : Number(m[4]) }
    }
    /// A canonical colour as a QML colour (translucent ones with their alpha), "transparent" for anything else.
    function qmlColour(text) {
        var c = lib.literalOf(text)
        return c ? Qt.rgba(c.r / 255, c.g / 255, c.b / 255, c.a) : "transparent"   // theme-lint: ok — content: the colour being edited
    }
    function rgbToHsv(r, g, b) {
        var R = r / 255, G = g / 255, B = b / 255
        var max = Math.max(R, G, B), min = Math.min(R, G, B), d = max - min
        var h = 0
        if (d > 0) {
            if (max === R) h = 60 * (((G - B) / d) % 6)
            else if (max === G) h = 60 * ((B - R) / d + 2)
            else h = 60 * ((R - G) / d + 4)
            if (h < 0) h += 360
        }
        return { h: h, s: max === 0 ? 0 : d / max, v: max }
    }
    function hsvToRgb(h, s, v) {
        var c = v * s
        var hh = (((h % 360) + 360) % 360) / 60
        var x = c * (1 - Math.abs((hh % 2) - 1))
        var m = v - c
        var r = 0, g = 0, b = 0
        if (hh < 1) { r = c; g = x; b = 0 }
        else if (hh < 2) { r = x; g = c; b = 0 }
        else if (hh < 3) { r = 0; g = c; b = x }
        else if (hh < 4) { r = 0; g = x; b = c }
        else if (hh < 5) { r = x; g = 0; b = c }
        else { r = c; g = 0; b = x }
        function ch(t) { return Math.min(255, Math.max(0, Math.round((t + m) * 255))) }
        return [ch(r), ch(g), ch(b)]
    }
    function hex2(n) { var s = n.toString(16); return s.length < 2 ? "0" + s : s }
    /// Canonical text of rgb + alpha (0..1, kept to 2 decimals as the popover sets it): `#rrggbb` or `#rrggbb/0.85`.
    function colourText(rgb, alpha) {
        var a = alpha === undefined ? 1 : alpha
        var k = Math.round(Math.min(1, Math.max(0, a)) * 100)
        var hex = "#" + lib.hex2(rgb[0]) + lib.hex2(rgb[1]) + lib.hex2(rgb[2])
        if (k === 100) return hex
        if (k === 0) return hex + "/0"
        return hex + "/" + String(k / 100)
    }

    // ---- readability lines in words (§13.5), the refusals of the ops (§13.2) ---------------------------------------
    /// "N colours adjusted for readability"
    function adjustedText(n) { return n === 1 ? "1 colour adjusted for readability" : n + " colours adjusted for readability" }
    /// The editor's refusals in plain words (the web's ColourPopover errorText).
    function errorText(code) {
        var s = String(code), i = s.indexOf(":"), head = i < 0 ? s : s.substring(0, i)
        switch (head) {
        case "bad-colour": return "That isn't a colour Tin Hat can use here."
        case "bad-expr": return "That expression doesn't read: " + s.substring(i + 1).trim() + "."
        case "refused": {
            var rest = s.substring(i + 1).trim()
            if (rest.indexOf("cycle") === 0) return "That would make the colour depend on itself."
            if (rest.indexOf("undefined-ref") === 0) return "That expression names a colour this theme doesn't have."
            if (rest.indexOf("too-large") === 0) return "The theme would be larger than 8 KB."
            if (rest.indexOf("too-many-overrides") === 0) return "The theme would change more than 400 colours."
            if (rest.indexOf("url") === 0) return "Themes can't contain web links."
            return "Tin Hat can't use that (" + rest + ")."
        }
        case "no-palette": return "This theme has no terminal palette."
        }
        return "Tin Hat can't use that (" + s + ")."
    }
}
