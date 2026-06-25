// RoundedImage — an image clipped to rounded corners (Qt has no Image.radius).
// Uses a MultiEffect mask so the corners are genuinely rounded, not boxed.

import QtQuick
import QtQuick.Effects

Item {
    id: ri
    property alias source: img.source
    property int radius: 16
    readonly property real ratio: img.implicitWidth > 0 ? img.implicitHeight / img.implicitWidth : 0.66

    // subtle placeholder while/if the image isn't available
    Rectangle { anchors.fill: parent; radius: ri.radius; color: Qt.rgba(0.5, 0.5, 0.55, 0.18)
                visible: img.status !== Image.Ready }

    Image {
        id: img
        anchors.fill: parent
        fillMode: Image.PreserveAspectCrop
        asynchronous: false      // sync load so the MultiEffect mask never snapshots an empty frame
        cache: true
        visible: false
        layer.enabled: true
    }
    Rectangle {
        id: mask
        anchors.fill: parent
        radius: ri.radius
        visible: false
        layer.enabled: true
    }
    MultiEffect {
        source: img
        anchors.fill: img
        maskEnabled: true
        maskSource: mask
    }
}
