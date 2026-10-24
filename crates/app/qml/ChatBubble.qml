// ChatBubble.qml — polished message bubble for Tin Hat.
//
// Replaces the inline bubbleRect: a sender Avatar (hidden when grouped under the
// previous message), a colored sender name, a bubble with a soft drop shadow, a
// fox-flame gradient for your own messages (furry) / accent->darker (otherwise),
// rounded corners (radiusXl furry / radiusLg otherwise), a timestamp, and a slot
// for a reactions row.
//
// Layout: the whole component is a row that fills the list width. Your own
// messages right-align (avatar gutter kept for rhythm); others left-align with the
// avatar in the gutter. Grouped messages collapse the avatar + name and tighten
// spacing for a clean Element/Telegram-style thread.
//
// CXX-Qt note: everything arrives via plain properties from the delegate's
// modelData — no direct controller getter bindings.
import QtQuick
import QtQuick.Layouts
import QtMultimedia
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the bubble.* / timeline.row.* tokens
import Qt5Compat.GraphicalEffects   // 0.8.30: OpacityMask — rounded photo corners (GIFs: a CornerMask since 11.0)

Item {
    id: bubble
    required property var app   // ApplicationWindow (theme tokens); set by parent
    FallbackTheme { id: fallbackTheme }
    readonly property var theme: app || fallbackTheme

    // ---- public API ----
    property bool mine: false
    property bool grouped: false
    property string body: ""
    property string sender: ""          // mxid
    property string senderName: ""      // resolved display (falls back to localpart)
    // 10.4 (space tags): the sender's highest CUSTOM role in the room's space — colours the name
    // and wears a tag next to it. Empty name = none (DMs, rooms outside a space, template roles).
    property string roleName: ""
    property string roleColor: ""
    property double ts: 0
    property real maxBubble: 320
    property string avatarSource: ""
    // ---- 9.3 ----
    property string replyToId: ""       // quoted parent id (click the quote → jump to it)
    property string mediaBusy: ""       // 10.1 (TH-MSG-13): event id the controller is fetching for Open / Save as
    readonly property bool opening: mediaBusy.length > 0 && mediaBusy === eventId
    property bool pending: false        // local echo, not yet accepted by the server
    property bool failed: false         // local echo the server refused / never got
    property string sendError: ""       // why (friendly)
    property string txn: ""             // the echo's transaction id (retry / discard)
    property bool mentionsMe: false     // @me / @room / my name → the highlight wash
    property bool pinned: false
    property bool canPin: false
    property bool flash: false          // briefly lit after a jump-to
    property var preview: null          // link preview descriptor (LinkPreviewCard)
    signal replyRequested()
    signal editRequested()
    signal moreReactRequested()
    signal quoteClicked(string eventId)
    signal retryRequested(string txn)
    signal discardRequested(string txn)
    signal pinRequested(bool on)
    readonly property bool isEmote: msgtype === "m.emote"
    // Emoji-only messages (up to three) render big, the way every chat app does it. Counts
    // pictographic code points after stripping joiners / variation selectors / skin tones.
    readonly property bool jumbo: {
        if (formatted.length > 0 || isMedia || redacted || isEmote) return false
        var s = String(body).trim()
        if (s.length === 0 || s.length > 24) return false
        var n = 0
        for (var i = 0; i < s.length; ) {
            var c = s.codePointAt(i)
            i += c > 0xffff ? 2 : 1
            if (c === 0x200d || c === 0xfe0f || c === 0xfe0e || (c >= 0x1f3fb && c <= 0x1f3ff) || c === 0x20e3) continue
            if (c === 0x20) continue
            var emoji = (c >= 0x1f000 && c <= 0x1faff) || (c >= 0x2600 && c <= 0x27bf) || (c >= 0x2b00 && c <= 0x2bff)
                     || c === 0x2122 || c === 0x2139 || (c >= 0x2190 && c <= 0x21ff) || c === 0x231a || c === 0x231b || c === 0x23e9 || c === 0x23f0 || (c >= 0x25aa && c <= 0x25fe) || c === 0x3030 || c === 0x303d
            if (!emoji) return false
            n++
        }
        return n >= 1 && n <= 3
    }
    property string formatted: ""       // HTML formatted_body (rich text); empty = plain
    property string msgtype: "m.text"
    property string mimetype: ""
    property int durationMs: 0
    property real fileSize: 0
    property string media: ""           // decrypted file:// path for an m.image (else "")
    property int imageW: 0              // event-provided pixel width  (0 = unknown → use decoded size)
    property int imageH: 0              // event-provided pixel height (0 = unknown)
    signal imageClicked(string url, int w, int h, string alt) // open the zoomable lightbox
    signal mediaClicked(string eventId)
    signal saveRequested(string eventId, string suggestedName)
    signal videoPlayRequested(string eventId)
    /// {"id","url","loading"} from room.mediaResolved, forwarded by Main.qml. Only the
    /// bubble whose id matches reacts, so one shared property drives every video.
    property var resolvedMedia: ({})
    readonly property bool videoIsMine: resolvedMedia && resolvedMedia.id === eventId
    readonly property string videoUrl: videoIsMine && resolvedMedia.url ? resolvedMedia.url : ""
    readonly property bool videoLoading: videoIsMine && resolvedMedia.loading === true // file/video/audio chip → download & open
    property var embeds: []             // inline media embeds (youtube/gif/direct media)
    property bool onlyEmbed: false      // body is only an embeddable link → hide raw url
    // 0.8.33: a calendar announcement (dev.tinhat.calendar {container, event}) → live card.
    property string calContainer: ""
    property string calEvent: ""
    signal calendarClicked(string container, string eventId)
    signal calendarRsvp(string container, string eventId, string status)
    // 11.0 step 5 (THEMES-UI.md §14): a theme share card — the row's `themeShare` (ok / refused / newer, read by this
    // receiver from the ORIGINAL content); the body text hides while the card shows
    property var themeShare: null
    readonly property bool hasThemeCard: bubble.themeShare !== null && bubble.themeShare !== undefined && !bubble.redacted
    // widgets W3 (WIDGETS-UI.md §16.4): a dice roll — the row's `dice` (the ORIGINAL content's dev.tinhat.dice, read by the
    // shared reader: a card only when it re-adds up); the card replaces the body text, the message is never edited
    property var dice: null
    readonly property bool hasDiceCard: bubble.dice !== null && bubble.dice !== undefined && !bubble.redacted
    // widgets W3 (WIDGETS-UI.md §17.4): a poll — the row's `poll` (polls_ctl.rs card_json: the shared reader + tally); the
    // card replaces the body text (vote, results, End poll → pollVote / pollEnd for the conversation's RoomController)
    property var poll: null
    readonly property bool hasPollCard: bubble.poll !== null && bubble.poll !== undefined && !bubble.redacted
    signal pollVote(var answers)
    signal pollEnd()
    /// ENGINE.md §10: a theme's code font (type.code) on the code of a formatted message (inline and blocks) — the rich
    /// text's <code> / <pre> get it as their font-family (the allowed families are plain words); without one, as sent
    function _codeFont(html) {
        if (Th.fontCode.length === 0 || (html.indexOf("<code") < 0 && html.indexOf("<pre") < 0)) return html
        return html.replace(/<(code|pre)(?=[\s>])/g, "<$1 style=\"font-family:'" + Th.fontCode + "'\"")
    }
    Component {
        id: videoSurface
        VideoOutput {
            id: vout
            MediaPlayer {
                id: vplayer
                source: bubble.videoUrl
                videoOutput: vout
                audioOutput: AudioOutput {}
                Component.onCompleted: play()
            }
            MouseArea {
                anchors.fill: parent
                onClicked: vplayer.playbackState === MediaPlayer.PlayingState
                           ? vplayer.pause() : vplayer.play()
            }
        }
    }

    // "Animated", really: GIF, WebP and APNG all go through AnimatedImage (a still WebP
    // renders the same there). Keyed on mimetype OR filename OR the cached path, because
    // bridged/bot images often arrive without info.mimetype.
    readonly property bool isGif: /gif|webp|apng/.test((mimetype || "").toLowerCase())
                                 || /\.(gif|webp|apng)$/.test((body || "").toLowerCase())
                                 || /\.(gif|webp|apng)($|\?)/.test((media || "").toLowerCase())
    // 11.0 (GPU.md §2.6): inside the timeline's viewport (ConversationView) — an animated image plays only while it is
    property bool inView: true
    // my bubble's fill at a height t (0 = top, 1 = bottom): the gradient overlay's colour there (the GIF's corner bites)
    function ownFillAt(t) {
        var a = Th.bubbleOwnTopBg, b = Th.bubbleOwnBottomBg, k = Math.max(0, Math.min(1, t))
        return Qt.rgba(a.r + (b.r - a.r) * k, a.g + (b.g - a.g) * k, a.b + (b.b - a.b) * k, 1)   // theme-lint: ok — a mix of two tokens (the gradient at a height), not a literal
    }

    /// Human file size. Deliberately 1 decimal place: "4.7 MB" reads, "4718592" does not.
    function fmtSize(bytes) {
        if (!bytes || bytes <= 0) return ""
        var u = ["B", "KB", "MB", "GB"]
        var i = 0
        var n = bytes
        while (n >= 1024 && i < u.length - 1) { n /= 1024; i++ }
        return (i === 0 ? n : n.toFixed(1)) + " " + u[i]
    }

    /// A themed icon for the file, chosen from the mimetype first (authoritative) and
    /// the extension second (present even when the sender omitted the mimetype).
    function fileIconName(name, mime) {
        var m = (mime || "").toLowerCase()
        var n = (name || "").toLowerCase()
        function ext(list) {
            for (var i = 0; i < list.length; i++)
                if (n.endsWith("." + list[i])) return true
            return false
        }
        if (m.indexOf("image/") === 0 || ext(["png", "jpg", "jpeg", "gif", "webp", "svg"]))
            return "image-x-generic"
        if (m.indexOf("audio/") === 0 || ext(["mp3", "ogg", "opus", "flac", "wav", "m4a"]))
            return "audio-x-generic"
        if (m.indexOf("video/") === 0 || ext(["mp4", "mkv", "webm", "mov", "avi"]))
            return "video-x-generic"
        if (m.indexOf("pdf") >= 0 || ext(["pdf"])) return "application-pdf"
        if (ext(["zip", "gz", "xz", "zst", "7z", "rar", "tar"])) return "application-x-archive"
        if (ext(["txt", "md", "log", "csv", "json", "yaml", "yml", "toml"])) return "text-x-generic"
        return "application-octet-stream"
    }

    /// The user id a matrix.to mention link points at, or "" for any other link. Mirrors Tin Hat
    /// Web's html.ts (2026-09-25): a percent-encoded mention (…/#/%40user%3Aserver) is a mention too,
    /// and decodeURIComponent throws on a stray '%' — fall back to the raw id instead of aborting
    /// the click handler. (Gate: scripts/mention-link-gate.sh runs this very function.)
    function mentionTarget(link) {
        // Same rule as the web's mention.ts mentionFromHref(), byte for byte in behaviour (shared vectors in
        // scripts/mention-link-gate.sh and the web's mention.test.ts): case-insensitive, and on a malformed
        // escape only %40 / %3A are decoded; the result must be "@" + something.
        var m = /^https:\/\/matrix\.to\/#\/((?:@|%40)[^/?#]*)/i.exec(String(link))
        if (!m) return ""
        var id
        try { id = decodeURIComponent(m[1]) } catch (e) { id = m[1].replace(/%40/gi, "@").replace(/%3a/gi, ":") }
        return (id.length > 1 && id.charAt(0) === "@") ? id : ""
    }

    /// mm:ss for the audio player; ms may be 0 before the file loads.
    function fmtTime(ms) {
        if (!ms || ms < 0) ms = 0
        var total = Math.floor(ms / 1000)
        var m = Math.floor(total / 60)
        var s = total % 60
        return m + ":" + (s < 10 ? "0" : "") + s
    }

    readonly property bool isMedia: msgtype === "m.image" || msgtype === "m.file"
                                     || msgtype === "m.video" || msgtype === "m.audio"
    property string replyToSender: ""   // quoted-parent author (empty = not a reply)
    property string replyToBody: ""     // quoted-parent snippet
    property bool edited: false         // show an "(edited)" marker
    property bool redacted: false       // render a "message deleted" placeholder
    property bool undecryptable: false  // 10.1 (TH-MSG-12): this device has no key — a locked placeholder, no actions
    // Reactions: array of { key, count, mine, reactionId }. Chips toggle on click.
    property var reactions: []
    // Read receipts: array of { userId, name } — members who've read up to this message.
    property var receipts: []
    property string eventId: ""
    signal react(string key)
    signal unreact(string reactionId)
    signal menuRequested()   // right-click / long-press → parent opens the action menu
    signal avatarClicked(string mxid)  // left-click on the sender avatar → profile flyout

    readonly property real avatarSize: 34
    readonly property real gutter: avatarSize + Kirigami.Units.largeSpacing
    readonly property real bubbleRadius: bubble.theme.radiusLg
    // 10.4: a space role's colour wins over the hashed hue.
    readonly property color senderHue: {
        if (bubble.roleColor.length > 0) { var rc = Qt.color(bubble.roleColor); if (rc.valid) return rc }   // theme-lint: ok content: the space role's colour
        return bubble.theme.senderColor(sender)
    }
    readonly property string displayName: senderName.length > 0 ? senderName : bubble.theme.localpart(sender)

    implicitHeight: rowLayout.implicitHeight
    width: ListView.view ? ListView.view.width : implicitWidth

    // 10.1 (TH-MSG-21): the bar sits above the bubble unless that would clip at the top of the
    // view — then it tucks inside the bubble's own top edge. Decided at hover time.
    property var hostView: null
    property bool barAbove: true
    HoverHandler { id: bubbleHover; onHoveredChanged: if (hovered) bubble.barAbove = !bubble.hostView || bubble.mapToItem(bubble.hostView, 0, 0).y > 40 }
    // 9.3: a wash behind the row — amber when the line addresses me, accent when it was
    // just jumped to (a reply quote / a pin). Never both at full strength.
    Rectangle {
        anchors.fill: parent
        anchors.leftMargin: -Kirigami.Units.smallSpacing
        anchors.rightMargin: -Kirigami.Units.smallSpacing
        radius: 6
        visible: bubble.mentionsMe || bubble.flash
        // themes: the tokens carry the wash's alpha; it stays the item's opacity (docs/MIGRATING-THEME.md §3.8 — an
        // alpha colour would move anti-aliased pixels), so the colour is the token at full alpha.
        color: bubble.flash ? Qt.alpha(Th.timelineRowBgActive, 1) : Qt.alpha(Th.timelineRowBgMention, 1)
        opacity: bubble.flash ? Th.timelineRowBgActive.a : Th.timelineRowBgMention.a
        Behavior on opacity { NumberAnimation { duration: Motion.durLong } }
        Rectangle {
            anchors.left: parent.left; anchors.top: parent.top; anchors.bottom: parent.bottom
            width: 3; radius: 1.5
            // the mention stripe (the wash's colour while a jump flashes it), at the wash's opacity
            color: bubble.flash ? Qt.alpha(Th.timelineRowBgActive, 1) : Qt.alpha(Th.timelineRowStripeMention, 1)
            opacity: 1.0
            visible: bubble.mentionsMe
        }
    }

    RowLayout {
        id: rowLayout
        // A message still on its way is dimmed; the server's copy un-dims it.
        opacity: bubble.pending ? 0.55 : 1.0
        Behavior on opacity { NumberAnimation { duration: Motion.durLong } }
        width: parent.width
        spacing: Kirigami.Units.smallSpacing
        layoutDirection: bubble.mine ? Qt.RightToLeft : Qt.LeftToRight

        // Avatar gutter — present for incoming messages, hidden (but spaced) when
        // grouped so the thread aligns. Own messages drop the avatar entirely.
        Item {
            Layout.alignment: Qt.AlignTop
            Layout.leftMargin: bubble.mine ? 0 : Kirigami.Units.smallSpacing
            Layout.rightMargin: bubble.mine ? Kirigami.Units.smallSpacing : 0
            implicitWidth: bubble.mine ? 0 : bubble.avatarSize
            implicitHeight: bubble.avatarSize
            visible: !bubble.mine
            Avatar {
                app: bubble.app
                anchors.fill: parent
                visible: !bubble.grouped
                name: bubble.displayName
                seed: bubble.sender
                size: bubble.avatarSize
                imageSource: bubble.avatarSource
                // Subtle hover lift so the click-for-profile affordance reads.
                scale: avatarHover.hovered ? 1.08 : 1.0
                Behavior on scale { NumberAnimation { duration: bubble.theme.dur; easing.type: Easing.OutCubic } }
                TapHandler {
                    acceptedButtons: Qt.LeftButton
                    onTapped: bubble.avatarClicked(bubble.sender)
                }
                HoverHandler { id: avatarHover; cursorShape: Qt.PointingHandCursor }
            }
        }

        // Bubble column: name (incoming, ungrouped) + bubble + reactions.
        ColumnLayout {
            Layout.fillWidth: true
            Layout.maximumWidth: bubble.maxBubble + bubble.gutter
            Layout.topMargin: bubble.grouped ? 1 : Kirigami.Units.smallSpacing
            spacing: 2

            // Sender name (colored), only for incoming + ungrouped. 10.4: the name wears the
            // sender's space role — its colour, and a RoleBadge tag right after it.
            RowLayout {
                Layout.alignment: bubble.mine ? Qt.AlignRight : Qt.AlignLeft
                Layout.maximumWidth: bubble.maxBubble
                Layout.fillWidth: false   // a nested layout fills by default — the tag would be justified to the far edge
                visible: !bubble.grouped && !bubble.mine && !bubble.isEmote
                spacing: 6
                Controls.Label {
                    Layout.maximumWidth: bubble.maxBubble - (roleTag.visible ? roleTag.implicitWidth + 6 : 0)
                    text: bubble.displayName
                    font.bold: true
                    font.pixelSize: Math.round(Kirigami.Theme.smallFont.pixelSize)
                    color: bubble.senderHue
                    elide: Text.ElideRight
                    font.underline: nameHover.hovered
                    HoverHandler { id: nameHover; cursorShape: Qt.PointingHandCursor }
                    TapHandler { acceptedButtons: Qt.LeftButton; onTapped: bubble.avatarClicked(bubble.sender) }
                }
                RoleBadge {
                    id: roleTag
                    Layout.alignment: Qt.AlignVCenter
                    app: bubble.app
                    compact: true
                    name: bubble.roleName
                    roleColor: bubble.roleColor
                }
            }

            // The bubble itself — ShadowedRectangle gives a soft drop shadow and
            // per-corner radius (a tucked corner on the speaker's side reads as a
            // chat tail without an extra triangle).
            Kirigami.ShadowedRectangle {
                id: bubbleRect
                Layout.alignment: bubble.mine ? Qt.AlignRight : Qt.AlignLeft
                Layout.maximumWidth: bubble.maxBubble
                implicitWidth: content.implicitWidth + 24
                implicitHeight: content.implicitHeight + 16
                Kirigami.Theme.inherit: false

                radius: bubble.bubbleRadius
                // Tuck the corner nearest the avatar/edge for a subtle tail.
                corners {
                    topLeftRadius: (!bubble.mine && !bubble.grouped) ? 4 : bubble.bubbleRadius
                    topRightRadius: (bubble.mine && !bubble.grouped) ? 4 : bubble.bubbleRadius
                    bottomLeftRadius: bubble.bubbleRadius
                    bottomRightRadius: bubble.bubbleRadius
                }

                color: bubble.mine ? Th.bubbleOwnBg : Th.bubbleOtherBg

                shadow.size: 10
                shadow.color: bubble.mine ? Th.bubbleOwnShadow : Th.bubbleOtherShadow
                shadow.yOffset: 2

                border.width: bubble.failed ? 1 : (bubble.mine ? 0 : 1)
                border.color: bubble.failed ? Th.bubbleBorderError : Th.bubbleOtherBorder   // 10.1 (TH-MSG-22): palette red

                // 9.3: the hover action bar — Discord's "you never have to right-click".
                // Three quick reactions, the picker, reply, edit (mine), more. A child of
                // the bubble so hovering it keeps the row hovered; overlaps the top edge.
                Kirigami.ShadowedRectangle {
                    id: hoverBar
                    z: 30
                    shadow.size: 10; shadow.color: Th.bubbleHoverBarShadow; shadow.yOffset: 2
                    visible: opacity > 0
                    opacity: (bubbleHover.hovered || barHover.hovered) && !bubble.redacted && !bubble.undecryptable && !bubble.pending && !bubble.failed && bubble.eventId.length > 0 && bubble.eventId.charAt(0) !== "~" ? 1 : 0
                    Behavior on opacity { NumberAnimation { duration: Motion.durShort } }
                    anchors.bottom: bubble.barAbove ? parent.top : undefined
                    anchors.top: bubble.barAbove ? undefined : parent.top
                    anchors.bottomMargin: -12
                    anchors.topMargin: 4
                    // Grows INTO the free side of the row: rightwards from an incoming bubble's
                    // left edge, leftwards from my own bubble's right edge — so a two-word bubble
                    // never pushes the bar off the edge of the timeline.
                    anchors.left: bubble.mine ? undefined : parent.left
                    anchors.right: bubble.mine ? parent.right : undefined
                    anchors.leftMargin: bubble.mine ? 0 : 6
                    anchors.rightMargin: bubble.mine ? 6 : 0
                    width: barRow.implicitWidth + 8
                    height: 28
                    radius: 8
                    color: Th.bubbleHoverBarBg
                    border.width: 1
                    border.color: Th.bubbleHoverBarBorder
                    HoverHandler { id: barHover }
                    Row {
                        id: barRow
                        anchors.centerIn: parent
                        spacing: 2
                        Repeater {
                            model: ["👍", "❤️", "😂"]
                            delegate: Rectangle {
                                required property var modelData
                                // 10.1 (TH-MSG-10): the quick buttons TOGGLE like the chips do (a second
                                // click used to send a duplicate reaction the server rejects).
                                readonly property var chip: { var r = bubble.reactions || []; for (var i = 0; i < r.length; i++) if (r[i].key === modelData) return r[i]; return null }
                                readonly property bool mineAlready: chip !== null && chip.mine === true
                                width: 26; height: 24; radius: 6
                                color: mineAlready ? Th.bubbleHoverBarButtonBgChecked : (qrMa.containsMouse ? Th.bubbleHoverBarButtonBgHover : "transparent")
                                Controls.Label { anchors.centerIn: parent; text: parent.modelData; font.pixelSize: 15 }
                                MouseArea {
                                    id: qrMa; anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor
                                    onClicked: {
                                        if (parent.mineAlready && parent.chip.reactionId !== undefined && String(parent.chip.reactionId).length > 0) bubble.unreact(parent.chip.reactionId)
                                        else if (!parent.mineAlready) bubble.react(parent.modelData)
                                    }
                                }
                                Controls.ToolTip.text: (mineAlready ? "Take back your " : "React ") + modelData
                                Controls.ToolTip.visible: qrMa.containsMouse
                                Controls.ToolTip.delay: 500
                            }
                        }
                        Repeater {
                            model: [
                                { icon: "face-smile-symbolic", tip: "More reactions", act: "react" },
                                { icon: "mail-reply-sender-symbolic", tip: "Reply", act: "reply" },
                                { icon: "document-edit-symbolic", tip: "Edit", act: "edit", mineOnly: true, textOnly: true },
                                { icon: "overflow-menu-symbolic", tip: "More", act: "more" }
                            ]
                            delegate: Rectangle {
                                required property var modelData
                                // 10.1 (TH-MSG-01): the pencil never appears on a picture/file — an edit
                                // sends m.text and REPLACES the attachment with its filename, for everyone.
                                visible: (!(modelData.mineOnly === true) || bubble.mine) && !(modelData.textOnly === true && (bubble.isMedia || bubble.hasDiceCard || bubble.hasPollCard))
                                width: visible ? 26 : 0; height: 24; radius: 6
                                color: abMa.containsMouse ? Th.bubbleHoverBarButtonBgHover : "transparent"
                                Kirigami.Icon {
                                    anchors.centerIn: parent
                                    width: 16; height: 16
                                    source: parent.modelData.icon
                                    color: Th.bubbleHoverBarIcon
                                }
                                MouseArea {
                                    id: abMa
                                    anchors.fill: parent
                                    hoverEnabled: true
                                    cursorShape: Qt.PointingHandCursor
                                    onClicked: {
                                        var a = parent.modelData.act
                                        if (a === "react") bubble.moreReactRequested()
                                        else if (a === "reply") bubble.replyRequested()
                                        else if (a === "edit") bubble.editRequested()
                                        else bubble.menuRequested()
                                    }
                                }
                                Controls.ToolTip.text: modelData.tip
                                Controls.ToolTip.visible: abMa.containsMouse
                                Controls.ToolTip.delay: 500
                            }
                        }
                    }
                }

                // Right-click / long-press opens the message action menu.
                TapHandler {
                    acceptedButtons: Qt.RightButton
                    onTapped: bubble.menuRequested()
                }
                TapHandler {
                    acceptedButtons: Qt.LeftButton
                    onLongPressed: bubble.menuRequested()
                }

                // Own-message gradient overlay (ShadowedRectangle has no `gradient`):
                // fox-flame when furry, accent->darker otherwise. Clipped to the radius.
                Rectangle {
                    anchors.fill: parent
                    visible: bubble.mine
                    radius: bubble.bubbleRadius
                    gradient: Gradient {
                        orientation: Gradient.Vertical
                        GradientStop {
                            position: 0.0
                            color: Th.bubbleOwnTopBg
                        }
                        GradientStop {
                            position: 1.0
                            color: Th.bubbleOwnBottomBg
                        }
                    }
                }

                ColumnLayout {
                    id: content
                    anchors.left: parent.left
                    anchors.top: parent.top
                    anchors.leftMargin: 12
                    anchors.topMargin: 8
                    anchors.rightMargin: 12
                    spacing: 2

                    // Quoted reply parent (when this message replies to another).
                    Rectangle {
                        // 10.1 (TH-TL-05): a parent that is not loaded still gets a quote (placeholder + jump).
                        visible: (bubble.replyToSender.length > 0 || bubble.replyToId.length > 0) && !bubble.redacted
                        Layout.maximumWidth: bubble.maxBubble - 24
                        radius: 6
                        color: bubble.mine ? (quoteHover.hovered ? Th.bubbleOwnQuoteBgHover : Th.bubbleOwnQuoteBg)
                                           : (quoteHover.hovered ? Th.bubbleOtherQuoteBgHover : Th.bubbleOtherQuoteBg)
                        // 9.3: click the quote to jump to the message it answers.
                        HoverHandler { id: quoteHover; cursorShape: Qt.PointingHandCursor }
                        TapHandler { acceptedButtons: Qt.LeftButton; onTapped: if (bubble.replyToId.length > 0) bubble.quoteClicked(bubble.replyToId) }
                        implicitWidth: replyCol.implicitWidth + 16
                        implicitHeight: replyCol.implicitHeight + 8
                        Rectangle {
                            anchors.left: parent.left; anchors.top: parent.top; anchors.bottom: parent.bottom
                            width: 3; radius: 1.5
                            color: bubble.mine ? Th.bubbleOwnQuoteStripe : Th.bubbleOtherQuoteStripe
                        }
                        ColumnLayout {
                            id: replyCol
                            anchors.left: parent.left
                            anchors.leftMargin: 10
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 0
                            Controls.Label {
                                text: bubble.replyToSender.length > 0 ? bubble.replyToSender : "Replying to an earlier message"
                                color: bubble.mine ? Th.bubbleOwnQuoteFg : (bubble.replyToSender.length > 0 ? bubble.theme.senderColor(bubble.replyToSender) : Th.bubbleOtherQuoteFg)
                                font.bold: true
                                font.pixelSize: Math.round(Kirigami.Theme.smallFont.pixelSize * 0.95)
                            }
                            Controls.Label {
                                Layout.maximumWidth: bubble.maxBubble - 44
                                text: bubble.replyToBody.length > 0 ? bubble.replyToBody : (bubble.replyToSender.length > 0 ? "" : "Older than what's loaded — click to look for it")
                                color: bubble.mine ? Th.bubbleOwnQuoteFg : Th.bubbleOtherQuoteFg
                                elide: Text.ElideRight
                                maximumLineCount: 1
                                opacity: 0.85
                                font.pixelSize: Math.round(Kirigami.Theme.smallFont.pixelSize * 0.95)
                            }
                        }
                    }

                    // Redacted placeholder (replaces all content for deleted messages).
                    Controls.Label {
                        visible: bubble.redacted
                        text: "Message deleted"
                        color: bubble.mine ? Th.bubbleOwnMetaFg : Th.bubbleOtherMetaFg
                        font.italic: true
                        opacity: 0.8
                    }
                    // 10.1 (TH-MSG-12): a locked row reads as a marker, not as something the sender typed.
                    RowLayout {
                        visible: bubble.undecryptable
                        spacing: 6
                        ThIcon {
                            app: bubble.app
                            name: "lock-symbolic"
                            Layout.preferredWidth: 14; Layout.preferredHeight: 14
                            Layout.alignment: Qt.AlignTop
                            color: bubble.mine ? Th.bubbleOwnMetaFg : Th.bubbleOtherMetaFg
                            isMask: true
                        }
                        Controls.Label {
                            Layout.maximumWidth: bubble.maxBubble - 70
                            // selfhost/firstrun §3: why it can't be read, in the web's words (the row's body, utd.rs)
                            text: bubble.body.length > 0 ? bubble.body : "Can't read this message yet — its key hasn't reached this device."
                            color: bubble.mine ? Th.bubbleOwnMetaFg : Th.bubbleOtherMetaFg
                            font.italic: true
                            opacity: 0.8
                            wrapMode: Text.WordWrap
                        }
                    }

                    // TextEdit (read-only), NOT a Label: mouse selection is the whole
                    // point ("copy specific portions of a message" — friends round 2).
                    // Costs vs Label: no lineHeight/linkColor properties (links render
                    // the palette default; spacing is fractionally tighter) — accepted.
                    TextEdit {
                        id: bodyText
                        Layout.maximumWidth: bubble.maxBubble - 24
                        // 10.1 (TH-MSG-07): links in the palette's colour, not the style's default blue.
                        palette.link: bubble.mine ? Th.bubbleOwnLinkFg : Th.bubbleOtherLinkFg
                        palette.linkVisited: bubble.mine ? Th.bubbleOwnLinkFg : Th.bubbleOtherLinkFg
                        // Hide the raw text for embed-only links and for media bubbles
                        // (those render as an image / chip below).
                        visible: text.length > 0 && !bubble.isMedia && !bubble.redacted && !bubble.undecryptable
                                 && !(bubble.onlyEmbed && bubble.embeds.length > 0)
                                 && !(calCard.visible && calCard.live !== null) // 0.8.33: the live card replaces the summary
                                 && !bubble.hasThemeCard   // 11.0: a theme share card replaces its body line
                                 && !bubble.hasDiceCard    // widgets W3: a dice card replaces its body line
                                 && !bubble.hasPollCard    // widgets W3: a poll card replaces its body line
                        // Rich text when a formatted_body (markdown/HTML) is present. 9.3: an
                        // emote reads "∗ Name does a thing" in italics; emoji-only lines go big.
                        text: bubble.isEmote ? ("∗ " + bubble.displayName + " " + (bubble.formatted.length > 0 ? bubble._codeFont(bubble.formatted) : bubble.body))
                            : (bubble.formatted.length > 0 ? bubble._codeFont(bubble.formatted) : bubble.body)
                        textFormat: bubble.formatted.length > 0 ? TextEdit.RichText : TextEdit.PlainText
                        font.italic: bubble.isEmote
                        // ENGINE.md §10: × the theme's text size
                        font.pointSize: (bubble.jumbo ? 2.6 : 1) * Kirigami.Theme.defaultFont.pointSize * Th.textScale
                        // ENGINE.md §10: the theme's chat font (type.chat, else type.ui) — only while a theme names one
                        // (without it the text keeps the app's own font, untouched)
                        Binding { target: bodyText; property: "font.family"; value: Th.fontChat; when: Th.fontChat.length > 0; restoreMode: Binding.RestoreBindingOrValue }
                        wrapMode: TextEdit.Wrap
                        color: bubble.mine ? Th.bubbleOwnFg : Th.bubbleOtherFg
                        readOnly: true
                        selectByMouse: true
                        selectionColor: Th.bubbleSelectionBg
                        selectedTextColor: Th.bubbleSelectionFg
                        // A matrix.to user link is a @mention "pill" → open the profile
                        // flyout (not a browser); any other link opens externally.
                        onLinkActivated: (link) => {
                            var who = bubble.mentionTarget(link)
                            if (who.length > 0) bubble.avatarClicked(who)
                            else Qt.openUrlExternally(link)
                        }
                        TapHandler {
                            acceptedButtons: Qt.RightButton
                            onTapped: bodyMenu.popup()
                        }
                        Controls.Menu {
                            id: bodyMenu
                            Controls.MenuItem {
                                text: "Copy selection"
                                enabled: bodyText.selectedText.length > 0
                                onTriggered: bodyText.copy()
                            }
                            Controls.MenuItem {
                                text: "Copy message"
                                onTriggered: {
                                    bodyText.selectAll()
                                    bodyText.copy()
                                    bodyText.deselect()
                                }
                            }
                        }
                    }

                    // 0.8.33: calendar invite card — the event as it is NOW (title, time in MY zone,
                    // who is going), Going / Maybe / Can't in place, click → the calendar window.
                    // The markdown summary stays in the message for stock clients and hides here
                    // while the live card shows.
                    CalendarCard {
                        id: calCard
                        visible: bubble.calEvent.length > 0 && !bubble.redacted
                        app: bubble.app
                        theme: bubble.theme
                        container: bubble.calContainer
                        eventId: bubble.calEvent
                        mine: bubble.mine
                        maxW: bubble.maxBubble - 24
                        Layout.preferredWidth: implicitWidth
                        Layout.preferredHeight: implicitHeight
                        Layout.topMargin: 2
                        onOpenRequested: bubble.calendarClicked(bubble.calContainer, bubble.calEvent)
                        onAnswered: (status) => bubble.calendarRsvp(bubble.calContainer, bubble.calEvent, status)
                    }

                    // 11.0 step 5 (THEMES-UI.md §14.3): a theme shared in the room — Try (30 s) / Keep, in the theme's own colours
                    Loader {
                        active: bubble.hasThemeCard
                        visible: active
                        Layout.preferredWidth: item ? item.implicitWidth : 0
                        Layout.preferredHeight: item ? item.implicitHeight : 0
                        Layout.topMargin: 4
                        Layout.bottomMargin: 4
                        sourceComponent: ThemeShareCard {
                            app: bubble.app
                            row: bubble.themeShare
                            eventId: bubble.eventId
                            mine: bubble.mine
                            maxWidth: bubble.maxBubble - 24
                            width: implicitWidth
                            height: implicitHeight
                        }
                    }

                    // widgets W3 (WIDGETS-UI.md §17.4): a poll — the card in place of the body line
                    Loader {
                        active: bubble.hasPollCard
                        visible: active
                        Layout.preferredWidth: item ? item.implicitWidth : 0
                        Layout.preferredHeight: item ? item.implicitHeight : 0
                        Layout.topMargin: 2
                        Layout.bottomMargin: 2
                        sourceComponent: PollCard {
                            app: bubble.app
                            c: bubble.poll
                            canVote: true
                            maxW: bubble.maxBubble - 24
                            width: implicitWidth
                            height: implicitHeight
                            onVoteRequested: (a) => bubble.pollVote(a)
                            onEndRequested: bubble.pollEnd()
                        }
                    }

                    // widgets W3 (WIDGETS-UI.md §16.4): a dice roll — the card in place of the body line
                    Loader {
                        active: bubble.hasDiceCard
                        visible: active
                        Layout.preferredWidth: item ? item.implicitWidth : 0
                        Layout.preferredHeight: item ? item.implicitHeight : 0
                        Layout.topMargin: 2
                        Layout.bottomMargin: 2
                        sourceComponent: DiceCard {
                            app: bubble.app
                            r: bubble.dice
                            variant: "chat"
                            maxW: bubble.maxBubble - 24
                            width: implicitWidth
                            height: implicitHeight
                        }
                    }

                    // Inline image (m.image), decrypted to a file:// by the backend. The display
                    // box is RESERVED from the event-provided w/h (capped) BEFORE the image
                    // decodes, so the row height is stable and loading an image while scrolling up
                    // through history doesn't shove the viewport. Click opens the zoomable lightbox.
                    Image {
                        id: inlineImg
                        // Static images only — GIFs go to the AnimatedImage below, because
                        // Image renders just their first frame.
                        visible: bubble.msgtype === "m.image" && bubble.media.length > 0
                                 && !bubble.redacted && !bubble.isGif
                        Layout.topMargin: 2
                        readonly property real capW: bubble.maxBubble - 24
                        readonly property real capH: 360
                        // Natural size: event w/h when known (stable, no decode needed), else the
                        // decoded implicit size (older events without dimensions — best effort).
                        readonly property real natW: bubble.imageW > 0 ? bubble.imageW : implicitWidth
                        readonly property real natH: bubble.imageH > 0 ? bubble.imageH : implicitHeight
                        readonly property real fit: (natW > 0 && natH > 0)
                            ? Math.min(1.0, capW / natW, capH / natH) : 0
                        Layout.preferredWidth: natW > 0 ? Math.round(natW * fit) : capW
                        // 10.1 (TH-MSG-26): no size known → reserve a 16:9 box, not 0 px (the row used
                        // to grow from nothing when the decode landed and shove the viewport).
                        Layout.preferredHeight: natH > 0 ? Math.round(natH * fit) : Math.round(capW * 9 / 16)
                        fillMode: Image.PreserveAspectFit
                        source: visible ? bubble.media : ""
                        asynchronous: true
                        cache: true
                        // Honour the EXIF orientation tag. Without this a photo taken in
                        // portrait on a phone displays on its side, which reads as "the app
                        // rotated my picture".
                        autoTransform: true
                        // 0.8.30: rounded corners through a real mask (a clip only clips the
                        // rectangle). Off under the software renderer, where a ShaderEffect draws
                        // nothing at all — safe mode keeps square photos rather than no photos.
                        layer.enabled: GraphicsInfo.api !== GraphicsInfo.Software && status === Image.Ready
                        layer.effect: OpacityMask {
                            maskSource: Rectangle { width: inlineImg.width; height: inlineImg.height; radius: 8 }
                        }
                        // Subtle reserved-box fill while the image downloads/decrypts/decodes.
                        // 10.1 (TH-MSG-26): loading and failed are two different things now.
                        Rectangle {
                            anchors.fill: parent
                            visible: inlineImg.status !== Image.Ready
                            radius: 6
                            color: Th.bubbleImageLoadingBg
                            ThBusyIndicator {   // 11.0 (GPU.md §2.5): still
                                anchors.centerIn: parent
                                running: inlineImg.status === Image.Loading && parent.visible
                                visible: running
                                size: 28
                            }
                            RowLayout {
                                anchors.centerIn: parent
                                width: Math.min(parent.width - 16, implicitWidth)
                                visible: inlineImg.status === Image.Error
                                spacing: 6
                                ThIcon {
                                    Layout.preferredWidth: 16; Layout.preferredHeight: 16
                                    app: bubble.app
                                    name: "image-missing"
                                    color: bubble.mine ? Th.bubbleOwnMetaFg : Th.bubbleOtherMetaFg
                                    isMask: true
                                }
                                Controls.Label {
                                    Layout.fillWidth: true
                                    text: "Couldn't load this image — click to try again"
                                    elide: Text.ElideRight
                                    color: bubble.mine ? Th.bubbleOwnMetaFg : Th.bubbleOtherMetaFg
                                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                                }
                            }
                            MouseArea {
                                anchors.fill: parent
                                enabled: inlineImg.status === Image.Error
                                cursorShape: enabled ? Qt.PointingHandCursor : Qt.ArrowCursor
                                onClicked: { var s = inlineImg.source; inlineImg.source = ""; inlineImg.source = s }
                            }
                        }
                        // Left-click → lightbox; right-click falls through to the bubble menu.
                        MouseArea {
                            anchors.fill: parent
                            acceptedButtons: Qt.LeftButton
                            cursorShape: Qt.PointingHandCursor
                            onClicked: bubble.imageClicked(bubble.media, inlineImg.natW, inlineImg.natH, bubble.body)
                        }
                    }
                    // Image still decrypting / unavailable → a labeled placeholder.
                    RowLayout {   // 0.8.30: pack glyph (was a typeset 🖼)
                        visible: bubble.msgtype === "m.image" && bubble.media.length === 0 && !bubble.redacted
                        spacing: 6
                        ThIcon {
                            Layout.preferredWidth: 16; Layout.preferredHeight: 16
                            app: bubble.app
                            name: "image-x-generic"
                            color: bubble.mine ? Th.bubbleOwnMetaFg : Th.bubbleOtherMetaFg
                            isMask: true
                        }
                        Controls.Label {
                            text: bubble.body.length > 0 ? bubble.body : "image"
                            color: bubble.mine ? Th.bubbleOwnMetaFg : Th.bubbleOtherMetaFg
                        }
                    }
                    // Non-image media (file / video / audio) → a tappable chip. The tap
                    // was MISSING until 0.3.4 ("no clear way to view an mp4" — a field report):
                    // click downloads+decrypts to the media cache and opens it.
                    RowLayout {   // 0.8.30: pack glyph (was a typeset 🎵)
                        visible: !bubble.redacted && bubble.msgtype === "m.audio" && bubble.media.length === 0
                        spacing: 6
                        ThIcon {
                            Layout.preferredWidth: 16; Layout.preferredHeight: 16
                            app: bubble.app
                            name: "audio-x-generic"
                            color: bubble.mine ? Th.bubbleOwnLinkFg : Th.bubbleOtherLinkFg
                            isMask: true
                        }
                        Controls.Label {
                            text: (bubble.body.length > 0 ? bubble.body : "file") + (bubble.opening ? "  — opening…" : "")   // 10.1 (TH-MSG-13)
                            color: bubble.mine ? Th.bubbleOwnLinkFg : Th.bubbleOtherLinkFg
                            font.underline: true
                            Controls.ToolTip.text: bubble.opening ? "Opening…" : "Download & open"
                            Controls.ToolTip.visible: chipTap.containsMouse
                            Controls.ToolTip.delay: 400
                            MouseArea {
                                id: chipTap
                                anchors.fill: parent
                                hoverEnabled: true
                                cursorShape: Qt.PointingHandCursor
                                onClicked: bubble.mediaClicked(bubble.eventId)
                            }
                        }
                    }

                    // Animated GIFs (QOL 1.6.4). Same reserved-box sizing as the static
                    // image so history does not jump while it decodes.
                    // 11.0 (GPU.md §2.6): ThGif — plays ≤ 3 loops or 15 s while focused + in view, then rests on
                    // its first frame (a click opens the lightbox, a hover replays).
                    ThGif {
                        id: inlineGif
                        visible: bubble.msgtype === "m.image" && bubble.media.length > 0
                                 && !bubble.redacted && bubble.isGif
                        Layout.topMargin: 2
                        readonly property real capW: bubble.maxBubble - 24
                        readonly property real capH: 360
                        readonly property real natW: bubble.imageW > 0 ? bubble.imageW : implicitWidth
                        readonly property real natH: bubble.imageH > 0 ? bubble.imageH : implicitHeight
                        readonly property real fit: (natW > 0 && natH > 0)
                            ? Math.min(1.0, capW / natW, capH / natH) : 0
                        Layout.preferredWidth: natW > 0 ? Math.round(natW * fit) : capW
                        // 10.1 (TH-MSG-26): no size known → reserve a 16:9 box, not 0 px (the row used
                        // to grow from nothing when the decode landed and shove the viewport).
                        Layout.preferredHeight: natH > 0 ? Math.round(natH * fit) : Math.round(capW * 9 / 16)
                        fillMode: Image.PreserveAspectFit
                        source: visible ? bubble.media : ""
                        asynchronous: true
                        inView: bubble.inView   // (cache: ThGif's own rule — frames not kept for a local file)
                        clickPlays: false
                        // Rounded corners: four static bites in the bubble's own colour at this height (my bubble's
                        // gradient included). 11.0 (GPU.md §2.6): the OpacityMask layer re-rendered on every GIF frame.
                        CornerMask {
                            radius: 8
                            visible: inlineGif.status === Image.Ready
                            readonly property real t0: bubbleRect.height > 0 ? (content.y + inlineGif.y) / bubbleRect.height : 0
                            readonly property real t1: bubbleRect.height > 0 ? (content.y + inlineGif.y + inlineGif.height) / bubbleRect.height : 1
                            color: bubble.mine ? bubble.ownFillAt(t0) : Th.bubbleOtherBg
                            topColor: bubble.mine ? bubble.ownFillAt(t0) : Th.bubbleOtherBg
                            bottomColor: bubble.mine ? bubble.ownFillAt(t1) : Th.bubbleOtherBg
                        }
                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: bubble.imageClicked(bubble.media, bubble.imageW,
                                                           bubble.imageH, bubble.body)
                        }
                    }

                    // Video: poster card + ▶, and on click it plays IN PLACE (QOL 1.6.2).
                    // Same lazy-surface discipline as ThEmbedCard — no MediaPlayer and
                    // no download until the user actually asks for it, because videos are
                    // the one attachment big enough that eagerly fetching them would be
                    // rude.
                    Rectangle {
                        visible: !bubble.redacted && bubble.msgtype === "m.video"
                        Layout.fillWidth: true
                        // 10.1 (TH-MSG-06): a width of its own — a ColumnLayout gives a bare Rectangle
                        // nothing, so the card collapsed to the width of the clock. (TH-MSG-17): the
                        // player box follows the clip's real aspect ratio when the event carries one.
                        Layout.preferredWidth: bubble.maxBubble - 24
                        Layout.minimumWidth: 220
                        Layout.preferredHeight: bubble.videoUrl.length > 0
                            ? ((bubble.imageW > 0 && bubble.imageH > 0) ? Math.max(120, Math.min(360, Math.round((bubble.maxBubble - 24) * bubble.imageH / bubble.imageW))) : 220)
                            : 96
                        radius: 8
                        color: Th.bubbleVideoBg
                        border.width: 1
                        border.color: Th.bubbleVideoBorder
                        clip: true

                        // Poster state: what it is, how big, and an obvious ▶.
                        RowLayout {
                            anchors.fill: parent
                            anchors.margins: 10
                            spacing: 10
                            visible: bubble.videoUrl.length === 0
                            Kirigami.Icon {
                                source: "video-x-generic"
                                Layout.preferredWidth: 32
                                Layout.preferredHeight: 32
                            }
                            ColumnLayout {
                                spacing: 1
                                Layout.fillWidth: true
                                Controls.Label {
                                    text: bubble.body.length > 0 ? bubble.body : "video"
                                    elide: Text.ElideMiddle
                                    Layout.fillWidth: true
                                    color: Th.bubbleVideoFg
                                }
                                Controls.Label {
                                    text: bubble.videoLoading
                                          ? "Downloading…"
                                          : (bubble.fmtSize(bubble.fileSize)
                                             + (bubble.durationMs > 0
                                                ? "  ·  " + bubble.fmtTime(bubble.durationMs) : ""))
                                    color: Th.bubbleVideoMetaFg
                                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                                }
                            }
                            Controls.ToolButton {
                                icon.name: "media-playback-start"
                                enabled: !bubble.videoLoading
                                Controls.ToolTip.text: "Play here"
                                Controls.ToolTip.visible: hovered
                                onClicked: bubble.videoPlayRequested(bubble.eventId)
                            }
                            Controls.ToolButton {
                                icon.name: "document-save-symbolic"
                                Controls.ToolTip.text: "Save as…"
                                Controls.ToolTip.visible: hovered
                                onClicked: bubble.saveRequested(bubble.eventId,
                                                                bubble.body.length > 0 ? bubble.body : "video")
                            }
                        }

                        // Playing state: the surface only exists once there is a file.
                        Loader {
                            anchors.fill: parent
                            active: bubble.videoUrl.length > 0
                            sourceComponent: videoSurface
                        }
                    }

                    // File card (QOL 1.6.3). A bare underlined filename that silently
                    // shelled out told you nothing: not the type, not the size, and gave
                    // no way to keep the file. Now it says what it is and offers both.
                    Rectangle {
                        visible: !bubble.redacted && bubble.msgtype === "m.file"
                        Layout.fillWidth: true
                        Layout.preferredWidth: bubble.maxBubble - 24   // 10.1 (TH-MSG-06)
                        Layout.minimumWidth: 220
                        Layout.preferredHeight: fileRow.implicitHeight + 16
                        radius: 8
                        color: bubble.mine ? Th.bubbleOwnFileBg : Th.bubbleOtherFileBg
                        border.width: 1
                        border.color: Th.bubbleFileBorder

                        RowLayout {
                            id: fileRow
                            anchors.fill: parent
                            anchors.margins: 8
                            spacing: 10

                            Kirigami.Icon {
                                source: bubble.fileIconName(bubble.body, bubble.mimetype)
                                Layout.preferredWidth: 32
                                Layout.preferredHeight: 32
                            }
                            ColumnLayout {
                                spacing: 1
                                Layout.fillWidth: true
                                Controls.Label {
                                    text: bubble.body.length > 0 ? bubble.body : "file"
                                    elide: Text.ElideMiddle
                                    Layout.fillWidth: true
                                    color: bubble.mine ? Th.bubbleOwnFg : Th.bubbleOtherFg
                                }
                                Controls.Label {
                                    text: bubble.fmtSize(bubble.fileSize)
                                    visible: bubble.fileSize > 0
                                    color: bubble.mine ? Th.bubbleOwnMetaFg : Th.bubbleOtherMetaFg
                                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                                }
                            }
                            // 10.1 (TH-MSG-13): a fetch in flight shows here; the toast says where a save went.
                            ThBusyIndicator {   // 11.0 (GPU.md §2.5): still
                                visible: bubble.opening
                                running: visible
                                size: 18
                            }
                            Controls.ToolButton {
                                icon.name: "document-open-symbolic"
                                enabled: !bubble.opening
                                Controls.ToolTip.text: bubble.opening ? "Opening…" : "Open"
                                Controls.ToolTip.visible: hovered
                                onClicked: bubble.mediaClicked(bubble.eventId)
                            }
                            Controls.ToolButton {
                                icon.name: "document-save-symbolic"
                                Controls.ToolTip.text: "Save as…"
                                Controls.ToolTip.visible: hovered
                                onClicked: bubble.saveRequested(bubble.eventId,
                                                                bubble.body.length > 0 ? bubble.body : "file")
                            }
                        }
                    }

                    // Inline audio player (QOL 1.6.1). Audio used to hand the file to the
                    // system music player, which threw you out of the conversation to hear
                    // a two-second clip. Local playback only — publishing into a call is a
                    // different path entirely (play_url_in_call), so this cannot leak into
                    // a room.
                    ColumnLayout {
                        visible: !bubble.redacted && bubble.msgtype === "m.audio" && bubble.media.length > 0
                        spacing: 2
                        Layout.fillWidth: true
                        Layout.preferredWidth: bubble.maxBubble - 24   // 10.1 (TH-MSG-06)
                        Layout.minimumWidth: 220

                        MediaPlayer {
                            id: audioPlayer
                            source: bubble.media
                            audioOutput: AudioOutput { id: audioOut }
                        }

                        RowLayout {
                            spacing: 8
                            Layout.fillWidth: true
                            Controls.ToolButton {
                                icon.name: audioPlayer.playbackState === MediaPlayer.PlayingState
                                           ? "media-playback-pause" : "media-playback-start"
                                onClicked: audioPlayer.playbackState === MediaPlayer.PlayingState
                                           ? audioPlayer.pause() : audioPlayer.play()
                            }
                            ThSlider {
                                id: audioSeek
                                Layout.fillWidth: true
                                from: 0
                                // The event's own duration is only a hint; once the file is
                                // loaded the player knows the truth, so prefer that.
                                to: audioPlayer.duration > 0 ? audioPlayer.duration
                                                             : Math.max(1, bubble.durationMs)
                                value: audioPlayer.position
                                onMoved: audioPlayer.position = value
                            }
                            Controls.Label {
                                text: bubble.fmtTime(audioPlayer.position) + " / "
                                      + bubble.fmtTime(audioPlayer.duration > 0 ? audioPlayer.duration
                                                                                : bubble.durationMs)
                                color: bubble.mine ? Th.bubbleOwnMetaFg : Th.bubbleOtherMetaFg
                                font.pointSize: Kirigami.Theme.smallFont.pointSize
                            }
                        }
                        Controls.Label {
                            text: bubble.body.length > 0 ? bubble.body : "audio"
                            color: bubble.mine ? Th.bubbleOwnMetaFg : Th.bubbleOtherMetaFg
                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                            elide: Text.ElideRight
                            Layout.fillWidth: true
                        }
                    }

                    // Inline media embeds: GIFs/images render inline; video/iframe links
                    // become click-to-PLAY cards (ThEmbedCard) that spin up a
                    // MediaPlayer / WebEngineView in place on click (nothing third-party
                    // loads until then), with ↗ open-externally + ✕ collapse-back.
                    Repeater {
                        model: bubble.embeds
                        delegate: ThEmbedCard {
                            required property var modelData
                            app: bubble.app
                            inView: bubble.inView
                            roomId: (bubble.room && bubble.room.roomId) ? String(bubble.room.roomId) : ""   // Media Party 11.4
                            embed: modelData
                            maxW: bubble.maxBubble - 24
                            Layout.topMargin: 4
                            Layout.preferredWidth: implicitWidth
                            Layout.preferredHeight: implicitHeight
                        }
                    }

                    // 9.3: a card for an ordinary link (article, store page, repo…).
                    LinkPreviewCard {
                        visible: bubble.preview !== null && bubble.preview !== undefined && !bubble.redacted
                        app: bubble.app
                        preview: bubble.preview
                        maxW: bubble.maxBubble - 24
                        onAccent: bubble.mine
                        Layout.topMargin: 4
                        Layout.preferredWidth: implicitWidth
                        Layout.preferredHeight: visible ? implicitHeight : 0
                    }

                    // Timestamp row, right-aligned within the bubble. 9.3: delivery state
                    // (sending… / not sent), the pin, and the full date on hover.
                    Controls.Label {
                        id: stampLabel
                        Layout.fillWidth: true
                        text: bubble.failed ? "Not sent"
                            : (bubble.pending ? "Sending…"
                            : ((bubble.pinned ? "📌 " : "") + (bubble.edited ? "(edited) " : "") + (bubble.theme.fmtClock ? bubble.theme.fmtClock(bubble.ts) : Qt.formatTime(new Date(bubble.ts), Locale.ShortFormat))))   // 10.1 (TL-19 / TH-PREF-11): the app's clock setting, else the system's
                        horizontalAlignment: Text.AlignRight
                        font.pixelSize: Math.round(Kirigami.Theme.smallFont.pixelSize * 0.92)
                        opacity: bubble.failed ? 1.0 : 0.7
                        color: bubble.failed ? Th.bubbleFgError : (bubble.mine ? Th.bubbleOwnMetaFg : Th.bubbleOtherMetaFg)
                        HoverHandler { id: stampHover }
                        Controls.ToolTip.text: Qt.formatDateTime(new Date(bubble.ts), Locale.LongFormat)
                        Controls.ToolTip.visible: stampHover.hovered && !bubble.pending && !bubble.failed
                        Controls.ToolTip.delay: 600
                    }
                }
            }

            // 9.3: a message the server refused / never got. Retry keeps the same row.
            RowLayout {
                Layout.alignment: bubble.mine ? Qt.AlignRight : Qt.AlignLeft
                visible: bubble.failed
                spacing: 8
                Controls.Label {
                    text: bubble.sendError.length > 0 ? bubble.sendError : "Couldn't send"
                    color: Th.bubbleFgError
                    font.pixelSize: Math.round(Kirigami.Theme.smallFont.pixelSize)
                }
                Controls.Label {
                    text: "Retry"
                    color: Th.bubbleRetryFg
                    font.bold: true
                    font.underline: retryHover.hovered
                    font.pixelSize: Math.round(Kirigami.Theme.smallFont.pixelSize)
                    HoverHandler { id: retryHover; cursorShape: Qt.PointingHandCursor }
                    TapHandler { onTapped: bubble.retryRequested(bubble.txn) }
                }
                Controls.Label {
                    text: "Delete"
                    color: Th.bubbleDiscardFg
                    font.underline: discardHover.hovered
                    font.pixelSize: Math.round(Kirigami.Theme.smallFont.pixelSize)
                    HoverHandler { id: discardHover; cursorShape: Qt.PointingHandCursor }
                    TapHandler { onTapped: bubble.discardRequested(bubble.txn) }
                }
            }

            // Reactions row slot — renders chips when `reactions` is populated.
            // Each reaction is { key, count, mine, reactionId, who }.
            Flow {
                Layout.fillWidth: true
                Layout.maximumWidth: bubble.maxBubble   // 10.1 (TH-VIS-15): wraps within the bubble's width
                Layout.alignment: bubble.mine ? Qt.AlignRight : Qt.AlignLeft
                // 9.3: a Flow fills from its leading edge — under my own (right-aligned) bubble
                // the chips used to land at the far left of the row.
                layoutDirection: bubble.mine ? Qt.RightToLeft : Qt.LeftToRight
                visible: bubble.reactions && bubble.reactions.length > 0
                spacing: 4
                Repeater {
                    model: bubble.reactions
                    delegate: Rectangle {
                        required property var modelData
                        radius: height / 2
                        implicitHeight: 22
                        implicitWidth: reactRow.implicitWidth + 12
                        color: modelData.mine
                            ? (chipMa.containsMouse ? Th.bubbleReactionOwnBgHover : Th.bubbleReactionOwnBg)
                            : (chipMa.containsMouse ? Th.bubbleReactionBgHover : Th.bubbleReactionBg)
                        border.width: 1
                        border.color: modelData.mine ? Th.bubbleReactionOwnBorder : Th.bubbleReactionBorder
                        scale: chipMa.pressed ? 0.94 : 1.0
                        Behavior on color { ColorAnimation { duration: bubble.theme.dur; easing.type: Easing.OutCubic } }
                        Behavior on scale { NumberAnimation { duration: bubble.theme.dur; easing.type: Easing.OutBack } }
                        // Click toggles our reaction (add the same key, or redact ours).
                        MouseArea {
                            id: chipMa
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: {
                                if (modelData.mine && modelData.reactionId !== undefined && modelData.reactionId.length > 0)
                                    bubble.unreact(modelData.reactionId)
                                else
                                    bubble.react(modelData.key)
                            }
                        }
                        // 9.3: who reacted, on hover.
                        Controls.ToolTip.text: {
                            var who = (modelData.who !== undefined && modelData.who !== null) ? modelData.who : []
                            var names = []
                            for (var i = 0; i < who.length && i < 8; i++) names.push(who[i])
                            if (who.length > 8) names.push("+" + (who.length - 8))
                            var k = modelData.key !== undefined ? modelData.key : ""
                            if (names.length === 0) return (modelData.mine ? "You" : "Someone") + " reacted with " + k
                            return names.join(", ") + " reacted with " + k
                        }
                        Controls.ToolTip.visible: chipMa.containsMouse
                        Controls.ToolTip.delay: 450
                        RowLayout {
                            id: reactRow
                            anchors.centerIn: parent
                            spacing: 3
                            Controls.Label {
                                text: modelData.key !== undefined ? modelData.key : ""
                                font.pixelSize: Math.round(Kirigami.Theme.smallFont.pixelSize)
                            }
                            Controls.Label {
                                visible: modelData.count !== undefined && modelData.count > 1
                                text: modelData.count
                                color: Th.bubbleReactionCountFg
                                font.pixelSize: Math.round(Kirigami.Theme.smallFont.pixelSize)
                            }
                        }
                    }
                }
                // 10.1 (TH-VIS-15): the inline "+" is gone — "More reactions" lives in the hover bar,
                // and the half-faded smiley floating beside the chips read as a stray glyph.
            }

            // Read receipts — small overlapping avatars of members who've read up to
            // this message (their read marker rests here). Shows up to 5 + a "+N".
            RowLayout {
                Layout.alignment: bubble.mine ? Qt.AlignRight : Qt.AlignLeft
                Layout.topMargin: 1
                visible: bubble.receipts && bubble.receipts.length > 0
                spacing: 3
                // 9.3: "Seen by …" on hover.
                HoverHandler { id: seenHover }
                Controls.ToolTip.text: {
                    var r = bubble.receipts || []
                    var names = []
                    for (var i = 0; i < r.length && i < 10; i++) names.push(r[i].name !== undefined ? r[i].name : "")
                    if (r.length > 10) names.push("+" + (r.length - 10))
                    return "Seen by " + names.join(", ")
                }
                Controls.ToolTip.visible: seenHover.hovered
                Controls.ToolTip.delay: 450
                Row {
                    spacing: -6 // overlap the avatars
                    Repeater {
                        model: bubble.receipts.slice(0, 5)
                        delegate: Avatar {
                            required property var modelData
                            app: bubble.app
                            size: 16
                            name: modelData.name !== undefined ? modelData.name : ""
                            seed: modelData.userId !== undefined ? modelData.userId : (modelData.name !== undefined ? modelData.name : "")
                            imageSource: modelData.avatar !== undefined ? String(modelData.avatar) : ""   // 10.1 (TH-MSG-20): real faces
                        }
                    }
                }
                Controls.Label {
                    visible: bubble.receipts.length > 5
                    text: "+" + (bubble.receipts.length - 5)
                    color: Th.bubbleReceiptsFg
                    font.pixelSize: Math.round(Kirigami.Theme.smallFont.pixelSize * 0.85)
                }
            }
        }

        // Spacer pushes the bubble column to the correct edge and lets it shrink.
        Item { Layout.fillWidth: true }
    }
}
