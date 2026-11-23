// GameCallStrip.qml — games round 2, B4 (GAMES-V1.1-ADDENDUM §1.1/§1.2): the call's people who aren't seated at the open
// table ("watchers"), as a strip above the felt while the call's stage is the table. Each tile = the avatar with the
// call's speaking ring (Th.tileRingSpeaking, the tiles' ring), the muted glyph, the prestige token + the name; a sharer's
// tile carries "sharing" + Watch (= switch the stage to Call). Seats come from the view only; this strip is the rest.
// Narrow stage (< SEAT_VIDEO_MIN_STAGE_W): avatars only. Tin Hat Web draws the same (callui strip).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app

Item {
    id: cs
    property var app: null
    property var view: null
    property string myId: ""
    property string spaceId: ""
    property var words: ({})
    property real fBody: 10
    readonly property bool compact: cs.width < 1100   // SEAT_VIDEO_MIN_STAGE_W
    readonly property var r2: cs.words && cs.words.table && cs.words.table.r2 ? cs.words.table.r2 : ({})

    // the call's tiles minus shares, minus the seated (the self tile = me); a sharer is marked on their own tile
    readonly property var people: {
        var tiles = cs.app && cs.app.callTiles ? cs.app.callTiles : []
        var seated = ({})
        var seats = cs.view && cs.view.seats ? cs.view.seats : (cs.view && cs.view.players ? cs.view.players : [])
        for (var i = 0; i < seats.length; ++i) seated[String(seats[i].u)] = true
        var sharing = ({})
        for (var j = 0; j < tiles.length; ++j) if (tiles[j].kind === "screenshare" || tiles[j].screenshare === true) sharing[String(tiles[j].participant)] = true
        var out = [], seen = ({})
        for (var k = 0; k < tiles.length; ++k) {
            var t = tiles[k]
            if (t.kind === "screenshare" || t.screenshare === true) continue
            var self = t.id === "vid_selfview"
            var u = self ? cs.myId : String(t.participant || "")
            if (u === "" || seated[u] || seen[u]) continue
            seen[u] = true
            out.push({ u: u, self: self, tile: t, sharing: !!sharing[u] || (self && cs.app && cs.app.mCallSharing === true) })
        }
        return out
    }
    visible: cs.people.length > 0
    implicitHeight: cs.visible ? (cs.compact ? 40 : 64) : 0

    function nameOf(u) { return cs.app && cs.app.localpart ? String(cs.app.localpart(u)) : String(u) }

    Row {
        anchors.left: parent.left
        anchors.verticalCenter: parent.verticalCenter
        spacing: 8
        Repeater {
            model: cs.people
            Rectangle {
                id: st
                required property var modelData
                readonly property var t: st.modelData.tile
                readonly property bool muted: st.modelData.self ? (cs.app && (cs.app.mMicMuted || cs.app.mServerMuted)) : st.t.muted === true
                readonly property real level: !cs.app || st.muted ? 0 : cs.app.tileAudioLevel(st.t.members || st.t.participant, st.modelData.self, false)
                readonly property bool speaking: Motion.focused && st.level > 0.02
                objectName: "gameStripTile"
                width: cs.compact ? 36 : 88
                height: cs.compact ? 36 : 56
                radius: 8
                color: cs.compact ? "transparent" : Th.surfaceOverlay
                border.width: cs.compact ? 0 : 1
                border.color: Th.borderSubtle
                Accessible.role: Accessible.StaticText
                Accessible.name: cs.nameOf(st.modelData.u) + (st.muted ? ", muted" : "") + (st.modelData.sharing ? ", sharing" : "")
                Item {   // avatar + the speaking ring + the muted glyph
                    id: av
                    width: 32; height: 32
                    x: cs.compact ? 2 : (st.width - width) / 2
                    y: cs.compact ? 2 : 4
                    Avatar { anchors.centerIn: parent; app: cs.app; size: 28; name: cs.nameOf(st.modelData.u); seed: st.modelData.u }
                    Rectangle {
                        anchors.fill: parent; radius: width / 2; color: "transparent"
                        border.width: 2; border.color: Th.tileRingSpeaking
                        visible: st.speaking
                    }
                    Rectangle {   // muted: the call tiles' mic-off glyph on a 16 px disc, bottom left (as the seats)
                        visible: st.muted
                        width: 16; height: 16; radius: 8
                        x: -4; y: parent.height + 2 - height
                        color: Th.surfaceOverlay
                        border.width: 1; border.color: Th.borderSubtle
                        Kirigami.Icon { anchors.centerIn: parent; width: 11; height: 11; source: "microphone-sensitivity-muted-symbolic"; color: Th.tileMicOffIcon }
                    }
                }
                Row {
                    visible: !cs.compact
                    anchors.horizontalCenter: parent.horizontalCenter
                    y: av.y + av.height + 3
                    PrestigeToken { id: stTok; anchors.verticalCenter: parent.verticalCenter; mxid: String(st.modelData.u || ""); roomId: cs.spaceId; size: 12 }
                    Controls.Label {
                        anchors.verticalCenter: parent.verticalCenter
                        width: Math.min(implicitWidth, 80 - stTok.implicitWidth)
                        text: cs.nameOf(st.modelData.u)
                        textFormat: Text.PlainText
                        elide: Text.ElideRight
                        color: Th.textPrimary
                        font.pointSize: cs.fBody * 0.81
                    }
                }
                Rectangle {   // a sharer: "sharing" + Watch (the stage goes to Call with the share; we never jump on our own)
                    visible: st.modelData.sharing && !cs.compact && !st.modelData.self
                    anchors.right: parent.right; anchors.top: parent.top; anchors.margins: 2
                    radius: 4
                    color: Th.accentBase
                    implicitWidth: watchLbl.implicitWidth + 8
                    implicitHeight: watchLbl.implicitHeight + 2
                    Controls.Label {
                        id: watchLbl
                        anchors.centerIn: parent
                        text: String(cs.r2.watch || "Watch")
                        textFormat: Text.PlainText
                        color: Th.buttonPrimaryFg
                        font.pointSize: cs.fBody * 0.72
                    }
                    TapHandler { onTapped: if (cs.app) cs.app.setGameStage("call") }
                    HoverHandler { cursorShape: Qt.PointingHandCursor }
                    Accessible.role: Accessible.Button
                    Accessible.name: String(cs.r2.watch || "Watch") + " " + cs.nameOf(st.modelData.u)
                    Accessible.onPressAction: if (cs.app) cs.app.setGameStage("call")
                }
            }
        }
    }
}
