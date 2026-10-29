//! richtext.rs — what another member's HTML may become on this screen (security review 2026-10-26, H2).
//!
//! A message's `formatted_body` (and an edit's `m.new_content.formatted_body`) is written by whoever sent it, and the
//! bubble shows it as Qt RichText: before this module it went there verbatim, so a member could make every reader's
//! client fetch `<img src="https://…">` (the reader's IP and time of reading, also in encrypted rooms, plus attacker
//! bytes into Qt's image decoders) and hand `<a href="file:…">` / `search-ms:` / `ms-msdt:` links to the OS opener.
//!
//! [`sanitize`] parses the HTML with html5ever (through `ruma-html`, already in the tree for matrix-sdk) and writes a
//! NEW document from an allow-list — nothing of the input is copied through unless a rule below names it:
//! - tags: the Matrix spec's subset that Qt's rich text renders (p div blockquote h1–h6 ul ol li b i u strong em s
//!   code pre sup sub hr br table thead tbody tr th td span a); `del`/`strike` → `s`, `font` → `span`, `details` →
//!   `div`, `summary` → `<p><b>…</b></p>`; any other tag keeps its text but loses its markup;
//! - dropped with everything inside: script style template iframe noscript noembed noframes xmp plaintext title head
//!   textarea select audio video canvas colgroup frameset frame, `mx-reply` (the reply fallback), and anything outside
//!   the HTML namespace (svg, math);
//! - `<img>` never loads: it becomes its `alt` text (a custom emoji reads `:party:`), at most 256 characters;
//! - attributes: `a[href]` only when [`link_verdict`] allows it (http/https with a host and no user name, mailto — the
//!   same rule as the one link gate the QML opens links through, `LinkGate.qml`), written back normalised; `ol[start]` digits;
//!   `code[class]` `language-*` words; `data-mx-color` / `color` / `data-mx-bg-color` `#hex` → our own `style`;
//!   `span[data-mx-spoiler]` (its contents become plain text: `embeds::hide_spoilers` then hides them). Every other
//!   attribute (style, on*, id, class, src, title, data-*) is gone;
//! - link text that names another host than the link goes to (`<a href="https://evil.example">https://bank.example</a>`)
//!   is followed by the real host as text: `https://bank.example (evil.example)`; a link with a user name before the host
//!   (`https://bank.example@evil.example`) is no link at all;
//! - nesting deeper than [`MAX_DEPTH`] keeps its text only; a body with more than [`MAX_TAGS`] `<` is not rendered as
//!   rich text at all (the caller shows the plain `body`).
//!
//! The same rule set is Tin Hat Web's (`src/features/chat/lib/html.ts`, DOMPurify + its hook): both apps replay the
//! shared vectors `testvectors/member-content.json` (section `html` → visible text, links, what must be absent; `qt` is
//! this app's exact output) and the link gate's section `links`.

use ruma_html::{Html, NodeData, NodeRef};
use url::Url;

/// More markup than a chat message has any use for: past this many `<` the formatted body is not rendered as rich
/// text (the plain body is). Also bounds the parse tree's depth (its drop is recursive).
pub const MAX_TAGS: usize = 4096;
/// Elements nested deeper than this keep their text, not their markup.
pub const MAX_DEPTH: usize = 100;
/// An `<img>`'s alt text, at most (characters).
const MAX_ALT: usize = 256;
/// Review follow-up (Astra, 2026-10-26): bounds on what one body may cost. A formatted body longer than this (bytes;
/// a Matrix event is at most 64 KiB) or whose sanitised form would be longer than [`MAX_OUTPUT`] is not rendered as rich
/// text (the plain body is) — `&` × 40 000 became 200 KB of `&amp;`.
pub const MAX_INPUT: usize = 128 * 1024;
pub const MAX_OUTPUT: usize = 512 * 1024;
/// The sanitised-HTML cache keeps at most this many bytes (inputs + outputs), oldest out first.
const CACHE_BYTES: usize = 8 * 1024 * 1024;

/// What a tag becomes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Rule {
    /// written as this tag
    Keep(&'static str),
    /// written as `<outer><inner>…</inner></outer>`
    Wrap(&'static str, &'static str),
    /// the tag goes, its contents stay
    Transparent,
    /// the tag and everything inside go
    Drop,
    /// `<img>`: its alt text
    Image,
}

fn rule_of(tag: &str) -> Rule {
    match tag {
        "p" | "div" | "blockquote" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ul" | "ol" | "li" | "b" | "i" | "u"
        | "strong" | "em" | "s" | "code" | "pre" | "sup" | "sub" | "hr" | "br" | "table" | "thead" | "tbody" | "tr"
        | "th" | "td" | "span" | "a" => Rule::Keep(keep_name(tag)),
        "del" | "strike" => Rule::Keep("s"),
        "font" => Rule::Keep("span"),
        "details" => Rule::Keep("div"),
        "summary" => Rule::Wrap("p", "b"),
        "img" => Rule::Image,
        "script" | "style" | "template" | "iframe" | "noscript" | "noembed" | "noframes" | "xmp" | "plaintext"
        | "title" | "head" | "textarea" | "select" | "audio" | "video" | "canvas" | "colgroup" | "frameset"
        | "frame" | "mx-reply" => Rule::Drop,
        _ => Rule::Transparent,
    }
}

/// The `&'static str` spelling of an allowed tag (the output never carries the input's own string).
fn keep_name(tag: &str) -> &'static str {
    const NAMES: [&str; 32] = [
        "p", "div", "blockquote", "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "li", "b", "i", "u", "strong", "em",
        "s", "code", "pre", "sup", "sub", "hr", "br", "table", "thead", "tbody", "tr", "th", "td", "span", "a",
    ];
    NAMES.iter().copied().find(|n| *n == tag).unwrap_or("span")
}

