// WidgetPeople.qml — widgets: who's going to a calendar event (WIDGETS-UI.md §4 Up next line 3; widgets W2 moved it out
// of UpNextWidget.qml unchanged so Game night shows the same faces): 18 px faces 14 apart (a 1.5 px widget.bg ring; none
// on the soon wash), maybe at 0.6, at most 5 + "+n"; nobody: "No replies yet". avatarSlide: a face that appears after
// the first fill slides in from the left (x −6→0 + opacity, the host's `long` step). `host` = the widget (app, hostRef).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Item {
    id: ppl
    property var host                       // the widget: app, hostRef
    property var ev
    property bool soon: false
    readonly property var list: ppl.ev && ppl.ev.rsvp ? (ppl.ev.rsvp.yes || []).concat(ppl.ev.rsvp.maybe || []) : []
    readonly property int yesN: ppl.ev && ppl.ev.rsvp ? (ppl.ev.rsvp.yes || []).length : 0
    width: faces.width
    // avatarSlide: a face that appears after the first fill slides in from the left (x −6→0 + opacity, long)
    readonly property var ids: ppl.list.slice(0, 5).map(function (p) { return p.mxid })
    property var prevIds: null
    onIdsChanged: Qt.callLater(function () { ppl.prevIds = ppl.ids })
    Component.onCompleted: ppl.prevIds = ppl.ids
    WidgetText {
        visible: ppl.list.length === 0
        height: 20
        role: "meta"
        text: "No replies yet"
        color: Th.widgetMutedFg
    }
    Row {
        id: faces
        visible: ppl.list.length > 0
        anchors.verticalCenter: parent.verticalCenter
        spacing: -4
        Repeater {
            model: ppl.ids   // one delegate per face: a change re-creates them (≤ 5), only new ones slide
            delegate: Item {
                id: face
                required property int index
                readonly property var person: ppl.list[index]
                width: 18; height: 18
                opacity: index >= ppl.yesN ? 0.6 : 1
                Rectangle {
                    visible: !ppl.soon
                    x: -1.5; y: -1.5; width: 21; height: 21; radius: 10.5
                    color: Th.widgetBg
                }
                Avatar { anchors.fill: parent; app: ppl.host ? ppl.host.app : null; size: 18; seed: face.person.mxid; name: face.person.name; imageSource: face.person.avatar || "" }
                Component.onCompleted: {
                    if (ppl.prevIds !== null && ppl.prevIds.indexOf(face.person.mxid) < 0 && !!ppl.host && ppl.host.hostRef && ppl.host.hostRef.durLong > 0) enterSide.start()
                }
                ParallelAnimation {
                    id: enterSide
                    NumberAnimation { target: face; property: "opacity"; from: 0; to: face.index >= ppl.yesN ? 0.6 : 1; duration: ppl.host && ppl.host.hostRef ? ppl.host.hostRef.durLong : 0; easing.type: Easing.OutCubic }
                    NumberAnimation { target: faceShift; property: "x"; from: -6; to: 0; duration: ppl.host && ppl.host.hostRef ? ppl.host.hostRef.durLong : 0; easing.type: Easing.OutCubic }
                }
                transform: Translate { id: faceShift; x: 0 }
            }
        }
        Item { visible: ppl.list.length > 5; width: 8; height: 1 }   // margin-left 4 (+ the −4 row spacing)
        Rectangle {
            visible: ppl.list.length > 5
            anchors.verticalCenter: parent.verticalCenter
            height: 16; radius: 4
            width: plusN.implicitWidth + 10
            color: Th.widgetChipBg
            WidgetText { id: plusN; anchors.centerIn: parent; role: "chip"; text: "+" + (ppl.list.length - 5); color: Th.widgetChipFg }
        }
    }
}
