// WeatherCard — a rich weather result card (iOS-Weather inspired): a blue
// gradient panel with the location, a big temperature, the condition icon and
// the day's high/low. Fed by the vendi-ai `weather` tool's card payload
// {city, icon, temp, cond, hi, lo}.
//
// Root is a Rectangle (a transparent Item root with anchors.fill children does
// not draw inside a Loader here — Rectangle root renders reliably).

import QtQuick

Rectangle {
    id: wc
    property var card: ({})
    property string mono: "JetBrainsMonoNL Nerd Font"

    implicitHeight: 104
    radius: 20
    gradient: Gradient {
        orientation: Gradient.Vertical
        GradientStop { position: 0.0; color: "#3a8fd6" }
        GradientStop { position: 1.0; color: "#2061ad" }
    }

    // soft top sheen
    Rectangle {
        anchors { left: parent.left; right: parent.right; top: parent.top }
        height: parent.height / 2; radius: parent.radius
        gradient: Gradient {
            GradientStop { position: 0.0; color: Qt.rgba(1, 1, 1, 0.10) }
            GradientStop { position: 1.0; color: "transparent" }
        }
    }

    // location
    Row {
        anchors { left: parent.left; top: parent.top; leftMargin: 18; topMargin: 14 }
        spacing: 6
        Text {
            text: wc.card.city && String(wc.card.city).length ? wc.card.city : "Weather"
            color: "white"; font.family: wc.mono; font.pixelSize: 15; font.weight: Font.DemiBold
        }
        Text { text: "➤"; color: Qt.rgba(1,1,1,0.85); font.pixelSize: 11; rotation: -45
               anchors.verticalCenter: parent.verticalCenter }
    }

    // big temperature
    Text {
        anchors { left: parent.left; bottom: parent.bottom; leftMargin: 18; bottomMargin: 12 }
        text: wc.card.temp ? String(wc.card.temp).replace("C", "") : "--°"
        color: "white"; font.family: wc.mono; font.pixelSize: 40; font.weight: Font.Light
    }

    // icon
    Text {
        anchors { right: parent.right; top: parent.top; rightMargin: 18; topMargin: 10 }
        text: wc.card.icon ? wc.card.icon : "☀️"
        font.pixelSize: 30
    }
    // condition + hi/lo
    Column {
        anchors { right: parent.right; bottom: parent.bottom; rightMargin: 18; bottomMargin: 14 }
        Text { text: wc.card.cond ? wc.card.cond : ""; color: "white"
               font.family: wc.mono; font.pixelSize: 14; anchors.right: parent.right }
        Text {
            text: (wc.card.hi ? "H:" + wc.card.hi : "") + (wc.card.lo ? "  L:" + wc.card.lo : "")
            color: Qt.rgba(1,1,1,0.82); font.family: wc.mono; font.pixelSize: 12
            anchors.right: parent.right
        }
    }
}
