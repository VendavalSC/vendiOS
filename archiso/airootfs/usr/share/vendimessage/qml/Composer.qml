// Composer — a "+" attachments button, a pill text field that grows from one
// line, and a send button. When replying, a banner shows above the input with
// the quoted message and an ✕ to cancel.

import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Dialogs

Item {
    id: comp
    property var theme
    signal send(string text)
    signal attach(string path)

    property string replyName: ""
    property string replyText: ""
    property string replyId: ""
    function setReply(name, text, id) { replyName = name; replyText = text; replyId = id || ""; field.forceActiveFocus(); }
    function clearReply() { replyName = ""; replyText = ""; replyId = ""; }

    readonly property int lineH: 36
    readonly property int maxH: 120
    implicitHeight: content.implicitHeight + 16

    function submit() {
        var t = field.text.replace(/\s+$/, "");
        if (!t.length) return;
        comp.send(t);
        field.text = "";
    }

    FileDialog {
        id: imageDlg
        title: "Send a photo"
        nameFilters: ["Images (*.png *.jpg *.jpeg *.gif *.webp *.bmp)"]
        onAccepted: comp.attach(String(selectedFile))
    }

    Rectangle {  // top divider
        anchors { left: parent.left; right: parent.right; top: parent.top }
        height: 1; color: theme.divider
    }

    Column {
        id: content
        anchors { left: parent.left; right: parent.right; top: parent.top; topMargin: 8 }
        anchors.leftMargin: 0; anchors.rightMargin: 0
        spacing: 6

        // reply banner
        Rectangle {
            id: banner
            visible: comp.replyText.length > 0
            x: 12; width: parent.width - 24
            height: 36; radius: 9
            color: theme.hoverBg
            Rectangle { width: 3; height: 22; radius: 1.5; color: theme.accent
                        anchors.left: parent.left; anchors.leftMargin: 10; anchors.verticalCenter: parent.verticalCenter }
            Column {
                anchors.left: parent.left; anchors.leftMargin: 22; anchors.right: closeReply.left; anchors.rightMargin: 8
                anchors.verticalCenter: parent.verticalCenter
                Text { text: "Replying to " + comp.replyName; color: theme.accent
                       font.pixelSize: 11; font.weight: Font.DemiBold; font.family: theme.ui }
                Text { text: comp.replyText; color: theme.textSecondary
                       font.pixelSize: 12; font.family: theme.ui; elide: Text.ElideRight; width: parent.width }
            }
            Rectangle {
                id: closeReply
                width: 22; height: 22; radius: 11
                color: closeHover.hovered ? theme.hoverBg : "transparent"
                anchors.right: parent.right; anchors.rightMargin: 8; anchors.verticalCenter: parent.verticalCenter
                Text { anchors.centerIn: parent; text: "✕"; color: theme.textSecondary; font.pixelSize: 12 }
                HoverHandler { id: closeHover }
                TapHandler { onTapped: comp.clearReply() }
            }
        }

        // input row
        Item {
            id: inputRow
            width: parent.width
            height: Math.max(comp.lineH, fieldBox.height)

            Rectangle {
                id: plus
                width: 32; height: 32; radius: 16
                color: plusHover.hovered ? theme.hoverBg : "transparent"
                anchors.verticalCenter: fieldBox.verticalCenter
                anchors.left: parent.left; anchors.leftMargin: 12
                scale: plusTap.pressed ? 0.88 : 1.0
                Behavior on scale { NumberAnimation { duration: 110; easing.type: Easing.OutQuad } }
                Text { anchors.centerIn: parent; text: "＋"; color: theme.textSecondary; font.pixelSize: 19 }
                HoverHandler { id: plusHover }
                TapHandler { id: plusTap; onTapped: imageDlg.open() }
            }

            Rectangle {
                id: fieldBox
                anchors.verticalCenter: parent.verticalCenter
                anchors.left: plus.right; anchors.leftMargin: 8
                anchors.right: sendBtn.left; anchors.rightMargin: 8
                height: Math.min(comp.maxH, Math.max(comp.lineH, field.implicitHeight))
                Behavior on height { NumberAnimation { duration: 90; easing.type: Easing.OutQuad } }
                radius: height > comp.lineH + 4 ? 16 : height / 2
                color: theme.inputBg
                border.width: 1
                border.color: field.activeFocus ? Qt.rgba(theme.accent.r, theme.accent.g, theme.accent.b, 0.55)
                                                : "transparent"
                Behavior on border.color { ColorAnimation { duration: 150 } }

                ScrollView {
                    anchors.fill: parent
                    anchors.leftMargin: 6; anchors.rightMargin: 6
                    clip: true
                    ScrollBar.horizontal.policy: ScrollBar.AlwaysOff

                    TextArea {
                        id: field
                        wrapMode: TextArea.Wrap
                        placeholderText: comp.replyText.length ? "Reply…" : "Message"
                        placeholderTextColor: theme.textSecondary
                        color: theme.textPrimary
                        font.pixelSize: 15; font.family: theme.ui
                        topPadding: 8; bottomPadding: 8; leftPadding: 8; rightPadding: 8
                        background: null
                        Keys.onReturnPressed: function (e) {
                            if (e.modifiers & Qt.ShiftModifier) { e.accepted = false; return; }
                            e.accepted = true; comp.submit();
                        }
                    }
                }
            }

            Rectangle {
                id: sendBtn
                width: 32; height: 32; radius: 16
                anchors.verticalCenter: fieldBox.verticalCenter
                anchors.right: parent.right; anchors.rightMargin: 12
                property bool ready: field.text.replace(/\s+/g, "").length > 0
                color: ready ? theme.accent : theme.inputBg
                Behavior on color { ColorAnimation { duration: 150 } }
                onReadyChanged: sendIcon.requestPaint()
                scale: sendTap.pressed ? 0.82 : (ready ? 1.0 : 0.94)
                Behavior on scale { NumberAnimation { duration: 130; easing.type: Easing.OutBack } }
                Canvas {
                    id: sendIcon
                    anchors.centerIn: parent; width: 17; height: 17
                    onPaint: {
                        var ctx = getContext("2d"); ctx.clearRect(0, 0, width, height);
                        ctx.strokeStyle = sendBtn.ready ? "white" : theme.textSecondary;
                        ctx.lineWidth = 2; ctx.lineCap = "round"; ctx.lineJoin = "round";
                        ctx.beginPath(); ctx.moveTo(8.5, 14); ctx.lineTo(8.5, 3.2); ctx.stroke();
                        ctx.beginPath(); ctx.moveTo(3.8, 8); ctx.lineTo(8.5, 3); ctx.lineTo(13.2, 8); ctx.stroke();
                    }
                    Connections { target: theme; function onTextSecondaryChanged() { sendIcon.requestPaint() } }
                }
                TapHandler { id: sendTap; onTapped: comp.submit() }
            }
        }
    }
}
