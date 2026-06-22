// InfoCard — a generic structured-result card (sports scores, stats,
// comparisons, rankings). Fed by the vendi-ai `show_card` tool payload
// {title, subtitle, accent, rows:[{label,value}]}. Rectangle root so it draws
// reliably inside a Loader.

import QtQuick

Rectangle {
    id: ic
    property var card: ({})
    property color accent: "#cba6f7"
    property color fg: "#cdd6f4"
    property color dim: "#717189"
    property string mono: "JetBrainsMonoNL Nerd Font"

    readonly property color tint: (card.accent && String(card.accent).length) ? card.accent : accent
    readonly property var rows: card.rows && card.rows.length ? card.rows : []

    implicitHeight: body.implicitHeight + 26
    radius: 16
    color: Qt.rgba(1, 1, 1, 0.05)
    border.width: 1
    border.color: Qt.rgba(1, 1, 1, 0.09)

    Column {
        id: body
        anchors { left: parent.left; right: parent.right; top: parent.top }
        anchors.leftMargin: 16; anchors.rightMargin: 16; anchors.topMargin: 13
        spacing: 4

        Text {
            width: parent.width
            text: ic.card.title ? ic.card.title : ""
            color: ic.fg; font.family: ic.mono; font.pixelSize: 15; font.weight: Font.DemiBold
            wrapMode: Text.WordWrap
        }
        Text {
            width: parent.width
            visible: ic.card.subtitle && String(ic.card.subtitle).length
            text: ic.card.subtitle ? ic.card.subtitle : ""
            color: ic.dim; font.family: ic.mono; font.pixelSize: 12
            wrapMode: Text.WordWrap
            bottomPadding: 4
        }
        Repeater {
            model: ic.rows
            delegate: Item {
                required property var modelData
                width: body.width
                height: 22
                Text {
                    anchors.left: parent.left; anchors.verticalCenter: parent.verticalCenter
                    text: modelData.label ? modelData.label : ""
                    color: ic.dim; font.family: ic.mono; font.pixelSize: 13
                }
                Text {
                    anchors.right: parent.right; anchors.verticalCenter: parent.verticalCenter
                    text: modelData.value ? modelData.value : ""
                    color: ic.fg; font.family: ic.mono; font.pixelSize: 13; font.weight: Font.Medium
                }
            }
        }
    }
}