fn is_void(tag: &str) -> bool {
    matches!(tag, "br" | "hr")
}

const HTML_NS: &str = "http://www.w3.org/1999/xhtml";

enum Step {
    /// node, depth, inside a spoiler, inside a kept link
    Node(NodeRef, usize, bool, bool),
    Close(String),
}

/// A member's HTML → the HTML this app shows (the module doc has the rules). `None`: too much markup to render
/// ([`MAX_TAGS`]) — show the plain body instead.
pub fn sanitize(html: &str) -> Option<String> {
    if html.len() > MAX_INPUT || html.bytes().filter(|&b| b == b'<').count() > MAX_TAGS {
        return None;
    }
    let doc = Html::parse(html);
    // a body with a link the reader may follow loses its reordering bidi controls everywhere (text, alt text, spoiler
    // reasons): an override before the link reorders its text as much as one inside it (assembly-038, D-c review)
    let links = has_kept_link(&doc);
    let strip = |t: &str| -> String { if links { t.chars().filter(|c| !is_bidi_control(*c)).collect() } else { t.to_string() } };
    let mut out = String::with_capacity(html.len() + 16);
    let mut stack: Vec<Step> = Vec::new();
    push_children(&mut stack, doc.children(), 0, false, false);
    while let Some(step) = stack.pop() {
        let (node, depth, in_spoiler, in_link) = match step {
            Step::Close(s) => {
                out.push_str(&s);
                continue;
            }
            Step::Node(n, d, sp, l) => (n, d, sp, l),
        };
        match node.data() {
            NodeData::Text(t) => esc_text(&mut out, &strip(&t.borrow())),
            NodeData::Element(el) => {
                // svg, math and anything else outside HTML: gone with its contents (foreign content is where the
                // parser differentials live)
                if &*el.name.ns != HTML_NS {
                    continue;
                }
                let tag: &str = &el.name.local;
                let mut rule = rule_of(tag);
                // past the depth limit, inside a spoiler (its contents are plain text) and inside a link (no link in
                // a link), markup goes and text stays
                let nested_link = tag == "a" && has_link_ancestor(&node);
                if (depth >= MAX_DEPTH || in_spoiler || nested_link) && matches!(rule, Rule::Keep(_) | Rule::Wrap(..)) {
                    rule = Rule::Transparent;
                }
                match rule {
                    Rule::Drop => {}
                    Rule::Image => {
                        let alt = attr_of(el, "alt").unwrap_or_default();
                        let alt: String = js_trim_alt(&alt).chars().take(MAX_ALT).collect();
                        esc_text(&mut out, &strip(&alt));
                    }
                    Rule::Transparent => push_children(&mut stack, node.children(), depth + 1, in_spoiler, in_link),
                    Rule::Wrap(outer, inner) => {
                        out.push_str(&format!("<{outer}><{inner}>"));
                        stack.push(Step::Close(format!("</{inner}></{outer}>")));
                        push_children(&mut stack, node.children(), depth + 1, in_spoiler, in_link);
                    }
                    Rule::Keep(name) => match start_tag(name, el, &node, links, in_link) {
                        // a link nobody may follow: its text, no anchor
                        None => push_children(&mut stack, node.children(), depth + 1, in_spoiler, in_link),
                        Some((start, close, spoiler)) => {
                            out.push_str(&start);
                            if !is_void(name) {
                                stack.push(Step::Close(close));
                                // inside a kept link from here down (assembly-038: carried, not re-derived per element)
                                push_children(&mut stack, node.children(), depth + 1, in_spoiler || spoiler, in_link || name == "a");
                            }
                        }
                    },
                }
            }
            _ => {}
        }
        if out.len() > MAX_OUTPUT {
            return None;
        }
    }
    Some(out)
}

fn push_children(stack: &mut Vec<Step>, children: ruma_html::Children, depth: usize, in_spoiler: bool, in_link: bool) {
    let kids: Vec<NodeRef> = children.collect();
    stack.extend(kids.into_iter().rev().map(|n| Step::Node(n, depth, in_spoiler, in_link)));
}

fn attr_of(el: &ruma_html::ElementData, name: &str) -> Option<String> {
    el.attrs.borrow().iter().find(|a| &*a.name.local == name && a.name.ns.is_empty()).map(|a| a.value.to_string())
}

