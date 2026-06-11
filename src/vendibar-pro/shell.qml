// vendibar Pro — seamless notch bar for vendiOS (quickshell/QML).
//
// One continuous silhouette hugging the top edge: a thin strip across the
// screen with three notches flowing out of it (concave fillets, rounded
// bottoms), Dynamic-Island style. Notch widths track their content and
// morph smoothly. Theme accent follows ~/.config/vendi/theme-state live;
// compositor state over vendi-ctl.    Run: quickshell -c vendibar-pro
//
//   ┌────────────────────────────────────────────────────────────────┐  ← strip
//   ╰── shard · ws · title ──╯  ╰── clock · date ──╯  ╰── status ────╯
//
import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import QtQuick
import QtQuick.Layouts

ShellRoot {
    id: root

    // ── theme ────────────────────────────────────────────────────────────────
    property color accent: "#cba6f7"
    property color panel:  Qt.rgba(0.043, 0.043, 0.071, 0.94)   // #0b0b12
    property color fg:     "#cdd6f4"
    property color dim:    "#717189"
    property string mono:  "JetBrainsMonoNL Nerd Font"

    // geometry
    readonly property int stripH: 3      // the edge strip between notches
    readonly property int barH:   38     // notch height including strip
    readonly property int fillet: 12     // concave curve into the strip
    readonly property int bcr:    15     // notch bottom corner radius
    readonly property int pad:    18     // notch content side padding

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
            id: panelWin
            required property var modelData
            screen: modelData
            anchors { top: true; left: true; right: true }
            implicitHeight: root.barH
            color: "transparent"
            WlrLayershell.namespace: "vendibar-pro"

            // animated notch widths, driven by content
            property real lw: leftRow.implicitWidth  + root.pad * 2
            property real cw: centerRow.implicitWidth + root.pad * 2
            property real rw: rightRow.implicitWidth + root.pad * 2
            Behavior on lw { NumberAnimation { duration: 220; easing.type: Easing.OutCubic } }
            Behavior on cw { NumberAnimation { duration: 220; easing.type: Easing.OutCubic } }
            Behavior on rw { NumberAnimation { duration: 220; easing.type: Easing.OutCubic } }
            onLwChanged: silhouette.requestPaint()
            onCwChanged: silhouette.requestPaint()
            onRwChanged: silhouette.requestPaint()

            // ── the silhouette: strip + three seamless notches ──────────────
            Canvas {
                id: silhouette
                anchors.fill: parent
                Connections {
                    target: root
                    function onPanelChanged() { silhouette.requestPaint() }
                }
                onPaint: {
                    const ctx = getContext("2d");
                    const w = width, H = height;
                    const s = root.stripH, r = root.fillet, b = root.bcr;
                    const lw = panelWin.lw, cw = panelWin.cw, rw = panelWin.rw;
                    const cx = (w - cw) / 2;
                    const rx = w - rw;
                    ctx.reset();
                    ctx.beginPath();
                    ctx.moveTo(0, 0);
                    // left notch — flush with the screen corner
                    ctx.lineTo(0, H - b);
                    ctx.arcTo(0, H, b, H, b);
                    ctx.lineTo(lw - b, H);
                    ctx.arcTo(lw, H, lw, H - b, b);
                    ctx.lineTo(lw, s + r);
                    ctx.arc(lw + r, s + r, r, Math.PI, Math.PI * 1.5, false);
                    // strip to center notch
                    ctx.lineTo(cx - r, s);
                    ctx.arc(cx - r, s + r, r, -Math.PI / 2, 0, false);
                    ctx.lineTo(cx, H - b);
                    ctx.arcTo(cx, H, cx + b, H, b);
                    ctx.lineTo(cx + cw - b, H);
                    ctx.arcTo(cx + cw, H, cx + cw, H - b, b);
                    ctx.lineTo(cx + cw, s + r);
                    ctx.arc(cx + cw + r, s + r, r, Math.PI, Math.PI * 1.5, false);
                    // strip to right notch — flush with the right corner
                    ctx.lineTo(rx - r, s);
                    ctx.arc(rx - r, s + r, r, -Math.PI / 2, 0, false);
                    ctx.lineTo(rx, H - b);
                    ctx.arcTo(rx, H, rx + b, H, b);
                    ctx.lineTo(w - b, H);
                    ctx.arcTo(w, H, w, H - b, b);
                    ctx.lineTo(w, 0);
                    ctx.closePath();
                    ctx.fillStyle = root.panel;
                    ctx.fill();
                }
            }

            component Mono: Text {
                color: root.fg
                font.family: root.mono
                font.pixelSize: 12
                verticalAlignment: Text.AlignVCenter
            }
            component Sep: Rectangle {
                width: 1
                Layout.preferredHeight: 14
                color: Qt.rgba(1, 1, 1, 0.10)
            }

            // ── left notch: shard · workspaces · title ──────────────────────
            RowLayout {
                id: leftRow
                x: root.pad
                height: parent.height - root.stripH
                y: root.stripH
                spacing: 12

                Mono { text: "󰜁"; color: root.accent; font.pixelSize: 17 }

                RowLayout {
                    spacing: 5
                    Repeater {
                        model: root.wsList
                        Rectangle {
                            required property var modelData
                            property bool current: modelData.id === root.activeWs
                            Layout.alignment: Qt.AlignVCenter
                            width: current ? 30 : 19
                            height: 19
                            radius: 9.5
                            color: current ? root.accent
                                 : modelData.windows > 0 ? Qt.rgba(1, 1, 1, 0.14)
                                 : Qt.rgba(1, 1, 1, 0.05)
                            Behavior on width { NumberAnimation { duration: 200; easing.type: Easing.OutBack } }
                            Behavior on color { ColorAnimation { duration: 150 } }
                            Mono {
                                anchors.centerIn: parent
                                text: parent.modelData.id
                                color: parent.current ? "#0b0b12" : root.dim
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

                Sep { visible: root.title.length > 0 }
                Mono {
                    text: root.title.length > 42 ? root.title.slice(0, 42) + "…" : root.title
                    visible: root.title.length > 0
                    color: root.dim
                }
            }

            // ── center notch: clock · date ──────────────────────────────────
            RowLayout {
                id: centerRow
                anchors.horizontalCenter: parent.horizontalCenter
                height: parent.height - root.stripH
                y: root.stripH
                spacing: 10
                Mono { id: clockT; font.bold: true; font.pixelSize: 14 }
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

            // ── right notch: music · stats · vol/net/bat · power ────────────
            RowLayout {
                id: rightRow
                anchors.right: parent.right
                anchors.rightMargin: root.pad
                height: parent.height - root.stripH
                y: root.stripH
                spacing: 12

                // music — collapses away when nothing plays
                RowLayout {
                    spacing: 8
                    visible: root.musicStatus.length > 0
                    Mono {
                        text: root.musicStatus === "Playing" ? "󰐊" : "󰏤"
                        color: root.accent
                        font.pixelSize: 13
                    }
                    Mono {
                        text: root.musicTrack.length > 30 ? root.musicTrack.slice(0, 30) + "…" : root.musicTrack
                        color: root.dim
                    }
                    TapHandler { onTapped: Quickshell.execDetached(["playerctl", "play-pause"]) }
                }
                Sep { visible: root.musicStatus.length > 0 }

                Mono { text: "󰻠"; color: root.cpu > 85 ? "#f38ba8" : root.dim; font.pixelSize: 13 }
                Mono { text: Math.round(root.cpu) + "%" }
                Mono { text: "󰍛"; color: root.mem > 85 ? "#f38ba8" : root.dim; font.pixelSize: 13 }
                Mono { text: Math.round(root.mem) + "%" }

                Sep {}

                Mono {
                    text: (root.muted ? "󰝟" : root.volume > 50 ? "󰕾" : root.volume > 0 ? "󰖀" : "󰕿")
                          + " " + (root.volume < 0 ? "—" : root.volume + "%")
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
                          + " " + root.battery + "%"
                    color: root.battery <= 20 && !root.charging ? "#f38ba8" : root.fg
                }

                Sep {}

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
