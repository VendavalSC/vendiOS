// A solid colour-coded, battery-shaped badge: a rounded *rectangle* body with
// the % inside (and a bolt when charging) plus a little square terminal on the
// right. Green charging, red ≤20%, theme-fg otherwise. Shared by the bar, the
// home dashboard and the control center. Size scales off `h`.
import QtQuick

Item {
    id: bi
    property int pct: 100
    property bool charging: false
    property color fg: "#cdd6f4"
    property color good: "#a6e3a1"
    property color alert: "#f38ba8"
    property color textColor: "#11111b"
    property string mono: "monospace"
    property real h: 14
    readonly property color tone: charging ? good : pct <= 20 ? alert : fg

    implicitHeight: h
    implicitWidth: body.width + Math.max(2, Math.round(h * 0.24))

    Rectangle {
        id: body
        height: parent.height
        width: row.implicitWidth + Math.round(h * 0.72)
        radius: h * 0.24
        color: bi.tone
        Behavior on color { ColorAnimation { duration: 200 } }
        Row {
            id: row
            anchors.centerIn: parent
            spacing: 1
            Text {
                text: bi.pct
                color: bi.textColor
                font.family: bi.mono
                font.pixelSize: Math.round(bi.h * 0.66)
                font.bold: true
                anchors.verticalCenter: parent.verticalCenter
            }
            Text {
                visible: bi.charging
                text: "󱐋"
                color: bi.textColor
                font.family: bi.mono
                font.pixelSize: Math.round(bi.h * 0.66)
                font.bold: true
                anchors.verticalCenter: parent.verticalCenter
            }
        }
    }
    Rectangle {
        anchors.left: body.right
        anchors.leftMargin: Math.max(1, Math.round(h * 0.07))
        anchors.verticalCenter: body.verticalCenter
        width: Math.max(2, Math.round(h * 0.16))
        height: Math.round(h * 0.42)
        radius: 1
        color: bi.tone
        Behavior on color { ColorAnimation { duration: 200 } }
    }
}
