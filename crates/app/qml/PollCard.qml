// PollCard.qml — widgets W3 (2026-09-28): a poll as a card (WIDGETS-UI.md §17.3; the web's widgets/polls/PollCard.tsx).
// "POLL" + the state, the question, one row per answer — a radio (a single choice) or a check (several) that votes on
// click while the poll is open, and, when the disclosure rule shows results (ended, or a disclosed poll once I voted),
// the answer's votes and a bar of its share (the winners of an ended poll in full accent with a star) — then the status
// line and, for the creator or a moderator, "End poll" (a confirm). An opaque card (widget.polls.card.*), so it reads
// the same in my bubble and in others'. Motion: the bars ease to a new share (`barEase`, long 200 ms, only while animate).
//
// `c` = the card JSON (polls_ctl.rs card_json: question, multi, answers [{id, text, votes, pm, mine, winner, next}],
// ended, show, status, state, canEnd, pending); every text and every decision comes from the shared polls logic.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Rectangle {
    id: card
    property var app
    property var c: null
    /// a click votes (an open poll, a room I'm in, not my unsent poll, not a gate sample)
    property bool canVote: false
    property real maxW: 360
    signal voteRequested(var answers)
    signal endRequested()

    readonly property var answers: card.c && card.c.answers ? card.c.answers : []
    readonly property bool multi: !!card.c && card.c.multi === true
    readonly property bool ended: !!card.c && card.c.ended === true
    readonly property bool results: !!card.c && card.c.show === true
    readonly property bool open: !!card.c && !card.ended && card.c.pending !== true
    readonly property real body: Kirigami.Theme.defaultFont.pointSize > 0 ? Kirigami.Theme.defaultFont.pointSize : 10

    implicitWidth: Math.max(260, Math.min(card.maxW, 360))
    implicitHeight: col.implicitHeight + 20
    radius: 10
    color: Th.widgetPollsCardBg
    border.width: 1
    border.color: Th.widgetPollsCardBorder
    Accessible.role: Accessible.Grouping
    Accessible.name: "Poll: " + (card.c ? card.c.question : "")

    ColumnLayout {
        id: col
        x: 12; y: 10
        width: card.width - 24
        spacing: 6
        // head: the glyph · POLL · the state
        RowLayout {
            Layout.fillWidth: true
            spacing: 5
            ThIcon {
                Layout.preferredWidth: 14; Layout.preferredHeight: 14
                Layout.alignment: Qt.AlignVCenter
                app: card.app
                name: "view-statistics-symbolic"
                color: Th.widgetPollsMetaFg
            }
            WidgetText { role: "label"; text: "POLL"; color: Th.widgetPollsMetaFg }
            WidgetText {
                Layout.fillWidth: true
                visible: text.length > 0
                role: "meta"
                text: card.c && card.c.state ? "· " + card.c.state : ""
                color: Th.widgetPollsMetaFg
                elide: Text.ElideRight
            }
            Item { Layout.fillWidth: true; visible: !(card.c && card.c.state) }
        }
        // the question (W-strong 1.05 × body, wraps)
        WidgetText {
            Layout.fillWidth: true
            role: "strong"
            font.pointSize: card.body * 1.05
            wrapMode: Text.Wrap
            text: card.c ? card.c.question : ""
            color: Th.widgetPollsQuestionFg
        }
        // the answers
        Repeater {
            model: card.answers.length
            delegate: AnswerRow {
                required property int index
                Layout.fillWidth: true
                a: card.answers[index]
            }
        }
        // the foot: the status line + End poll
        RowLayout {
            Layout.fillWidth: true
            Layout.minimumHeight: 22
            spacing: 8
            WidgetText {
                Layout.fillWidth: true
                role: "meta"
                text: card.c ? card.c.status : ""
                color: Th.widgetPollsMetaFg
                elide: Text.ElideRight
            }
            Controls.AbstractButton {
                id: endBtn
                visible: !!card.c && card.c.canEnd === true
                Layout.preferredHeight: 22
                Layout.preferredWidth: endLabel.implicitWidth + 16
                hoverEnabled: true
                focusPolicy: Qt.TabFocus
                Accessible.name: "End poll"
                background: Rectangle {
                    radius: 6
                    color: endBtn.hovered || endBtn.down ? Th.widgetPollsEndBgHover : "transparent"
                    border.width: endBtn.visualFocus ? 1 : 0
                    border.color: Th.widgetRingFocus
                }
                contentItem: Item {
                    WidgetText { id: endLabel; anchors.centerIn: parent; role: "meta"; semibold: true; text: "End poll"; color: Th.widgetPollsEndFg }
                }
                onClicked: confirmEnd.open()
            }
        }
    }

    ConfirmDialog {
        id: confirmEnd
        app: card.app
        dialogTitle: "End this poll?"
        message: "Everyone sees the final result, and no more votes are counted."
        confirmText: "End poll"
        onConfirmed: card.endRequested()
    }

    // ── an answer: the mark (16) · the text (wraps) · [the star] · the votes; the share bar under it ──
    component AnswerRow: Rectangle {
        id: ar
        property var a: null
        readonly property bool mine: !!ar.a && ar.a.mine === true
        readonly property bool winner: !!ar.a && ar.a.winner === true
        readonly property bool clickable: card.canVote && card.open && !!ar.a && ar.a.next !== null && ar.a.next !== undefined
        implicitHeight: Math.max(32, arCol.implicitHeight + 12)
        radius: 8
        color: arMa.containsMouse && card.canVote && card.open ? Th.widgetPollsOptionBgHover : Th.widgetPollsOptionBg
        border.width: arMa.activeFocus ? 1 : 0
        border.color: Th.widgetRingFocus
        Accessible.role: card.multi ? Accessible.CheckBox : Accessible.RadioButton
        Accessible.name: ar.a ? ar.a.text + (card.results ? ", " + ar.a.votes : "") : ""
        Accessible.checked: ar.mine
        MouseArea {
            id: arMa
            anchors.fill: parent
            hoverEnabled: true
            enabled: card.canVote && card.open
            activeFocusOnTab: ar.clickable
            cursorShape: ar.clickable ? Qt.PointingHandCursor : Qt.ArrowCursor
            onClicked: if (ar.clickable) card.voteRequested(ar.a.next)
            Keys.onPressed: (k) => { if ((k.key === Qt.Key_Return || k.key === Qt.Key_Enter || k.key === Qt.Key_Space) && ar.clickable) { card.voteRequested(ar.a.next); k.accepted = true } }
        }
        ColumnLayout {
            id: arCol
            x: 10; y: 6
            width: ar.width - 20
            spacing: 5
            RowLayout {
                Layout.fillWidth: true
                spacing: 8
                // the mark: a radio circle / a check square — chosen = accent (a dot / a filled square with a tick)
                Rectangle {
                    Layout.preferredWidth: 16; Layout.preferredHeight: 16
                    Layout.alignment: Qt.AlignVCenter
                    radius: card.multi ? 4 : 8
                    color: card.multi && ar.mine ? Th.widgetPollsOptionChecked : "transparent"
                    border.width: 1.5
                    border.color: ar.mine ? Th.widgetPollsOptionChecked : Th.widgetPollsOptionRing
                    antialiasing: true
                    Rectangle {
                        visible: !card.multi && ar.mine
                        anchors.centerIn: parent
                        width: 8; height: 8; radius: 4
                        color: Th.widgetPollsOptionChecked
                        antialiasing: true
                    }
                    ThIcon {
                        visible: card.multi && ar.mine
                        anchors.centerIn: parent
                        width: 10; height: 10
                        app: card.app
                        name: "checkmark-symbolic"
                        color: Th.widgetPollsOptionCheckedFg
                    }
                }
                WidgetText {
                    Layout.fillWidth: true
                    role: "text"
                    semibold: ar.mine
                    wrapMode: Text.Wrap
                    text: ar.a ? ar.a.text : ""
                    color: Th.widgetPollsOptionFg
                }
                ThIcon {
                    visible: ar.winner
                    Layout.preferredWidth: 12; Layout.preferredHeight: 12
                    Layout.alignment: Qt.AlignVCenter
                    app: card.app
                    name: "starred-symbolic"
                    color: Th.widgetPollsWinnerFg
                }
                WidgetText {
                    visible: card.results
                    role: "num"
                    semibold: ar.winner
                    text: ar.a ? ar.a.votes : ""
                    color: ar.winner ? Th.widgetPollsWinnerFg : Th.widgetPollsMetaFg
                }
            }
            // the share bar (results only): 4 px, radius 2, from x 24 (under the text)
            Rectangle {
                visible: card.results
                Layout.fillWidth: true
                Layout.leftMargin: 24
                Layout.preferredHeight: 4
                radius: 2
                color: Th.widgetPollsBarTrack
                Rectangle {
                    height: parent.height
                    radius: 2
                    width: parent.width * Math.max(0, Math.min(1000, ar.a ? Number(ar.a.pm) : 0)) / 1000
                    color: ar.winner ? Th.widgetPollsBarFillWinner : Th.widgetPollsBarFill
                    Behavior on width { enabled: Motion.durLong > 0; NumberAnimation { duration: Motion.durLong; easing.type: Easing.OutCubic } }
                }
            }
        }
    }
}
