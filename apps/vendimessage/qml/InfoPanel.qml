// InfoPanel — the contact / group details page. A premium iOS-contact-card:
// a hero avatar, name, drawn quick-action buttons, and grouped rounded sections
// (the member list for groups). Fades + scales in over a dimmed backdrop.

import QtQuick

Item {
    id: ip
    property var theme
    property var convo
    property bool open: false
    signal closed()
    signal act(string which)

    anchors.fill: parent
    visible: opacity > 0
    opacity: open ? 1 : 0
    Behavior on opacity { NumberAnimation { duration: 180; easing.type: Easing.OutQuad } }

    readonly property bool group: convo && convo.group === true
    readonly property color panel: Qt.rgba(theme.textPrimary.r, theme.textPrimary.g, theme.textPrimary.b, 0.05)

    Rectangle { anchors.fill: parent; color: Qt.rgba(0, 0, 0, 0.55) }
    TapHandler { onTapped: ip.closed() }

    Rectangle {
        id: card
        width: 360
        height: Math.min(parent.height - 64, content.implicitHeight + 64)
        anchors.centerIn: parent
        radius: 22
        color: theme.windowBg
        scale: ip.open ? 1 : 0.93
        Behavior on scale { NumberAnimation { duration: 210; easing.type: Easing.OutBack } }
        TapHandler {}   // swallow clicks inside

        Rectangle {   // close
            width: 30; height: 30; radius: 15
            anchors.right: parent.right; anchors.top: parent.top; anchors.margins: 14
            color: xHover.hovered ? ip.panel : "transparent"
            Behavior on color { ColorAnimation { duration: 120 } }
            Canvas {
                anchors.centerIn: parent; width: 12; height: 12
                onPaint: { var c = getContext("2d"); c.clearRect(0,0,12,12); c.strokeStyle = theme.textSecondary;
                           c.lineWidth = 1.5; c.lineCap = "round"; c.beginPath();
                           c.moveTo(2,2); c.lineTo(10,10); c.moveTo(10,2); c.lineTo(2,10); c.stroke(); }
            }
            HoverHandler { id: xHover }
            TapHandler { onTapped: ip.closed() }
        }

        Column {
            id: content
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.top: parent.top; anchors.topMargin: 36
            width: parent.width - 48
            spacing: 14

            // hero
            Item {
                width: 96; height: 96
                anchors.horizontalCenter: parent.horizontalCenter
                Rectangle {   // soft ring
                    anchors.centerIn: parent; width: 96; height: 96; radius: 48
                    color: Qt.rgba(theme.accent.r, theme.accent.g, theme.accent.b, 0.12)
                }
                Avatar {
                    anchors.centerIn: parent
                    name: ip.convo ? ip.convo.name : ""; tint: ip.convo ? ip.convo.color : theme.textSecondary
                    size: 82; ui: theme.ui
                    group: ip.group; members: ip.convo && ip.convo.members ? ip.convo.members : []
                }
            }
            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                text: ip.convo ? ip.convo.name : ""
                color: theme.textPrimary; font.pixelSize: 22; font.weight: Font.Bold; font.family: theme.ui
            }
            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                text: ip.group ? ((ip.convo.members.length + 1) + " members") : "vendiMessage"
                color: theme.textSecondary; font.pixelSize: 13; font.family: theme.ui
                bottomPadding: 4
            }

            // quick actions
            Row {
                anchors.horizontalCenter: parent.horizontalCenter
                spacing: 14
                QuickAction { glyph: "message"; label: "Message"; onClicked: ip.act("message") }
                QuickAction { glyph: "bell"; label: (ip.convo && ip.convo.muted) ? "Muted" : "Mute"
                              active: ip.convo && ip.convo.muted === true; onClicked: ip.act("mute") }
                QuickAction { glyph: "search";  label: "Search"; onClicked: ip.act("search") }
            }

            // block (1:1 chats only)
            Rectangle {
                visible: !ip.group && ip.convo && ip.convo.peer && ip.convo.peer.length > 0
                width: parent.width; height: 46; radius: 12
                color: blkHover.hovered ? Qt.rgba(0.9, 0.32, 0.28, 0.16) : ip.panel
                Behavior on color { ColorAnimation { duration: 120 } }
                Text {
                    anchors.centerIn: parent
                    text: "Block " + (ip.convo ? ip.convo.name : "")
                    color: "#e5534b"; font.pixelSize: 14; font.weight: Font.DemiBold; font.family: theme.ui
                }
                HoverHandler { id: blkHover }
                TapHandler { onTapped: ip.act("block") }
            }

            // grouped section: members (groups) — iOS-style rounded list
            Column {
                visible: ip.group
                width: parent.width
                spacing: 8
                topPadding: 6
                Text { text: "MEMBERS"; color: theme.textSecondary; font.pixelSize: 11; font.letterSpacing: 0.5
                       font.weight: Font.DemiBold; font.family: theme.ui; leftPadding: 4 }
                Rectangle {
                    width: parent.width
                    height: membersCol.height
                    radius: 14
                    color: ip.panel
                    Column {
                        id: membersCol
                        width: parent.width
                        Repeater {
                            model: ip.group ? [{ name: "You", color: ip.convo.color, role: "" }]
                                              .concat(ip.convo.members.map(function (m) {
                                                  return { name: m.name, color: m.color, role: "" }; })) : []
                            Item {
                                width: membersCol.width; height: 54
                                Avatar { id: ma; name: modelData.name; tint: modelData.color; size: 34; ui: theme.ui
                                         anchors.verticalCenter: parent.verticalCenter
                                         anchors.left: parent.left; anchors.leftMargin: 14 }
                                Text { text: modelData.name; color: theme.textPrimary; font.pixelSize: 15; font.family: theme.ui
                                       anchors.left: ma.right; anchors.leftMargin: 12; anchors.verticalCenter: parent.verticalCenter }
                                Rectangle { visible: index > 0; anchors.top: parent.top; anchors.left: ma.right
                                            anchors.leftMargin: 12; anchors.right: parent.right; height: 1
                                            color: Qt.rgba(theme.textPrimary.r, theme.textPrimary.g, theme.textPrimary.b, 0.07) }
                            }
                        }
                    }
                }
            }
        }
    }

    // a circular quick-action with a drawn icon + a label below
    component QuickAction: Column {
        id: qa
        property string glyph: ""
        property string label: ""
        property bool active: false
        signal clicked()
        spacing: 6
        Rectangle {
            id: qaBtn
            width: 50; height: 50; radius: 25
            anchors.horizontalCenter: parent.horizontalCenter
            color: qa.active ? Qt.rgba(theme.accent.r, theme.accent.g, theme.accent.b, 0.22)
                 : qaHover.hovered ? Qt.rgba(theme.accent.r, theme.accent.g, theme.accent.b, 0.16) : ip.panel
            Behavior on color { ColorAnimation { duration: 130 } }
            scale: qaTap.pressed ? 0.9 : (qaHover.hovered ? 1.07 : 1.0)
            Behavior on scale { NumberAnimation { duration: 130; easing.type: Easing.OutBack } }
            Canvas {
                id: qaIcon
                anchors.centerIn: parent; width: 20; height: 20
                onPaint: {
                    var c = getContext("2d"); c.clearRect(0, 0, 20, 20);
                    c.strokeStyle = theme.accent; c.fillStyle = theme.accent;
                    c.lineWidth = 1.7; c.lineCap = "round"; c.lineJoin = "round";
                    if (glyph === "message") {
                        c.beginPath();
                        c.moveTo(4,3); c.lineTo(16,3); c.quadraticCurveTo(18,3,18,5);
                        c.lineTo(18,12); c.quadraticCurveTo(18,14,16,14);
                        c.lineTo(8,14); c.lineTo(5,17); c.lineTo(5,14);
                        c.lineTo(4,14); c.quadraticCurveTo(2,14,2,12);
                        c.lineTo(2,5); c.quadraticCurveTo(2,3,4,3); c.stroke();
                    } else if (glyph === "bell") {
                        c.beginPath(); c.moveTo(5,14); c.quadraticCurveTo(5,6,10,5.5);
                        c.quadraticCurveTo(15,6,15,14); c.lineTo(5,14); c.stroke();
                        c.beginPath(); c.moveTo(10,3.5); c.lineTo(10,5.5); c.stroke();
                        c.beginPath(); c.arc(10,16,1.4,0,Math.PI*2); c.stroke();
                    } else { // search
                        c.beginPath(); c.arc(8.5,8.5,5,0,Math.PI*2); c.stroke();
                        c.beginPath(); c.moveTo(12.2,12.2); c.lineTo(17,17); c.stroke();
                    }
                }
                Connections { target: theme; function onAccentChanged() { qaIcon.requestPaint() } }
            }
            HoverHandler { id: qaHover }
            TapHandler { id: qaTap; onTapped: qa.clicked() }
        }
        Text { anchors.horizontalCenter: parent.horizontalCenter; text: label
               color: theme.textSecondary; font.pixelSize: 11; font.family: theme.ui }
    }
}
