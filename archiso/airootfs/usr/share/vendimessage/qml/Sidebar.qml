// Sidebar — the conversation list pane: a header (title + theme toggle + new
// message), a rounded search field, and the scrollable list of ConversationRows.

import QtQuick
import QtQuick.Controls.Basic

Item {
    id: sb
    property var theme
    property var model: []
    property var requests: []
    property int currentIndex: 0
    property bool dark: true
    property bool showLogout: false
    property string query: ""
    signal selected(string id)
    signal toggleTheme()
    signal newChat()
    signal logout()
    signal acceptRequest(string id)
    signal rejectRequest(string id)
    function focusSearch() { searchField.forceActiveFocus(); }

    // live-filtered conversation list (search actually works)
    readonly property var view: {
        var q = query.trim().toLowerCase();
        if (!q.length) return model;
        return model.filter(function (c) {
            return String(c.name).toLowerCase().indexOf(q) !== -1
                || String(c.preview).toLowerCase().indexOf(q) !== -1;
        });
    }

    Rectangle { anchors.fill: parent; color: theme.sidebarBg }
    Rectangle {  // right divider
        anchors { right: parent.right; top: parent.top; bottom: parent.bottom }
        width: 1; color: theme.divider
    }

    // header
    Item {
        id: header
        anchors { left: parent.left; right: parent.right; top: parent.top }
        height: 52
        Text {
            text: "Messages"
            color: theme.textPrimary
            font.pixelSize: 20; font.weight: Font.Bold; font.family: theme.ui
            anchors.left: parent.left; anchors.leftMargin: 18
            anchors.verticalCenter: parent.verticalCenter
        }
        Row {
            anchors.right: parent.right; anchors.rightMargin: 12
            anchors.verticalCenter: parent.verticalCenter
            spacing: 2
            IconButton { glyph: sb.dark ? "☾" : "☀"; theme: sb.theme; onClicked: sb.toggleTheme() }
            IconButton { visible: sb.showLogout; glyph: "⎋"; theme: sb.theme; onClicked: sb.logout() }
            IconButton { glyph: "✎"; theme: sb.theme; accent: true; onClicked: sb.newChat() }
        }
    }

    // search
    Rectangle {
        id: search
        anchors { left: parent.left; right: parent.right; top: header.bottom }
        anchors.leftMargin: 12; anchors.rightMargin: 13
        height: 34; radius: 9
        color: theme.inputBg
        Text {
            text: "⌕"; color: theme.textSecondary; font.pixelSize: 17
            anchors.left: parent.left; anchors.leftMargin: 10
            anchors.verticalCenter: parent.verticalCenter
        }
        TextField {
            id: searchField
            anchors.fill: parent
            anchors.leftMargin: 30; anchors.rightMargin: 10
            placeholderText: "Search"
            placeholderTextColor: theme.textSecondary
            color: theme.textPrimary
            font.pixelSize: 14; font.family: theme.ui
            background: null
            verticalAlignment: TextInput.AlignVCenter
            onTextChanged: sb.query = text
        }
    }

    // ── chat requests (pending invites) ──────────────────────────────────────────
    Column {
        id: reqArea
        anchors { left: parent.left; right: parent.right; top: search.bottom }
        anchors.topMargin: sb.requests.length ? 10 : 0
        anchors.leftMargin: 12; anchors.rightMargin: 12
        spacing: 6
        visible: sb.requests.length > 0
        Text {
            text: "Requests"; color: theme.textSecondary
            font.pixelSize: 11; font.weight: Font.DemiBold; font.family: theme.ui
            font.capitalization: Font.AllUppercase; leftPadding: 4
        }
        Repeater {
            model: sb.requests
            Rectangle {
                width: reqArea.width; height: 56; radius: 12
                color: Qt.rgba(theme.accent.r, theme.accent.g, theme.accent.b, 0.10)
                Avatar {
                    id: ra; name: modelData.name; tint: modelData.color; size: 36; ui: theme.ui
                    anchors.left: parent.left; anchors.leftMargin: 10; anchors.verticalCenter: parent.verticalCenter
                }
                Column {
                    anchors.left: ra.right; anchors.leftMargin: 10
                    anchors.right: actions.left; anchors.rightMargin: 8
                    anchors.verticalCenter: parent.verticalCenter; spacing: 1
                    Text { text: modelData.name; color: theme.textPrimary; width: parent.width; elide: Text.ElideRight
                           font.pixelSize: 14; font.weight: Font.DemiBold; font.family: theme.ui }
                    Text { text: "wants to chat"; color: theme.textSecondary
                           font.pixelSize: 12; font.family: theme.ui }
                }
                Row {
                    id: actions
                    anchors.right: parent.right; anchors.rightMargin: 8
                    anchors.verticalCenter: parent.verticalCenter; spacing: 6
                    Rectangle {  // accept
                        width: 28; height: 28; radius: 14; color: theme.accent
                        scale: accTap.pressed ? 0.86 : 1.0
                        Behavior on scale { NumberAnimation { duration: 100 } }
                        Text { anchors.centerIn: parent; text: "✓"; color: "white"; font.pixelSize: 15 }
                        TapHandler { id: accTap; onTapped: sb.acceptRequest(modelData.id) }
                    }
                    Rectangle {  // deny
                        width: 28; height: 28; radius: 14; color: theme.inputBg
                        scale: denTap.pressed ? 0.86 : 1.0
                        Behavior on scale { NumberAnimation { duration: 100 } }
                        Text { anchors.centerIn: parent; text: "✕"; color: theme.textSecondary; font.pixelSize: 13 }
                        TapHandler { id: denTap; onTapped: sb.rejectRequest(modelData.id) }
                    }
                }
            }
        }
    }

    ListView {
        id: list
        anchors { left: parent.left; right: parent.right; top: reqArea.bottom; bottom: parent.bottom }
        anchors.topMargin: 8; anchors.bottomMargin: 8
        clip: true
        model: sb.view
        spacing: 0
        delegate: ConversationRow {
            width: list.width
            theme: sb.theme
            convo: modelData
            // select by stable id (JS-array modelData isn't reference-stable)
            selected: {
                var cur = sb.model[sb.currentIndex];
                return cur && modelData && modelData.id === cur.id;
            }
            onClicked: sb.selected(modelData.id)
        }
        // animate rows settling when the filter changes
        add: Transition { NumberAnimation { property: "opacity"; from: 0; to: 1; duration: 160 } }
        displaced: Transition { NumberAnimation { properties: "y"; duration: 160; easing.type: Easing.OutCubic } }
    }

    // small round icon button used in the header
    component IconButton: Rectangle {
        property var theme
        property string glyph: ""
        property bool accent: false
        signal clicked()
        width: 30; height: 30; radius: 8
        color: ibHover.hovered ? theme.hoverBg : "transparent"
        Text { anchors.centerIn: parent; text: parent.glyph
               color: parent.accent ? theme.accent : theme.textSecondary
               font.pixelSize: 16; font.family: theme.ui }
        HoverHandler { id: ibHover }
        TapHandler { onTapped: parent.clicked() }
    }
}