/// An allowed element's start tag (only the attributes the rules name, rewritten), its end tag (plus the real host
/// after a link whose text names another), and whether it is a spoiler. `None`: an `<a>` without a link we allow.
fn start_tag(name: &'static str, el: &ruma_html::ElementData, node: &NodeRef, links: bool, in_link: bool) -> Option<(String, String, bool)> {
    let mut start = format!("<{name}");
    let mut close = format!("</{name}>");
    let mut spoiler = false;
    // a kept link and everything inside it: no member colours, no spoiler (`in_link` is carried down by sanitize)
    let linked = name == "a" || in_link;
    match name {
        "a" => {
            let href = attr_of(el, "href").and_then(|h| safe_href(&h))?;
            start.push_str(" href=\"");
            esc_attr(&mut start, &href);
            start.push('"');
            // (not twice: a body that already says it — this app's own output, quoted — keeps one)
            let said = |host: &str| node.next_sibling().and_then(|n| n.as_text().map(|t| t.borrow().starts_with(&format!(" ({host})")))).unwrap_or(false);
            if let Some(host) = spoof_host(&href, &text_of(node)).filter(|h| !said(h)) {
                close.push_str(" (");
                esc_text(&mut close, &host);
                close.push(')');
            }
        }
        "ol" => {
            if let Some(s) = attr_of(el, "start").filter(|s| !s.is_empty() && s.len() <= 9 && s.bytes().all(|b| b.is_ascii_digit())) {
                start.push_str(&format!(" start=\"{s}\""));
            }
        }
        "code" => {
            let class = attr_of(el, "class").unwrap_or_default();
            let langs: Vec<&str> = class.split_ascii_whitespace().filter(|c| is_language_class(c)).collect();
            if !langs.is_empty() {
                start.push_str(&format!(" class=\"{}\"", langs.join(" ")));
            }
        }
        "span" if !linked => {
            if let Some(reason) = attr_of(el, "data-mx-spoiler") {
                // the shape embeds::hide_spoilers reads: the attribute right after the tag name, nothing but text inside
                spoiler = true;
                start.push_str(" data-mx-spoiler");
                let reason: String = if links { reason.chars().filter(|c| !is_bidi_control(*c)).collect() } else { reason };
                if !reason.is_empty() {
                    start.push_str("=\"");
                    esc_attr(&mut start, &reason);
                    start.push('"');
                }
            }
        }
        _ => {}
    }
    if !spoiler && !linked {
        let mut style = Vec::new();
        if let Some(c) = attr_of(el, "data-mx-color").or_else(|| attr_of(el, "color")).filter(|c| is_hex_colour(c)) {
            style.push(format!("color:{}", c.to_ascii_lowercase()));
        }
        if let Some(c) = attr_of(el, "data-mx-bg-color").filter(|c| is_hex_colour(c)) {
            style.push(format!("background-color:{}", c.to_ascii_lowercase()));
        }
        if !style.is_empty() {
            start.push_str(&format!(" style=\"{}\"", style.join(";")));
        }
    }
    start.push('>');
    Some((start, close, spoiler))
}

/// An alt text trimmed as JavaScript's String.trim does (Tin Hat Web): Unicode White_Space except U+0085, plus U+FEFF
/// (assembly-038, D-c review round 2: a BOM before the alt made the two apps read different words).
fn js_trim_alt(s: &str) -> &str {
    s.trim_matches(|c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{FEFF}')
}

/// Does the body hold a link the reader may follow (an `<a>` whose href we keep)?
fn has_kept_link(doc: &Html) -> bool {
    let mut stack: Vec<NodeRef> = doc.children().collect();
    while let Some(n) = stack.pop() {
        if let Some(e) = n.as_element() {
            if &*e.name.ns == HTML_NS && &*e.name.local == "a" && attr_of(e, "href").and_then(|h| safe_href(&h)).is_some() {
                return true;
            }
        }
        stack.extend(n.children());
    }
    false
}

/// Elements whose edges separate words in a link's claim text (a line break, a block, a cell, a raised or lowered part:
/// `paypal.com<br>x` shows two words, so it is checked as two; the web's claims.ts claimText, the same list).
const CLAIM_BREAKS: &[&str] = &[
    "br", "hr", "p", "div", "blockquote", "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "li", "table", "thead", "tbody",
    "tr", "th", "td", "caption", "pre", "details", "summary", "sub", "sup",
];

/// The bidi controls that reorder text (embeddings, overrides, isolates): dropped from a whole body that holds a kept link.
/// The marks (U+200E, U+200F, U+061C) stay: they can't reorder letters (shared vector: a spoof note through U+200E).
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

fn has_link_ancestor(node: &NodeRef) -> bool {
    let mut cur = node.parent();
    while let Some(p) = cur {
        if p.as_element().is_some_and(|e| &*e.name.local == "a") {
            return true;
        }
        cur = p.parent();
    }
    false
}

/// The visible text of an element (what the reader takes the link for): its text nodes, an image's alt; a space at each
/// edge of a [`CLAIM_BREAKS`] element.
fn text_of(node: &NodeRef) -> String {
    let mut s = String::new();
    // None = a word break
    let mut stack: Vec<Option<NodeRef>> = node.children().map(Some).collect();
    stack.reverse();
    while let Some(item) = stack.pop() {
        let Some(n) = item else {
            s.push(' ');
            continue;
        };
        match n.data() {
            NodeData::Text(t) => s.push_str(&t.borrow()),
            NodeData::Element(el) => {
                if &*el.name.ns != HTML_NS {
                    continue;
                }
                let brk = CLAIM_BREAKS.contains(&&*el.name.local);
                match rule_of(&el.name.local) {
                    Rule::Drop => {}
                    Rule::Image => {
                        let attrs = el.attrs.borrow();
                        if let Some(a) = attrs.iter().find(|a| &*a.name.local == "alt") {
                            s.extend(js_trim_alt(&a.value).chars().take(MAX_ALT));
                        }
                    }
                    _ => {
                        if brk {
                            s.push(' ');
                            stack.push(None);
                        }
                        let mut kids: Vec<Option<NodeRef>> = n.children().map(Some).collect();
                        kids.reverse();
                        stack.extend(kids);
                    }
                }
            }
            _ => {}
        }
        // assembly-038 (review): no 4 KiB cut any more — padding (zero-width spaces) before the host could hide it from
        // the claim check. The input is bounded (MAX_INPUT), and the web reads the whole text too.
        if s.len() > MAX_INPUT {
            break;
        }
    }
    s
}

fn is_language_class(c: &str) -> bool {
    c.strip_prefix("language-")
        .is_some_and(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
}

fn is_hex_colour(c: &str) -> bool {
    c.strip_prefix('#').is_some_and(|h| (3..=8).contains(&h.len()) && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn esc_text(out: &mut String, s: &str) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
}

fn esc_attr(out: &mut String, s: &str) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The link rule (one rule for the sanitiser's hrefs and for LinkGate.qml, which every link the app opens goes through)
// ---------------------------------------------------------------------------------------------------------------------

/// Why a link is refused (the shared vectors' `why`; LinkGate.qml says the same words).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refused {
    /// nothing, or no `scheme:` at the start
    NoScheme,
    /// a scheme other than http, https, mailto (file:, smb:, javascript:, data:, ms-*, search-ms:, custom schemes…)
    Scheme,
    /// http(s) without `//host`
    NoHost,
    /// a control character inside, a backslash, or a space in the host part
    BadChar,
    /// a user name / password before the host (`https://bank.example@evil.example/`)
    UserInfo,
}

impl Refused {
    pub fn as_str(self) -> &'static str {
        match self {
            Refused::NoScheme => "no-scheme",
            Refused::Scheme => "scheme",
            Refused::NoHost => "no-host",
            Refused::BadChar => "bad-char",
            Refused::UserInfo => "userinfo",
        }
    }
}

/// The link rule (Tin Hat Web's core/openlink.ts safeExternalUrl says yes to the same links). `Ok(url)` = the link as
/// it may be opened (spaces and controls trimmed from both ends). In this order: a control character left inside →
/// bad-char; no `scheme:` → no-scheme; a scheme other than http / https / mailto → scheme; a backslash → bad-char;
/// http(s) without `//` + a host (a space in it → bad-char, a user name before it → userinfo) → no-host.
/// Stricter than a browser where Qt's QUrl would read a link differently from WHATWG (a backslash, `https:host`,
/// `https:///host`): those are refused here, not repaired.
pub fn link_verdict(raw: &str) -> Result<String, Refused> {
    let s = raw.trim_matches(|c: char| c <= ' ').to_string();
    if s.chars().any(|c| c < ' ' || c == '\u{7f}') {
        return Err(Refused::BadChar);
    }
    let Some(colon) = s.find(':') else { return Err(Refused::NoScheme) };
    let scheme = &s[..colon];
    if scheme.is_empty()
        || !scheme.as_bytes()[0].is_ascii_alphabetic()
        || !scheme.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
    {
        return Err(Refused::NoScheme);
    }
    let scheme = scheme.to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https" | "mailto" | "steam") {
        return Err(Refused::Scheme);
    }
    if s.contains('\\') {
        return Err(Refused::BadChar);
    }
    if scheme == "steam" {
        // Decided 2026-10-26: steam:// links (join a server, run a game) are allowed — LinkGate.qml asks "Open in
        // Steam?" first (link_asks); every other app scheme stays refused
        let rest = &s[colon + 1..];
        let Some(after) = rest.strip_prefix("//").filter(|a| !a.is_empty()) else { return Err(Refused::NoHost) };
        if after.contains(' ') {
            return Err(Refused::BadChar);
        }
        return Ok(s);
    }
    if scheme != "mailto" {
        let rest = &s[colon + 1..];
        let Some(after) = rest.strip_prefix("//") else { return Err(Refused::NoHost) };
        let authority = after.split(['/', '?', '#']).next().unwrap_or("");
        if authority.contains(' ') {
            return Err(Refused::BadChar);
        }
        if authority.contains('@') {
            return Err(Refused::UserInfo);
        }
        let host = match authority.rfind(':') {
            Some(i) if !authority.ends_with(']') && authority[i + 1..].bytes().all(|b| b.is_ascii_digit()) => &authority[..i],
            _ => authority,
        };
        if host.is_empty() {
            return Err(Refused::NoHost);
        }
        if Url::parse(&s).ok().and_then(|u| u.host_str().map(str::to_string)).is_none() {
            return Err(Refused::NoHost);
        }
        return Ok(s);
    }
    Ok(mailto_kept(&s))
}

/// What an allowed link needs before it opens: `Some("steam")` = the "Open in Steam?" question (shared rule:
/// member-content.json `links` "ask"; LinkGate.qml asks it); None = nothing.
pub fn link_asks(url: &str) -> Option<&'static str> {
    let s = url.trim_matches(|c: char| c <= ' ');
    s.get(..6).filter(|p| p.eq_ignore_ascii_case("steam:")).map(|_| "steam")
}

