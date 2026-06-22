// MatchCard — a football/sports match-result card (Google/FotMob style):
// title, competition + status, a big centred scoreline flanked by each team's
// flag/badge + name, an optional stage line and scorers. Fed by the vendi-ai
// `show_match` tool payload {home, away, hs, as, homeFlag, awayFlag, comp,
// status, stage, scorers[]}.
//
// NOTE: property is `card` — never `data` (that's QML's children default prop).

import QtQuick

Rectangle {
    id: mc
    property var card: ({})
    property color fg: "#cdd6f4"
    property color dim: "#717189"
    property color accent: "#cba6f7"
    property string mono: "JetBrainsMonoNL Nerd Font"

    readonly property var scorers: card.scorers && card.scorers.length ? card.scorers : []
    readonly property string homeFlag: card.homeFlag ? String(card.homeFlag) : ""
    readonly property string awayFlag: card.awayFlag ? String(card.awayFlag) : ""

    implicitHeight: col.implicitHeight + 26
    radius: 16
    color: Qt.rgba(1, 1, 1, 0.05)
    border.width: 1
    border.color: Qt.rgba(1, 1, 1, 0.08)

    Column {
        id: col
        anchors { left: parent.left; right: parent.right; top: parent.top }
        anchors.leftMargin: 18; anchors.rightMargin: 18; anchors.topMargin: 14
        spacing: 10

        // title
        Text {
            width: parent.width
            text: (mc.card.home || "") + "  vs.  " + (mc.card.away || "")
            color: mc.fg; font.family: mc.mono; font.pixelSize: 15; font.weight: Font.DemiBold
            elide: Text.ElideRight
        }

        // competition (left) · status (right)
        Item {
            width: parent.width; height: 15
            Text {
                anchors.left: parent.left; anchors.verticalCenter: parent.verticalCenter
                text: mc.card.comp || ""; color: mc.dim; font.family: mc.mono; font.pixelSize: 12
            }
            Text {
                anchors.right: parent.right; anchors.verticalCenter: parent.verticalCenter
                text: mc.card.status || ""; color: mc.dim; font.family: mc.mono; font.pixelSize: 12
            }
        }

        // scoreline: badge+name | score - score | badge+name
        Item {
            width: parent.width; height: 80

            // ── home ──
            Column {
                anchors { left: parent.left; verticalCenter: parent.verticalCenter }
                width: 96; spacing: 6
                Item {
                    anchors.horizontalCenter: parent.horizontalCenter
                    width: 36; height: 36
                    Text { anchors.centerIn: parent; visible: mc.homeFlag.length > 0
                           text: mc.homeFlag; font.pixelSize: 28 }
                    Rectangle {
                        anchors.fill: parent; visible: mc.homeFlag.length === 0; radius: width / 2
                        color: Qt.rgba(mc.accent.r, mc.accent.g, mc.accent.b, 0.22)
                        border.width: 1; border.color: Qt.rgba(mc.accent.r, mc.accent.g, mc.accent.b, 0.5)
                        Text { anchors.centerIn: parent; color: "white"; font.pixelSize: 16; font.weight: Font.DemiBold
                               text: (mc.card.home || "?").charAt(0).toUpperCase() }
                    }
                }
                Text {
                    width: parent.width; horizontalAlignment: Text.AlignHCenter
                    text: mc.card.home || ""; color: mc.fg; font.family: mc.mono; font.pixelSize: 13
                    wrapMode: Text.WordWrap; maximumLineCount: 2; elide: Text.ElideRight
                }
            }

            // ── score ──
            Row {
                anchors.centerIn: parent
                spacing: 16
                Text { text: mc.card.hs !== undefined ? String(mc.card.hs) : "–"; color: mc.fg
                       font.family: mc.mono; font.pixelSize: 38; font.weight: Font.Light }
                Text { text: "-"; color: mc.dim; font.family: mc.mono; font.pixelSize: 28
                       anchors.verticalCenter: parent.verticalCenter }
                Text { text: mc.card.as !== undefined ? String(mc.card.as) : "–"; color: mc.fg
                       font.family: mc.mono; font.pixelSize: 38; font.weight: Font.Light }
            }

            // ── away ──
            Column {
                anchors { right: parent.right; verticalCenter: parent.verticalCenter }
                width: 96; spacing: 6
                Item {
                    anchors.horizontalCenter: parent.horizontalCenter
                    width: 36; height: 36
                    Text { anchors.centerIn: parent; visible: mc.awayFlag.length > 0
                           text: mc.awayFlag; font.pixelSize: 28 }
                    Rectangle {
                        anchors.fill: parent; visible: mc.awayFlag.length === 0; radius: width / 2
                        color: Qt.rgba(mc.accent.r, mc.accent.g, mc.accent.b, 0.22)
                        border.width: 1; border.color: Qt.rgba(mc.accent.r, mc.accent.g, mc.accent.b, 0.5)
                        Text { anchors.centerIn: parent; color: "white"; font.pixelSize: 16; font.weight: Font.DemiBold
                               text: (mc.card.away || "?").charAt(0).toUpperCase() }
                    }
                }
                Text {
                    width: parent.width; horizontalAlignment: Text.AlignHCenter
                    text: mc.card.away || ""; color: mc.fg; font.family: mc.mono; font.pixelSize: 13
                    wrapMode: Text.WordWrap; maximumLineCount: 2; elide: Text.ElideRight
                }
            }
        }

        // stage (centred)
        Text {
            width: parent.width; horizontalAlignment: Text.AlignHCenter
            visible: mc.card.stage && String(mc.card.stage).length
            text: mc.card.stage || ""; color: mc.dim; font.family: mc.mono; font.pixelSize: 12
        }

        // scorers — stacked vertically, like a real match report
        Row {
            width: parent.width; spacing: 8
            visible: mc.scorers.length > 0
            topPadding: 2
            Text { text: "⚽"; color: mc.dim; font.pixelSize: 12 }
            Column {
                spacing: 3
                Repeater {
                    model: mc.scorers
                    delegate: Text {
                        required property var modelData
                        text: modelData; color: mc.dim; font.family: mc.mono; font.pixelSize: 11
                    }
                }
            }
        }
    }
}
