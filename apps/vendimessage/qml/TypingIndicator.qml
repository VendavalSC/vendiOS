// TypingIndicator — a received-style bubble with three gently bouncing dots,
// shown at the bottom of the thread while the other person is typing.

import QtQuick

Item {
    id: ti
    property var theme
    property bool active: false

    implicitHeight: active ? bubble.height + 6 : 0
    visible: active
    Behavior on implicitHeight { NumberAnimation { duration: 160; easing.type: Easing.OutQuad } }

    Rectangle {
        id: bubble
        anchors.left: parent.left
        anchors.bottom: parent.bottom
        width: row.width + 28
        height: 34
        radius: 17
        color: theme.bubbleIn
        opacity: ti.active ? 1 : 0
        Behavior on opacity { NumberAnimation { duration: 160 } }

        Row {
            id: row
            anchors.centerIn: parent
            spacing: 5
            Repeater {
                model: 3
                Rectangle {
                    width: 7; height: 7; radius: 3.5
                    color: theme.textSecondary
                    SequentialAnimation on y {
                        running: ti.active; loops: Animation.Infinite
                        PauseAnimation { duration: index * 160 }
                        NumberAnimation { from: 0; to: -5; duration: 280; easing.type: Easing.OutQuad }
                        NumberAnimation { from: -5; to: 0; duration: 280; easing.type: Easing.InQuad }
                        PauseAnimation { duration: (2 - index) * 160 + 120 }
                    }
                }
            }
        }
    }
}
