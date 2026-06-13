// vendibar Pro — overview chrome.
//
// The compositor draws the exposé itself (live, scaled window thumbnails laid
// out per workspace — quickshell can't reproduce that). This overlay adds the
// mission-control framing on top: a spaces strip to jump workspaces and a hint
// footer. Its input region is limited to those two pills, so clicks anywhere
// else fall straight through to the compositor's thumbnail hit-testing.
//
// Shown whenever vendiwm reports the overview is open (the `overview` IPC
// event, surfaced as bar.overviewActive).

import Quickshell
import Quickshell.Wayland
import QtQuick
import QtQuick.Layouts

PanelWindow {
    id: ov
    required property var bar

    color: "transparent"
    WlrLayershell.namespace: "vendibar-pro-overview"
    WlrLayershell.layer: WlrLayer.Overlay
    WlrLayershell.keyboardFocus: WlrKeyboardFocus.None
    anchors { top: true; left: true; right: true; bottom: true }
    exclusiveZone: 0
    visible: bar?.overviewActive ?? false

    readonly property color accent: bar?.accent ?? "#cba6f7"
    readonly property color fg:     bar?.fg ?? "#cdd6f4"
    readonly property color dim:    bar?.dim ?? "#717189"
    readonly property string mono:  bar?.mono ?? "JetBrainsMonoNL Nerd Font"
    readonly property color glass:  Qt.rgba(0.043, 0.043, 0.071, 0.92)
    readonly property color hair:   Qt.rgba(1, 1, 1, 0.08)

    // Only the strip + hint are interactive; everything else clicks through to
    // the compositor's exposé (click a thumbnail to focus that window).
    mask: Region {
        item: strip
        Region { item: hint }
    }

    // ── spaces strip (top center) ──────────────────────────────────────────
    Rectangle {
        id: strip
        anchors { top: parent.top; topMargin: 18; horizontalCenter: parent.horizontalCenter }
        implicitWidth: stripRow.implicitWidth + 24
        height: 44
        radius: 22
        color: ov.glass
        border.width: 1
        border.color: ov.hair

        RowLayout {
            id: stripRow
            anchors.centerIn: parent
            spacing: 8
            Repeater {
                model: ov.bar?.wsList ?? []
                Rectangle {
                    required property var modelData
                    property bool current: modelData.id === (ov.bar?.activeWs ?? 1)
                    implicitWidth: Math.max(34, wsRow.implicitWidth + 18)
                    height: 30
                    radius: 15
                    color: current ? ov.accent
                         : wsHov.hovered ? Qt.rgba(1, 1, 1, 0.12) : Qt.rgba(1, 1, 1, 0.05)
                    Behavior on color { ColorAnimation { duration: 120 } }
                    HoverHandler { id: wsHov; cursorShape: Qt.PointingHandCursor }
                    TapHandler {
                        onTapped: Quickshell.execDetached(
                            ["vendi-ctl", "workspace", String(modelData.id)])
                    }
                    RowLayout {
                        id: wsRow
                        anchors.centerIn: parent
                        spacing: 5
                        Text {
                            text: modelData.id
                            color: parent.parent.current ? "#0b0b12" : ov.fg
                            font.family: ov.mono
                            font.pixelSize: 13
                            font.bold: parent.parent.current
                        }
                        Text {
                            visible: (modelData.windows ?? 0) > 0
                            text: "· " + modelData.windows
                            color: parent.parent.current ? "#0b0b12" : ov.dim
                            font.family: ov.mono
                            font.pixelSize: 11
                        }
                    }
                }
            }
        }
    }

    // ── hint footer (bottom center) ────────────────────────────────────────
    Rectangle {
        id: hint
        anchors { bottom: parent.bottom; bottomMargin: 26; horizontalCenter: parent.horizontalCenter }
        implicitWidth: hintT.implicitWidth + 28
        height: 32
        radius: 16
        color: ov.glass
        border.width: 1
        border.color: ov.hair
        Text {
            id: hintT
            anchors.centerIn: parent
            text: "Click a window to focus  ·  Super + 1–9 to jump  ·  Super + O to close"
            color: ov.dim
            font.family: ov.mono
            font.pixelSize: 11
        }
    }
}
