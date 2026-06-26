// ConversationRow — one row in the sidebar list: avatar, name, last-message
// preview, timestamp, and an unread dot. Highlights in the accent when selected.

import QtQuick

Item {
    id: row
    property var theme
    property var convo            // { name, color, preview, time, unread }
    property bool selected: false
    signal clicked()

    height: 64

    Rectangle {
        anchors.fill: parent
        anchors.margins: 6
        radius: 10
        color: row.selected ? theme.accent
             : hover.hovered ? theme.hoverBg : "transparent"
        Behavior on color { ColorAnimation { duration: 100 } }
    }

    HoverHandler { id: hover }
    TapHandler { onTapped: row.clicked() }

    Rectangle {   // unread dot — iMessage puts it in the far-left margin
        visible: row.convo && row.convo.unread && !row.selected
        width: 8; height: 8; radius: 4
        color: theme.accent
        anchors.verticalCenter: parent.verticalCenter
        anchors.left: parent.left; anchors.leftMargin: 6
        scale: visible ? 1 : 0
        Behavior on scale { NumberAnimation { duration: 180; easing.type: Easing.OutBack } }
    }

    Avatar {
        id: av
        name: row.convo ? row.convo.name : ""
        tint: row.convo ? row.convo.color : theme.textSecondary
        size: 44
        ui: theme.ui
        group: row.convo ? row.convo.group === true : false
        members: row.convo && row.convo.members ? row.convo.members : []
        anchors.verticalCenter: parent.verticalCenter
        anchors.left: parent.left
        anchors.leftMargin: 22
    }

    Column {
        anchors.left: av.right
        anchors.leftMargin: 12
        anchors.right: time.left
        anchors.rightMargin: 8
        anchors.verticalCenter: parent.verticalCenter
        spacing: 2
        Text {
            text: row.convo ? row.convo.name : ""
            color: row.selected ? "white" : theme.textPrimary
            font.pixelSize: 15; font.weight: Font.DemiBold; font.family: theme.ui
            elide: Text.ElideRight; width: parent.width
        }
        Text {
            text: row.convo ? row.convo.preview : ""
            color: row.selected ? Qt.rgba(1, 1, 1, 0.85) : theme.textSecondary
            font.pixelSize: 13; font.family: theme.ui
            elide: Text.ElideRight; width: parent.width; maximumLineCount: 1
        }
    }

    Text {
        id: time
        text: row.convo ? row.convo.time : ""
        color: row.selected ? Qt.rgba(1, 1, 1, 0.85) : theme.textSecondary
        font.pixelSize: 12; font.family: theme.ui
        anchors.top: parent.top; anchors.topMargin: 14
        anchors.right: parent.right; anchors.rightMargin: 16
    }

}
