// Avatar — a circular monogram avatar (coloured disc + initials). In group mode
// it shows two overlapping member discs. When the backend provides a real photo
// we swap in an Image with this as the fallback.

import QtQuick

Item {
    id: av
    property string name: ""
    property color tint: "#7d8590"
    property int size: 40
    property string ui: "Adwaita Sans"
    property bool group: false
    property var members: []
    implicitWidth: size
    implicitHeight: size

    function initials(n) {
        var parts = String(n).trim().split(/\s+/).filter(function (p) { return p.length });
        if (parts.length === 0) return "?";
        if (parts.length === 1) return parts[0].substring(0, 1).toUpperCase();
        return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
    }

    // single avatar
    Rectangle {
        visible: !av.group
        anchors.fill: parent
        radius: width / 2
        gradient: Gradient {
            GradientStop { position: 0.0; color: Qt.lighter(av.tint, 1.25) }
            GradientStop { position: 1.0; color: av.tint }
        }
        Text {
            anchors.centerIn: parent
            text: av.initials(av.name)
            color: "white"
            font.pixelSize: Math.round(av.size * 0.4)
            font.weight: Font.DemiBold
            font.family: av.ui
        }
    }

    // group: two overlapping discs
    Item {
        visible: av.group
        anchors.fill: parent
        readonly property int d: Math.round(av.size * 0.62)
        Repeater {
            model: av.group ? Math.min(2, av.members.length) : 0
            Rectangle {
                width: parent.d; height: parent.d; radius: width / 2
                color: av.members[index] ? av.members[index].color : av.tint
                border.width: 1.5; border.color: "transparent"
                x: index === 0 ? 0 : av.size - width
                y: index === 0 ? 0 : av.size - height
                z: index
                Text {
                    anchors.centerIn: parent
                    text: av.members[index] ? av.initials(av.members[index].name) : ""
                    color: "white"; font.pixelSize: Math.round(parent.width * 0.42)
                    font.weight: Font.DemiBold; font.family: av.ui
                }
            }
        }
    }
}