/// A mailto link with only its `subject`, `body` and `cc` fields (launch-gate's review, 2026-10-26: `attach=` and the
/// like go to the mail client through xdg-open / ShellExecute). The address part as written; field names compared
/// case-insensitively; a fragment dropped. Shared rule (member-content.json `links` "as"; LinkGate.qml; the web).
pub fn mailto_kept(s: &str) -> String {
    let s = s.split('#').next().unwrap_or("");
    let Some((addr, query)) = s.split_once('?') else { return s.to_string() };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|p| {
            let k = p.split('=').next().unwrap_or("").to_ascii_lowercase();
            matches!(k.as_str(), "subject" | "body" | "cc")
        })
        .collect();
    if kept.is_empty() {
        addr.to_string()
    } else {
        format!("{addr}?{}", kept.join("&"))
    }
}

/// An href the bubble may carry: [`link_verdict`]'s, then written the WHATWG way (what `new URL(x).href` gives).
fn safe_href(raw: &str) -> Option<String> {
    let s = link_verdict(raw).ok()?;
    Url::parse(&s).ok().map(|u| u.to_string())
}

/// The host to show after a link whose text claims another one ([`claim_for`]).
fn spoof_host(href: &str, text: &str) -> Option<String> {
    claim_for(href, text).map(|(_, real)| real)
}

