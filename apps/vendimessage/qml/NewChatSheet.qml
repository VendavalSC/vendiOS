// NewChatSheet — the compose (pencil) flow: search the user directory and pick
// someone to message, or type an exact @username. Fades + scales in.

import QtQuick
import QtQuick.Controls.Basic

Item {
    id: ns
    property var theme
    property bool open: false
    property bool connected: false   // real backend → search + start a Matrix chat
    property var results: []         // user-directory hits (from Backend.searchResults)
    signal create(string name)       // name = a user id / display name
    signal search(string query)
    signal closed()

    anchors.fill: parent
    visible: opacity > 0
    opacity: open ? 1 : 0
    Behavior on opacity { NumberAnimation { duration: 170; easing.type: Easing.OutQuad } }
    onOpenChanged: if (open) { toField.text = ""; ns.search(""); toField.forceActiveFocus(); }

    // debounce typing → search
    Timer { id: deb; interval: 220; onTriggered: ns.search(toField.text.trim()) }

    Rectangle { anchors.fill: parent; color: Qt.rgba(0, 0, 0, 0.5) }
    TapHandler { onTapped: ns.closed() }

    Rectangle {
        width: 360; height: card.implicitHeight + 36
        anchors.centerIn: parent
        radius: 20
        color: theme.windowBg
        scale: ns.open ? 1 : 0.93
        Behavior on scale { NumberAnimation { duration: 200; easing.type: Easing.OutBack } }
        TapHandler {}

        Column {
            id: card
            anchors.centerIn: parent
            width: parent.width - 36
            spacing: 14

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
                    placeholderText: ns.connected ? "Search people or type @username" : "Name"
                    placeholderTextColor: theme.textSecondary
                    color: theme.textPrimary; font.pixelSize: 14; font.family: theme.ui
                    background: null; verticalAlignment: TextInput.AlignVCenter
                    onTextChanged: if (ns.connected) deb.restart()
                    onAccepted: if (text.trim().length) ns.create(text.trim())
                }
            }

            // ── search results ──
            Rectangle {
                width: parent.width
                height: Math.min(ns.results.length, 4) * 52
                visible: ns.connected && ns.results.length > 0
                color: "transparent"
                ListView {
                    id: resList
                    anchors.fill: parent
                    clip: true
                    model: ns.results
                    spacing: 0
                    delegate: Rectangle {
                        width: resList.width; height: 52; radius: 10
                        color: rHov.hovered ? theme.hoverBg : "transparent"
                        Avatar {
                            id: rav; name: modelData.name; tint: theme.accent; size: 36; ui: theme.ui
                            anchors.left: parent.left; anchors.leftMargin: 6; anchors.verticalCenter: parent.verticalCenter
                        }
                        Column {
                            anchors.left: rav.right; anchors.leftMargin: 10
                            anchors.right: parent.right; anchors.rightMargin: 8
                            anchors.verticalCenter: parent.verticalCenter; spacing: 1
                            Text { text: modelData.name; color: theme.textPrimary; width: parent.width; elide: Text.ElideRight
                                   font.pixelSize: 14; font.weight: Font.DemiBold; font.family: theme.ui }
                            Text { text: modelData.id; color: theme.textSecondary; width: parent.width; elide: Text.ElideRight
                                   font.pixelSize: 12; font.family: theme.ui }
                        }
                        HoverHandler { id: rHov }
                        TapHandler { onTapped: ns.create(modelData.id) }
                    }
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
