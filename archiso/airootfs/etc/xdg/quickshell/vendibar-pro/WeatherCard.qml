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

    // night? (from the weather payload's is_day flag)
    readonly property bool night: card.night === true || String(card.night) === "true"
    // condition bucket from the condition text
    readonly property string wcat: {
        var c = String(card.cond || "").toLowerCase();
        if (/thunder|storm/.test(c)) return "storm";
        if (/rain|drizzle|shower/.test(c)) return "rain";
        if (/snow/.test(c)) return "snow";
        if (/cloud|overcast|fog/.test(c)) return "cloud";
        return "clear";
    }
    // [top, bottom] gradient per condition × day/night — clear is blue; cloud/
    // rain get grayer & darker; night drops to deep indigo/slate.
    readonly property var pal: ({
        clear: night ? ["#243a6e", "#121d3a"] : ["#3a8fd6", "#2061ad"],
        cloud: night ? ["#3b414b", "#23272e"] : ["#71808f", "#4c5662"],
        rain:  night ? ["#2c3a46", "#18212a"] : ["#46607a", "#2b3c4a"],
        storm: night ? ["#262f3c", "#141a22"] : ["#3c4b5e", "#232c39"],
        snow:  night ? ["#414856", "#272c35"] : ["#90a6bd", "#5f7185"]
    })
    readonly property var cols: pal[wcat]

    implicitHeight: 104
    radius: 20
    gradient: Gradient {
        orientation: Gradient.Vertical
        GradientStop { position: 0.0; color: wc.cols[0] }
        GradientStop { position: 1.0; color: wc.cols[1] }
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
        text: wc.card.icon ? wc.card.icon : (wc.night ? "🌙" : "☀️")
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