/// `(claimed, real)`: the link's text names another host than the http(s) link `href` opens (shared rule,
/// member-content.json `claims`; the note after a link and the click-time question, both apps). `real` = the link's
/// host, lower case, without a trailing dot or a leading `www.`. A matrix.to USER mention (a pill: its text is a display
/// name, its click opens the profile flyout — ChatBubble.qml mentionTarget) never claims; any other link to matrix.to
/// (a room, an event) is checked like every link (sec-night 2026-10-27, assembly verify B). The first claim (see
/// [`word_host`]) other than `real` counts.
pub fn claim_for(href: &str, text: &str) -> Option<(String, String)> {
    let u = Url::parse(href).ok()?;
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    let real = norm_host(u.host_str()?);
    if real == "matrix.to" && mention_href_re().is_match(href) {
        return None;
    }
    let t = invisible_re().replace_all(text, "");
    t.split_whitespace().filter_map(word_host).find(|c| *c != real).map(|c| (c, real))
}

/// The host one word of a link's text claims (launch-gate's review, 2026-10-26: punctuation around a host hid it, and
/// any dotted word — Node.js, package.json, Valheim.exe — counted). Leading and trailing characters that are not
/// letters or digits go (`(bank.com)`, `bank.com!`, `'bank.com'`, `bank.com.`); then the word claims a host when
///   - it has a scheme (`://`, http/https only), starts with `www.` or has a path (`/`; a `/` among the trimmed trailing
///     characters counts too: `bank.md/` is explicit, sec-night 2026-10-27, assembly verify A), and its host is dotted
///     with a last label of >= 2 ASCII letters or `xn--`; or
///   - it is a bare dotted name whose last label is a real top-level domain: testvectors/tlds.txt (IANA's list, shared
///     with the web) — js, json, exe, html are none.
fn word_host(word: &str) -> Option<String> {
    let w = edge_re().replace_all(word, "");
    if w.is_empty() {
        return None;
    }
    let trail = trail_re().find(word).map_or("", |m| m.as_str());
    let explicit = w.contains("://") || w.to_ascii_lowercase().starts_with("www.") || w.contains('/') || trail.contains('/');
    let full = if w.contains("://") { w.to_string() } else { format!("http://{w}") };
    let u = Url::parse(&full).ok()?;
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    let host = u.host_str()?;
    let h = host.trim_end_matches('.');
    if !h.contains('.') {
        return None;
    }
    let last = h.rsplit('.').next().unwrap_or("");
    let ok = if explicit {
        last.starts_with("xn--") || (last.len() >= 2 && last.bytes().all(|b| b.is_ascii_alphabetic()))
    } else {
        let l = last.to_ascii_lowercase();
        iana_tlds().contains(&l) && !CODE_FILE_TLDS.contains(&l.as_str())
    };
    ok.then(|| norm_host(host))
}

/// Real top-level domains that, as BARE dotted names (no scheme, no `www.`, no `/`), are code-file names far more often
/// than hosts: `README.md`, `main.py`, `lib.rs`, `run.sh`, `x.pl` claim nothing (zip and mov still do: phishing TLDs).
/// Shared rule (member-content.json `claimExclude`; coordinator's decision 2026-10-26).
pub const CODE_FILE_TLDS: &[&str] = &["md", "py", "rs", "sh", "pl"];

/// Leading / trailing characters that are not letters or digits (`^[^\p{L}\p{N}]+|[^\p{L}\p{N}]+$`).
fn edge_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^[^\p{L}\p{N}]+|[^\p{L}\p{N}]+$").unwrap())
}

/// Trailing characters that are not letters or digits (`[^\p{L}\p{N}]+$`): a `/` there still makes the word explicit.
fn trail_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"[^\p{L}\p{N}]+$").unwrap())
}

/// A matrix.to user mention href (ChatBubble.qml mentionTarget / the web's mention.ts, case-insensitive): the click
/// opens the profile, never a browser.
fn mention_href_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"(?i)^https://matrix\.to/#/(?:@|%40)[^/?#]+").unwrap())
}

/// The IANA top-level domains (testvectors/tlds.txt, shared with the web byte for byte), lower case.
fn iana_tlds() -> &'static std::collections::HashSet<String> {
    static S: std::sync::OnceLock<std::collections::HashSet<String>> = std::sync::OnceLock::new();
    S.get_or_init(|| {
        include_str!("testvectors/tlds.txt")
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(str::to_ascii_lowercase)
            .collect()
    })
}

/// `[\p{Cf}\p{Default_Ignorable_Code_Point}]` — Tin Hat Web's INVISIBLE (html.ts), the same Unicode classes.
fn invisible_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"[\p{Cf}\p{Default_Ignorable_Code_Point}]").unwrap())
}

fn norm_host(h: &str) -> String {
    let h = h.trim_end_matches('.').to_ascii_lowercase();
    h.strip_prefix("www.").map(str::to_string).unwrap_or(h)
}

// ---------------------------------------------------------------------------------------------------------------------
// The timeline's entry point
// ---------------------------------------------------------------------------------------------------------------------

/// A row's `formatted`: the member's HTML sanitised ([`sanitize`], cached by input), then the spoilers hidden in the
/// active theme's colours (`embeds::hide_spoilers`). "" when there is none or it is too big to render as rich text.
pub fn message_html(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    static CACHE: std::sync::OnceLock<std::sync::Mutex<HtmlCache>> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let hit = cache.lock().unwrap_or_else(|p| p.into_inner()).get(raw);
    let clean = match hit {
        Some(c) => c,
        None => {
            let c = sanitize(raw).unwrap_or_default();
            cache.lock().unwrap_or_else(|p| p.into_inner()).put(raw, &c);
            c
        }
    };
    crate::embeds::hide_spoilers(&clean)
}

