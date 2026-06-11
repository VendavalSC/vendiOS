// vendibar Pro — the maximalist vendiOS bar (quickshell/QML).
//
// Counterpart to the minimal GTK vendibar: floating widget islands over the
// wallpaper. Theme accent comes live from ~/.config/vendi/theme-state;
// compositor state over vendi-ctl. Run: quickshell -c vendibar-pro
//
// Islands, left → right:
//   [ shard · workspaces · title ]   [ clock · date ]   [ music ] [ cpu mem ] [ vol net bat ] [ power ]

import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import QtQuick
import QtQuick.Layouts

ShellRoot {
    id: root

    // ── theme ───────────────────────────────────────────────────────────────
    property color accent:  "#cba6f7"
    property color island:  Qt.rgba(0.085, 0.085, 0.13, 0.88)
    property color islandHi: Qt.rgba(0.13, 0.13, 0.19, 0.92)
    property color fg:      "#cdd6f4"
    property color dim:     "#6c7086"
    property string mono:   "JetBrainsMonoNL Nerd Font"

    FileView {
        path: Quickshell.env("HOME") + "/.config/vendi/theme-state"
        watchChanges: true
        onLoaded: {
            const m = /ACCENT_HEX=([0-9a-fA-F]{6})/.exec(text());
            if (m) root.accent = "#" + m[1];
        }
        onFileChanged: reload()
    }

    // ── compositor state ─────────────────────────────────────────────────────
    property int activeWs: 1
    property var wsList: [{ id: 1, windows: 0 }]
    property string title: ""

    function applyWorkspaces(active, list) {
        activeWs = active;
        wsList = list.map(w => ({ id: w.id, windows: w.windows ?? 0 }));
    }

    Process {
        id: wmSub
        command: ["vendi-ctl", "subscribe", "workspace", "window"]
        running: true
        stdout: SplitParser {
            onRead: data => {
                try {
                    const ev = JSON.parse(data);
                    if (ev.event === "workspaces-changed")
                        root.applyWorkspaces(ev.active, ev.workspaces);
                    else if (ev.event === "window-focused")
                        root.title = ev.title ?? "";
                    else if (ev.event === "window-title" && ev.focused)
                        root.title = ev.title ?? "";
                } catch (e) {}
            }
        }
        onExited: subRetry.start()
    }
    Timer { id: subRetry; interval: 2000; onTriggered: wmSub.running = true }

    // Initial snapshot (subscribe only streams changes).
    Process {
        id: wsSnap
        command: ["vendi-ctl", "list-workspaces"]
        running: true
        property var acc: []
        stdout: SplitParser {
            onRead: line => {
                const m = /^(\*?)\s*(\d+)/.exec(line);
                if (m) {
                    wsSnap.acc.push({ id: parseInt(m[2]), windows: 0 });
                    if (m[1] === "*") root.activeWs = parseInt(m[2]);
                }
            }
        }
        onExited: { if (acc.length) root.wsList = acc; acc = []; }
    }

    // ── system state ─────────────────────────────────────────────────────────
    property real cpu: 0
    property real mem: 0
    property var cpuPrev: null
    property int volume: -1
    property bool muted: false
    property string netIcon: "󰤭"
    property int battery: -1
    property bool charging: false
    property bool hasBattery: false
    property string musicStatus: ""
    property string musicTrack: ""

    FileView {
        id: procStat
        path: "/proc/stat"
        onLoaded: {
            const f = text().split("\n")[0].trim().split(/\s+/).slice(1).map(Number);
            const idle = f[3] + f[4], total = f.reduce((a, b) => a + b, 0);
            if (root.cpuPrev) {
                const dt = total - root.cpuPrev.total, di = idle - root.cpuPrev.idle;
                if (dt > 0) root.cpu = Math.max(0, Math.min(100, 100 * (1 - di / dt)));
            }
            root.cpuPrev = { total: total, idle: idle };
        }
    }
    FileView {
        id: memInfo
        path: "/proc/meminfo"
        onLoaded: {
            const t = /MemTotal:\s+(\d+)/.exec(text());
            const a = /MemAvailable:\s+(\d+)/.exec(text());
            if (t && a) root.mem = 100 * (1 - parseInt(a[1]) / parseInt(t[1]));
        }
    }
    Timer {
        interval: 2500; running: true; repeat: true; triggeredOnStart: true
        onTriggered: { procStat.reload(); memInfo.reload(); volProc.running = true; }
    }

    Process {
        id: volProc
        command: ["wpctl", "get-volume", "@DEFAULT_AUDIO_SINK@"]
        stdout: SplitParser {
            onRead: line => {
                const m = /Volume:\s+([\d.]+)(\s+\[MUTED\])?/.exec(line);
                if (m) { root.volume = Math.round(parseFloat(m[1]) * 100); root.muted = !!m[2]; }
            }
        }
    }

    Process {
        id: netProc
        property bool found: false
        command: ["sh", "-c", "nmcli -t -f TYPE,STATE d 2>/dev/null | grep -v unmanaged"]
        stdout: SplitParser {
            onRead: line => {
                if (line.includes(":connected")) {
                    root.netIcon = line.startsWith("wifi") ? "󰤨" : "󰈀";
                    netProc.found = true;
                }
            }
        }
        onStarted: found = false
        onExited: { if (!found) root.netIcon = "󰤭"; }
    }
    Process {
        id: batProc
        property var lines: []
        command: ["sh", "-c",
            "cat /sys/class/power_supply/BAT*/capacity /sys/class/power_supply/BAT*/status 2>/dev/null"]
        stdout: SplitParser { onRead: l => batProc.lines.push(l) }
        onStarted: lines = []
        onExited: {
            root.hasBattery = lines.length >= 2;
            if (root.hasBattery) {
                root.battery = parseInt(lines[0]);
                root.charging = lines[1].trim() === "Charging";
            }
        }
    }
    Timer {
        interval: 8000; running: true; repeat: true; triggeredOnStart: true
        onTriggered: { netProc.running = true; batProc.running = true; }
    }

    Process {
        id: musicProc
        command: ["playerctl", "--follow", "metadata", "--format", "{{status}}|{{artist}} — {{title}}"]
        running: true
        stdout: SplitParser {
            onRead: line => {
                const i = line.indexOf("|");
                if (i < 0) { root.musicStatus = ""; return; }
                root.musicStatus = line.slice(0, i);
                root.musicTrack = line.slice(i + 1);
            }
        }
        onExited: musicRetry.start()
    }
    Timer { id: musicRetry; interval: 5000; onTriggered: musicProc.running = true }

    // ── the bar ──────────────────────────────────────────────────────────────
    Variants {
        model: Quickshell.screens
        PanelWindow {
            id: panel
            required property var modelData
            screen: modelData
            anchors { top: true; left: true; right: true }
            implicitHeight: 42
            color: "transparent"
            WlrLayershell.namespace: "vendibar-pro"

            // reusable island chrome
            component Island: Rectangle {
                default property alias content: inner.data
                property bool hover: false
                width: inner.width + 26
                height: 30
                radius: 15
                color: hover ? root.islandHi : root.island
                border.width: 1
                border.color: Qt.rgba(1, 1, 1, 0.07)
                Behavior on color { ColorAnimation { duration: 140 } }
                RowLayout { id: inner; anchors.centerIn: parent; spacing: 10 }
                HoverHandler { onHoveredChanged: parent.hover = hovered }
            }
            component Mono: Text {
                color: root.fg
                font.family: root.mono
                font.pixelSize: 12
            }

            // ── left: shard · workspaces · title ────────────────────────────
            Island {
                anchors.left: parent.left
                anchors.leftMargin: 10
                anchors.verticalCenter: parent.verticalCenter

                Mono { text: "󰜁"; color: root.accent; font.pixelSize: 16 }

                RowLayout {
                    spacing: 5
                    Repeater {
                        model: root.wsList
                        Rectangle {
                            required property var modelData
                            property bool current: modelData.id === root.activeWs
                            width: current ? 28 : 18
                            height: 18
                            radius: 9
                            color: current ? root.accent
                                 : modelData.windows > 0 ? Qt.rgba(1, 1, 1, 0.12)
                                 : "transparent"
                            Behavior on width { NumberAnimation { duration: 180; easing.type: Easing.OutBack } }
                            Behavior on color { ColorAnimation { duration: 140 } }
                            Mono {
                                anchors.centerIn: parent
                                text: parent.modelData.id
                                color: parent.current ? "#11111b" : root.dim
                                font.pixelSize: 11
                                font.bold: parent.current
                            }
                            MouseArea {
                                anchors.fill: parent
                                cursorShape: Qt.PointingHandCursor
                                onClicked: Quickshell.execDetached(
                                    ["vendi-ctl", "workspace", String(parent.modelData.id)])
                            }
                        }
                    }
                }

                Rectangle { width: 1; height: 14; color: Qt.rgba(1, 1, 1, 0.1); visible: root.title.length > 0 }
                Mono {
                    text: root.title.length > 44 ? root.title.slice(0, 44) + "…" : root.title
                    visible: root.title.length > 0
                    color: root.dim
                }
            }

            // ── center: clock + date ────────────────────────────────────────
            Island {
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.verticalCenter: parent.verticalCenter
                Mono { id: clockT; font.bold: true; font.pixelSize: 13 }
                Mono { id: dateT; color: root.dim }
                Timer {
                    interval: 1000; running: true; repeat: true; triggeredOnStart: true
                    onTriggered: {
                        const now = new Date();
                        clockT.text = Qt.formatDateTime(now, "HH:mm");
                        dateT.text  = Qt.formatDateTime(now, "ddd d MMM");
                    }
                }
            }

            // ── right: islands row ──────────────────────────────────────────
            RowLayout {
                anchors.right: parent.right
                anchors.rightMargin: 10
                anchors.verticalCenter: parent.verticalCenter
                spacing: 8

                // music — hidden when no player
                Island {
                    visible: root.musicStatus.length > 0
                    Mono {
                        text: root.musicStatus === "Playing" ? "󰐊" : "󰏤"
                        color: root.accent
                        font.pixelSize: 13
                    }
                    Mono {
                        text: root.musicTrack.length > 34 ? root.musicTrack.slice(0, 34) + "…" : root.musicTrack
                        color: root.dim
                    }
                    TapHandler {
                        onTapped: Quickshell.execDetached(["playerctl", "play-pause"])
                    }
                }

                // cpu · mem
                Island {
                    Mono { text: "󰻠"; color: root.cpu > 85 ? "#f38ba8" : root.dim; font.pixelSize: 13 }
                    Mono { text: Math.round(root.cpu) + "%" }
                    Mono { text: "󰍛"; color: root.mem > 85 ? "#f38ba8" : root.dim; font.pixelSize: 13 }
                    Mono { text: Math.round(root.mem) + "%" }
                }

                // volume · network · battery
                Island {
                    Mono {
                        text: (root.muted ? "󰝟" : root.volume > 50 ? "󰕾" : root.volume > 0 ? "󰖀" : "󰕿")
                              + "  " + (root.volume < 0 ? "—" : root.volume + "%")
                        color: root.muted ? root.dim : root.fg
                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            acceptedButtons: Qt.LeftButton | Qt.RightButton
                            onClicked: ev => {
                                if (ev.button === Qt.RightButton)
                                    Quickshell.execDetached(["alacritty", "--class", "vendi-float", "-e", "vendi", "audio"]);
                                else
                                    Quickshell.execDetached(["wpctl", "set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"]);
                                volRefresh.start();
                            }
                            onWheel: w => {
                                Quickshell.execDetached(["wpctl", "set-volume", "-l", "1.0",
                                    "@DEFAULT_AUDIO_SINK@", w.angleDelta.y > 0 ? "2%+" : "2%-"]);
                                volRefresh.start();
                            }
                        }
                        Timer { id: volRefresh; interval: 120; onTriggered: volProc.running = true }
                    }

                    Mono {
                        text: root.netIcon
                        font.pixelSize: 13
                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: Quickshell.execDetached(
                                ["alacritty", "--class", "vendi-float", "-e", "vendi", "wifi"])
                        }
                    }

                    Mono {
                        visible: root.hasBattery
                        text: (root.charging ? "󰂄" : root.battery > 80 ? "󰁹"
                              : root.battery > 50 ? "󰁾" : root.battery > 20 ? "󰁻" : "󰁺")
                              + "  " + root.battery + "%"
                        color: root.battery <= 20 && !root.charging ? "#f38ba8" : root.fg
                    }
                }

                // power
                Island {
                    Mono {
                        text: "󰐥"
                        color: root.accent
                        font.pixelSize: 14
                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: Quickshell.execDetached(["vendi-menu", "power"])
                        }
                    }
                }
            }
        }
    }
}
