// NewChatSheet — the compose (pencil) flow: a small modal to start a new
// conversation. Type a name and press Start/Enter. Fades + scales in.

import QtQuick
import QtQuick.Controls.Basic

Item {
    id: ns
    property var theme
    property bool open: false
    property bool connected: false   // real backend → start a Matrix chat by @user
    signal create(string name)
    signal closed()

    anchors.fill: parent
    visible: opacity > 0
    opacity: open ? 1 : 0
    Behavior on opacity { NumberAnimation { duration: 170; easing.type: Easing.OutQuad } }
    onOpenChanged: if (open) { toField.text = ""; toField.forceActiveFocus(); }

    Rectangle { anchors.fill: parent; color: Qt.rgba(0, 0, 0, 0.5) }
    TapHandler { onTapped: ns.closed() }

    Rectangle {
        width: 340; height: card.implicitHeight + 40
        anchors.centerIn: parent
        radius: 20
        color: theme.windowBg
        scale: ns.open ? 1 : 0.93
        Behavior on scale { NumberAnimation { duration: 200; easing.type: Easing.OutBack } }
        TapHandler {}

        Column {
            id: card
            anchors.centerIn: parent
            width: parent.width - 40
            spacing: 16

            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                text: "New Message"; color: theme.textPrimary
                font.pixelSize: 17; font.weight: Font.Bold; font.family: theme.ui
            }
            Rectangle {
                width: parent.width; height: 42; radius: 11
                color: theme.inputBg
                border.width: 1
                border.color: toField.activeFocus ? Qt.rgba(theme.accent.r, theme.accent.g, theme.accent.b, 0.55) : "transparent"
                Behavior on border.color { ColorAnimation { duration: 150 } }
                Text { text: "To:"; color: theme.textSecondary; font.pixelSize: 14; font.family: theme.ui
                       anchors.left: parent.left; anchors.leftMargin: 14; anchors.verticalCenter: parent.verticalCenter }
                TextField {
                    id: toField
                    anchors.fill: parent; anchors.leftMargin: 42; anchors.rightMargin: 12
                    placeholderText: ns.connected ? "@username" : "Name"; placeholderTextColor: theme.textSecondary
                    color: theme.textPrimary; font.pixelSize: 14; font.family: theme.ui
                    background: null; verticalAlignment: TextInput.AlignVCenter
                    onAccepted: if (text.trim().length) ns.create(text.trim())
                }
            }
            Rectangle {
                width: parent.width; height: 42; radius: 11
                property bool ready: toField.text.trim().length > 0
                color: ready ? theme.accent : theme.inputBg
                Behavior on color { ColorAnimation { duration: 150 } }
                scale: startTap.pressed ? 0.97 : 1.0
                Behavior on scale { NumberAnimation { duration: 110; easing.type: Easing.OutQuad } }
                Text { anchors.centerIn: parent; text: "Start chat"
                       color: parent.ready ? "white" : theme.textSecondary
                       font.pixelSize: 14; font.weight: Font.DemiBold; font.family: theme.ui }
                TapHandler { id: startTap; onTapped: if (toField.text.trim().length) ns.create(toField.text.trim()) }
            }
        }
    }
}