/// The sanitised-HTML cache with a byte budget (Astra review 2026-10-26: 2 000 entries of any size could hold ~480 MB):
/// inputs + outputs at most [`CACHE_BYTES`], the oldest entry out first; one entry over a sixteenth of it isn't kept.
#[derive(Default)]
struct HtmlCache {
    map: std::collections::HashMap<String, String>,
    order: std::collections::VecDeque<String>,
    bytes: usize,
}
impl HtmlCache {
    fn get(&self, raw: &str) -> Option<String> {
        self.map.get(raw).cloned()
    }
    fn put(&mut self, raw: &str, clean: &str) {
        let cost = raw.len() + clean.len();
        if cost > CACHE_BYTES / 16 || self.map.contains_key(raw) {
            return;
        }
        while self.bytes + cost > CACHE_BYTES {
            let Some(old) = self.order.pop_front() else { break };
            if let Some(v) = self.map.remove(&old) {
                self.bytes -= old.len() + v.len();
            }
        }
        self.map.insert(raw.to_string(), clean.to_string());
        self.order.push_back(raw.to_string());
        self.bytes += cost;
    }
}

/// `(claimed, real)` when the link `href` in this app's own sanitised `html` (a row's `formatted`) has text that names
/// another host than the one it opens. Review follow-up (Astra, 2026-10-26): the " (real-host)" note after a link sits
/// inside the sender's own formatting, which can hide it (white on white) or stop it (a hidden word inside the link
/// makes the text no single address). So the bubble asks this at the click and, when it answers, opens the link only
/// after the app's own dialog — Tin Hat Web's wording: "The text says {claimed}, but this link opens {real}." Every
/// whitespace-separated word of the link's text (hidden parts too, invisible characters dropped) is read as an address
/// ([`claim_for`], the shared rule); the first one naming a host other than the link's is the claim. `real` = the link's host, lower
/// case, without `www.`. None for a link that shows no other host (its own address, words, a mailto).
pub fn link_claim(html: &str, href: &str) -> Option<(String, String)> {
    let doc = Html::parse(html);
    let mut stack: Vec<NodeRef> = doc.children().collect();
    while let Some(n) = stack.pop() {
        if let Some(el) = n.as_element() {
            if &*el.name.local == "a" && attr_of(el, "href").as_deref() == Some(href) {
                if let Some(c) = claim_for(href, &text_of(&n)) {
                    return Some(c);
                }
            }
        }
        stack.extend(n.children());
    }
    None
}

