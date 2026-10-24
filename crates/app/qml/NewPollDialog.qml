// NewPollDialog.qml — widgets W3 (2026-09-28): the New poll dialog (WIDGETS-UI.md §17.6; the web's widgets/polls/
// NewPoll.tsx): the question, the answers (2 to start, "Add answer" up to 20, × removes one while more than 2 remain),
// "Hide results until the poll ends" (an undisclosed poll) and Create poll — RoomController.sendPoll runs the shared
// startContent (trimmed, empty answers dropped, the limits and their friendly errors) and sends the poll in Element's
// shape through the local-echo path (the card shows at once). One choice per person, like Element's polls.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Controls.Dialog {
    id: dlg
    property var app
    property var roomCtl: null
    property string error: ""

    title: "New poll"
    modal: true
    parent: Controls.Overlay.overlay
    anchors.centerIn: parent
    // a stable content width (never the dialog's negotiated one: the wrapping error line would loop)
    readonly property real boxWidth: Math.min(420, (parent ? parent.width : 420) - Kirigami.Units.gridUnit * 2)
    implicitWidth: dlg.boxWidth

    onAboutToShow: {
        question.text = ""
        answerModel.clear()
        answerModel.append({ t: "" })
        answerModel.append({ t: "" })
        hideResults.checked = false
        dlg.error = ""
    }
    onOpened: question.forceActiveFocus()

    function create() {
        if (!dlg.roomCtl) return
        var a = []
        for (var i = 0; i < answerModel.count; ++i) a.push(String(answerModel.get(i).t))
        var err = String(dlg.roomCtl.sendPoll(question.text, JSON.stringify(a), !hideResults.checked) || "")
        dlg.error = err
        if (err.length === 0) dlg.close()
    }

    ListModel { id: answerModel }

    ColumnLayout {
        width: dlg.boxWidth - dlg.leftPadding - dlg.rightPadding
        spacing: 8
        WidgetText { role: "label"; text: "Question"; color: Th.widgetMetaFg }
        ThTextField {
            id: question
            Layout.fillWidth: true
            placeholderText: "What are we playing tonight?"
            maximumLength: 340
            Accessible.name: "Question"
            onTextEdited: dlg.error = ""
        }
        WidgetText { Layout.topMargin: 4; role: "label"; text: "Answers"; color: Th.widgetMetaFg }
        Repeater {
            model: answerModel
            delegate: RowLayout {
                id: ans
                required property int index
                required property string t
                Layout.fillWidth: true
                spacing: 6
                ThTextField {
                    id: ansField
                    Layout.fillWidth: true
                    text: ans.t
                    placeholderText: "Answer " + (ans.index + 1)
                    maximumLength: 340
                    Accessible.name: "Answer " + (ans.index + 1)
                    onTextEdited: { answerModel.setProperty(ans.index, "t", text); dlg.error = "" }
                    onAccepted: if (ans.index === answerModel.count - 1 && answerModel.count < 20) answerModel.append({ t: "" })
                }
                Controls.AbstractButton {
                    id: rm
                    visible: answerModel.count > 2
                    Layout.preferredWidth: 26; Layout.preferredHeight: 26
                    hoverEnabled: true
                    focusPolicy: Qt.TabFocus
                    Accessible.name: "Remove answer " + (ans.index + 1)
                    background: Rectangle {
                        radius: 6
                        color: rm.hovered || rm.down ? Th.widgetMenuBgHover : "transparent"
                        border.width: rm.visualFocus ? 1 : 0
                        border.color: Th.widgetRingFocus
                    }
                    contentItem: Item {
                        ThIcon { anchors.centerIn: parent; width: 12; height: 12; app: dlg.app; name: "window-close-symbolic"; color: Th.widgetMenuIcon }
                    }
                    onClicked: answerModel.remove(ans.index)
                }
            }
        }
        Controls.AbstractButton {
            id: addBtn
            visible: answerModel.count < 20
            Layout.preferredHeight: 26
            Layout.preferredWidth: addRow.implicitWidth + 16
            hoverEnabled: true
            focusPolicy: Qt.TabFocus
            Accessible.name: "Add answer"
            background: Rectangle {
                radius: 6
                color: addBtn.hovered || addBtn.down ? Th.widgetPollsNewBgHover : "transparent"
                border.width: addBtn.visualFocus ? 1 : 0
                border.color: Th.widgetRingFocus
            }
            contentItem: Item {
                Row {
                    id: addRow
                    anchors.centerIn: parent
                    spacing: 5
                    ThIcon { anchors.verticalCenter: parent.verticalCenter; width: 12; height: 12; app: dlg.app; name: "list-add-symbolic"; color: addBtn.hovered ? Th.widgetPollsNewFgHover : Th.widgetPollsNewFg }
                    WidgetText { anchors.verticalCenter: parent.verticalCenter; role: "text"; text: "Add answer"; color: addBtn.hovered ? Th.widgetPollsNewFgHover : Th.widgetPollsNewFg }
                }
            }
            onClicked: answerModel.append({ t: "" })
        }
        ThSwitch {
            id: hideResults
            Layout.fillWidth: true
            text: "Hide results until the poll ends"
        }
        WidgetText {
            visible: dlg.error.length > 0
            Layout.fillWidth: true
            role: "meta"
            wrapMode: Text.Wrap
            text: dlg.error
            color: Th.widgetPollsErrorFg
            Accessible.role: Accessible.AlertMessage
        }
    }

    footer: Controls.DialogButtonBox {
        Controls.Button {
            text: "Create poll"
            Controls.DialogButtonBox.buttonRole: Controls.DialogButtonBox.ActionRole
            onClicked: dlg.create()
        }
        Controls.Button {
            text: "Cancel"
            Controls.DialogButtonBox.buttonRole: Controls.DialogButtonBox.RejectRole
        }
    }
}
