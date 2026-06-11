// vendibar Pro — the maximalist vendiOS bar (quickshell/QML).
//
// Counterpart to the minimal GTK vendibar: a full panel with grouped
// widget islands. Reads the active theme from ~/.config/vendi/theme-state
// (ACCENT_HEX), talks to vendiwm over `vendi-ctl subscribe`.
//
// Run: quickshell -c vendibar-pro

import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import QtQuick
import QtQuick.Layouts

ShellRoot {
    id: root

    // ── theme ───────────────────────────────────────────────────────────
    property color accent: "#cba6f7"
    property color bg:     "#1e1e2eee"
    property color fg:     "#cdd6f4"
    property color dim:    "#6c7086"

    FileView {
        path: Quickshell.env("HOME") + "/.config/vendi/theme-state"
        watchChanges: true
        onLoaded: {
            const m = /ACCENT_HEX=([0-9a-fA-F]{6})/.exec(text());
            if (m) root.accent = "#" + m[1];
        }
        onFileChanged: reload()
    }

    // ── vendiwm state ───────────────────────────────────────────────────
    property int activeWs: 1
    property var workspaces: [1]
    property string title: ""

    Process {
        id: wmSub
        command: ["vendi-ctl", "subscribe", "workspace", "window"]
        running: true
        stdout: SplitParser {
            onRead: data => {
                try {
                    const ev = JSON.parse(data);
                    if (ev.event === "workspaces") {
                        root.activeWs = ev.active;
                        root.workspaces = ev.list.map(w => w.id ?? w);
                    } else if (ev.event === "window-focused") {
                        root.title = ev.title ?? "";
                    }
                } catch (e) {}
            }
        }
        // compositor restarts shouldn't kill the bar
        onExited: restartTimer.start()
    }
    Timer { id: restartTimer; interval: 2000; onTriggered: wmSub.running = true }

    Variants {
        model: Quickshell.screens
        PanelWindow {
            required property var modelData
            screen: modelData
            anchors { top: true; left: true; right: true }
            implicitHeight: 38
            color: "transparent"
            WlrLayershell.namespace: "vendibar-pro"

            // ── left island: workspaces ─────────────────────────────────
            Rectangle {
                anchors.verticalCenter: parent.verticalCenter
                anchors.left: parent.left
                anchors.leftMargin: 10
                width: wsRow.width + 20
                height: 28
                radius: 14
                color: root.bg

                RowLayout {
                    id: wsRow
                    anchors.centerIn: parent
                    spacing: 4
                    Repeater {
                        model: root.workspaces
                        Rectangle {
                            required property var modelData
                            width: modelData === root.activeWs ? 26 : 18
                            height: 18
                            radius: 9
                            color: modelData === root.activeWs ? root.accent : "transparent"
                            Behavior on width { NumberAnimation { duration: 160; easing.type: Easing.OutCubic } }
                            Text {
                                anchors.centerIn: parent
                                text: parent.modelData
                                color: parent.modelData === root.activeWs ? "#11111b" : root.dim
                                font.family: "JetBrainsMonoNL Nerd Font"
                                font.pixelSize: 11
                                font.bold: parent.modelData === root.activeWs
                            }
                            MouseArea {
                                anchors.fill: parent
                                onClicked: wsSwitch.exec(["vendi-ctl", "focus-workspace", String(parent.modelData)])
                            }
                        }
                    }
                }
            }
            Process { id: wsSwitch }

            // ── center island: focused title + clock ───────────────────
            Rectangle {
                anchors.centerIn: parent
                width: centerRow.width + 28
                height: 28
                radius: 14
                color: root.bg
                RowLayout {
                    id: centerRow
                    anchors.centerIn: parent
                    spacing: 12
                    Text {
                        text: root.title.length > 40 ? root.title.slice(0, 40) + "…" : root.title
                        visible: root.title.length > 0
                        color: root.dim
                        font.family: "JetBrainsMonoNL Nerd Font"
                        font.pixelSize: 12
                    }
                    Text {
                        id: clock
                        color: root.fg
                        font.family: "JetBrainsMonoNL Nerd Font"
                        font.pixelSize: 12
                        font.bold: true
                        Timer {
                            interval: 1000; running: true; repeat: true; triggeredOnStart: true
                            onTriggered: clock.text = Qt.formatDateTime(new Date(), "ddd MMM d  HH:mm")
                        }
                    }
                }
            }
        }
    }
}