/// The plain text of some HTML the way a reader sees it (text nodes, entities decoded) — the vectors' `text`.
pub fn visible_text(html: &str) -> String {
    let doc = Html::parse(html);
    let mut s = String::new();
    let mut stack: Vec<NodeRef> = doc.children().collect();
    stack.reverse();
    while let Some(n) = stack.pop() {
        match n.data() {
            NodeData::Text(t) => s.push_str(&t.borrow()),
            NodeData::Element(_) => {
                let mut kids: Vec<NodeRef> = n.children().collect();
                kids.reverse();
                stack.extend(kids);
            }
            _ => {}
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// Review follow-up (Astra + the web lane): a link whose text names another host is asked about at the click —
    /// also when the sender's colours hide the note, a hidden word inside the link stops it, or an invisible
    /// character sits in the host — with the web's wording.
    #[test]
    fn host_claims_are_confirmed_at_the_click() {
        let claim = |h: &str, href: &str| link_claim(&sanitize(h).unwrap(), href);
        let ev = || Some(("bank.example".to_string(), "evil.example".to_string()));
        assert_eq!(claim(r##"<span data-mx-color="#ffffff" data-mx-bg-color="#ffffff"><a href="https://evil.example" data-mx-color="#000000">https://bank.example</a></span>"##, "https://evil.example/"), ev());
        let stopped = sanitize(r##"<a href="https://evil.example">https://bank.example<span data-mx-color="#ffffff" data-mx-bg-color="#ffffff"> x</span></a>"##).unwrap();
        // Astra found the note stopped by a hidden word; reading the text word by word (launch-gate's review) brings it back
        assert!(stopped.contains("(evil.example)"), "{stopped}");
        assert_eq!(link_claim(&stopped, "https://evil.example/"), ev());
        assert_eq!(claim(r#"<a href="https://evil.example/">x <b>https://www.bank.example/login</b></a>"#, "https://evil.example/"), ev());
        assert_eq!(
            claim("<a href=\"https://evil.example/\">example.com\u{200e}</a>", "https://evil.example/"),
            Some(("example.com".to_string(), "evil.example".to_string()))
        );
        // no claim: its own address (any spelling), plain words, a mailto
        for (h, href) in [
            (r#"<a href="https://example.org">https://example.org</a>"#, "https://example.org/"),
            (r#"<a href="https://www.example.org/a">example.org/a</a>"#, "https://www.example.org/a"),
            (r#"<a href="https://example.org/docs">the docs</a>"#, "https://example.org/docs"),
            (r#"<a href="mailto:ana@bank.example">bank.example</a>"#, "mailto:ana@bank.example"),
        ] {
            assert_eq!(claim(h, href), None, "{h}");
        }
        assert_eq!(link_claim("<p>x</p>", "https://example.org/"), None);
    }

    /// The web lane's regression guard (its old regex hide_spoilers): a spoiler reason that is itself markup stays text
    /// through the sanitiser AND hide_spoilers — nothing in it becomes a tag.
    #[test]
    fn a_spoiler_reason_never_becomes_markup() {
        let p = r#"<img src=https://attacker.example/pixel onerror=alert(1)>"#;
        for html in [
            format!(r#"<div data-mx-spoiler="<span data-mx-spoiler>{p}</span>"></div>"#),
            format!(r#"<span data-mx-spoiler="<span data-mx-spoiler>{p}</span>"></span>"#),
            format!(r#"<span data-mx-spoiler="{p}">x</span>"#),
        ] {
            let shown = message_html(&html);
            assert!(!shown.contains("<img") && !shown.contains("<span data-mx-spoiler>"), "{html} → {shown}");
            let doc = Html::parse(&shown);
            let mut stack: Vec<NodeRef> = doc.children().collect();
            while let Some(n) = stack.pop() {
                if let Some(el) = n.as_element() {
                    assert!(matches!(&*el.name.local, "html" | "head" | "body" | "div" | "span"), "{shown}");
                }
                stack.extend(n.children());
            }
        }
    }

    /// Review follow-up (Astra): what one body may cost is bounded, and the cache by bytes.
    #[test]
    fn bodies_and_the_cache_are_bounded() {
        assert!(sanitize(&"&".repeat(40_000)).is_some(), "a big but allowed body still renders");
        assert_eq!(sanitize(&"a".repeat(MAX_INPUT + 1)), None);
        assert_eq!(sanitize(&"&".repeat(MAX_OUTPUT / 5 + 1)), None, "&amp; × n would exceed MAX_OUTPUT");
        let mut c = HtmlCache::default();
        for i in 0..2001 {
            let raw = format!("{i}{}", "&".repeat(40_000));
            let clean = "x".repeat(200_000);
            c.put(&raw, &clean);
            assert!(c.bytes <= CACHE_BYTES);
        }
        assert!(c.map.len() < 2001 && c.bytes <= CACHE_BYTES);
        c.put("big", &"y".repeat(CACHE_BYTES));
        assert!(c.get("big").is_none(), "one entry over a sixteenth of the budget isn't kept");
        c.put("small", "s");
        assert_eq!(c.get("small").as_deref(), Some("s"));
    }

    pub(crate) const VECTORS: &str = include_str!("testvectors/member-content.json");

    fn vectors() -> Value {
        serde_json::from_str(VECTORS).expect("member-content.json parses")
    }

    fn hrefs_of(html: &str) -> Vec<String> {
        let doc = Html::parse(html);
        let mut out = Vec::new();
        let mut stack: Vec<NodeRef> = doc.children().collect();
        while let Some(n) = stack.pop() {
            if let Some(el) = n.as_element() {
                if &*el.name.local == "a" {
                    if let Some(a) = el.attrs.borrow().iter().find(|a| &*a.name.local == "href") {
                        out.push(a.value.to_string());
                    }
                }
            }
            stack.extend(n.children());
        }
        out.sort();
        out
    }

    #[test]
    fn html_vectors() {
        let v = vectors();
        let cases = v["html"].as_array().expect("html cases");
        assert!(cases.len() >= 40, "{} html cases", cases.len());
        let mut fails = Vec::new();
        for c in cases {
            let name = c["name"].as_str().unwrap();
            let input = c["in"].as_str().unwrap();
            let out = sanitize(input).expect("a vector is small");
            if let Some(q) = c.get("qt").and_then(Value::as_str) {
                if out != q {
                    fails.push(format!("{name}: qt\n  got  {out:?}\n  want {q:?}"));
                }
            }
            if let Some(t) = c.get("text").and_then(Value::as_str) {
                let got = visible_text(&out);
                if got != t {
                    fails.push(format!("{name}: text\n  got  {got:?}\n  want {t:?}"));
                }
            }
            if let Some(h) = c.get("hrefs").and_then(Value::as_array) {
                let mut want: Vec<String> = h.iter().map(|x| x.as_str().unwrap().to_string()).collect();
                want.sort();
                let got = hrefs_of(&out);
                if got != want {
                    fails.push(format!("{name}: hrefs {got:?} want {want:?}"));
                }
            }
            for a in c.get("absent").and_then(Value::as_array).into_iter().flatten() {
                let a = a.as_str().unwrap();
                if out.to_ascii_lowercase().contains(&a.to_ascii_lowercase()) {
                    fails.push(format!("{name}: {a:?} survived in {out:?}"));
                }
            }
            // sanitising the output again keeps it safe and says the real host once
            let again = sanitize(&out).unwrap();
            for a in c.get("absent").and_then(Value::as_array).into_iter().flatten() {
                if again.to_ascii_lowercase().contains(&a.as_str().unwrap().to_ascii_lowercase()) {
                    fails.push(format!("{name}: second pass {again:?}"));
                }
            }
            if visible_text(&again) != visible_text(&out) {
                fails.push(format!("{name}: second pass changed the text: {again:?}"));
            }
        }
        assert!(fails.is_empty(), "{} failing html vectors:\n{}", fails.len(), fails.join("\n"));
    }

    /// The shared host-claim rule (member-content.json `claims`, launch-gate's review): punctuation, file names,
    /// mention pills, invisible characters, the IANA list.
    #[test]
    fn claim_vectors() {
        let v = vectors();
        let cases = v["claims"].as_array().expect("claims cases");
        assert!(cases.len() >= 20);
        let mut fails = Vec::new();
        for c in cases {
            let (text, href) = (c["text"].as_str().unwrap(), c["href"].as_str().unwrap());
            let got = claim_for(href, text).map(|(claimed, _)| claimed);
            if got.as_deref() != c["claim"].as_str() {
                fails.push(format!("{text:?} → {href}: {got:?}, want {:?}", c["claim"]));
            }
        }
        assert!(fails.is_empty(), "{}", fails.join("\n"));
        assert!(iana_tlds().contains("com") && iana_tlds().contains("md") && !iana_tlds().contains("js") && !iana_tlds().contains("json"));
        let ex: Vec<&str> = v["claimExclude"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
        assert_eq!(ex, CODE_FILE_TLDS);
        assert!(iana_tlds().len() > 1000);
    }

    #[test]
    fn link_vectors() {
        let v = vectors();
        let cases = v["links"].as_array().expect("link cases");
        assert!(cases.len() >= 30);
        let mut fails = Vec::new();
        for c in cases {
            let url = c["url"].as_str().unwrap();
            let want = c["open"].as_bool().unwrap();
            let got = link_verdict(url);
            if got.is_ok() != want {
                fails.push(format!("{url:?}: {got:?}, want open={want}"));
            }
            if got.is_ok() && link_asks(url) != c.get("ask").and_then(Value::as_str) {
                fails.push(format!("{url:?}: asks {:?}, want {:?}", link_asks(url), c.get("ask")));
            }
            if let (Ok(o), Some(w)) = (&got, c.get("as").and_then(Value::as_str)) {
                if o != w {
                    fails.push(format!("{url:?}: opens {o:?}, want {w:?}"));
                }
            }
            if let (Err(r), Some(w)) = (got, c.get("why").and_then(Value::as_str)) {
                if r.as_str() != w {
                    fails.push(format!("{url:?}: why {} want {w}", r.as_str()));
                }
            }
        }
        assert!(fails.is_empty(), "{}", fails.join("\n"));
    }

    /// Where Qt's QUrl and a browser's URL parser read a link differently, this app refuses it (Tin Hat Web repairs
    /// it through `new URL`); not in the shared vectors for that reason.
    #[test]
    fn links_qt_would_misread_are_refused() {
        for (url, why) in [
            ("https:evil.example", Refused::NoHost),
            ("https:///evil.example", Refused::NoHost),
            ("https:\\\\evil.example", Refused::BadChar),
            ("https://evil.example\\@good.example/", Refused::BadChar),
            ("https://good.example\\.evil.example/", Refused::BadChar),
        ] {
            assert_eq!(link_verdict(url), Err(why), "{url}");
        }
        for (html, text) in [
            ("<a href=\"https:evil.example\">x</a>", "x"),
            ("<a href=\"https://evil.example\\@good.example/\">x</a>", "x"),
        ] {
            assert_eq!(sanitize(html).unwrap(), text, "{html}");
        }
    }

    #[test]
    fn too_much_markup_is_not_rich_text() {
        let s = "<b>x</b>".repeat(MAX_TAGS / 2);
        assert!(sanitize(&s).is_some(), "exactly at the limit still renders");
        let s = "<b>x</b>".repeat(MAX_TAGS / 2 + 1);
        assert_eq!(sanitize(&s), None);
        assert_eq!(message_html(&s), "");
        assert_eq!(message_html(""), "");
    }

    /// assembly-038: padding before the named host can't push it out of the claim check (the web reads all the text)
    #[test]
    fn a_padded_link_text_still_names_its_host() {
        let pad = "\u{200b}".repeat(4097);
        let html = format!(r#"<a href="https://evil.example/login"><span>{pad}</span>paypal.com</a>"#);
        let out = sanitize(&html).expect("rendered");
        assert!(out.contains("(evil.example)"), "the real host is shown after the link: {}", &out[out.len().saturating_sub(200)..]);
    }

    /// Deep nesting: no stack overflow on a tokio-sized (2 MiB) stack, markup past MAX_DEPTH flattened, text kept.
    #[test]
    fn deep_nesting_is_flattened_on_a_small_stack() {
        let h = std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(|| {
                let n = MAX_TAGS / 2;
                let s = format!("{}deep{}", "<div>".repeat(n), "</div>".repeat(n));
                let out = sanitize(&s).unwrap();
                assert_eq!(visible_text(&out), "deep");
                assert_eq!(out.matches("<div>").count(), MAX_DEPTH, "{} divs", out.matches("<div>").count());
                let s = format!("{}x", "<b><i>".repeat(MAX_TAGS / 2));
                let out = sanitize(&s).unwrap();
                assert_eq!(visible_text(&out), "x");
            })
            .unwrap();
        h.join().expect("no overflow");
    }

    #[test]
    fn message_html_hides_spoilers_after_sanitising() {
        let s = message_html("before <span data-mx-spoiler=\"plot\" style=\"x\">the <b>butler</b> did it</span> after");
        assert!(s.contains("[spoiler: plot — select to reveal]"), "{s}");
        assert!(!s.contains("data-mx-spoiler"), "{s}");
        assert!(s.contains(">the butler did it</span>"), "{s}");
        // the reason is text: markup in it shows as text
        let s = message_html("<span data-mx-spoiler=\"&lt;img src=x&gt;\">x</span>");
        assert!(s.contains("[spoiler: &lt;img src=x&gt; — select to reveal]"), "{s}");
        // twice the same input: the cache answers the same
        assert_eq!(message_html("<p onclick=\"x\">a</p>"), "<p>a</p>");
        assert_eq!(message_html("<p onclick=\"x\">a</p>"), "<p>a</p>");
    }

    #[test]
    fn our_own_mention_html_survives() {
        // room.rs render_mention_html (the composer's echo) goes through unchanged
        let m = "look &lt;b&gt; &amp; <a href=\"https://matrix.to/#/@bob:example.com\">@Bob</a><br/>bye";
        assert_eq!(sanitize(m).unwrap(), "look &lt;b&gt; &amp; <a href=\"https://matrix.to/#/@bob:example.com\">@Bob</a><br>bye");
    }

    /// The byte copy equals the web's vectors (`TINHAT_WEB_DIR` = a tinhat-web tree, else `$HOME/tinhat-web`;
    /// skipped when neither has the file).
    #[test]
    fn member_content_vectors_are_the_webs() {
        let dir = std::env::var_os("TINHAT_WEB_DIR")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("tinhat-web")));
        let Some(dir) = dir else { return };
        let path = dir.join("src/features/chat/lib/testvectors/member-content.json");
        match std::fs::read_to_string(&path) {
            Ok(theirs) => {
                assert!(VECTORS == theirs, "testvectors/member-content.json differs from {}", path.display());
                eprintln!("member-content vectors: byte-identical to {}", path.display());
            }
            Err(_) => eprintln!("SKIPPED: no web vectors at {}", path.display()),
        }
    }
}
