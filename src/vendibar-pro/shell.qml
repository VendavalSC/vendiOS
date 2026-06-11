// vendibar Pro — dynamic notch bar for vendiOS (quickshell/QML).
//
// One silhouette hugging the top edge: a thin strip with three notches
// flowing out of it. The notches are alive — click the clock and the center
// notch swells into a dashboard (calendar + media); click the stats and the
// right notch opens the control center (volume, system, quick actions).
// Dynamic-Island morphs: widths and heights spring with content.
//
// Theme accent follows ~/.config/vendi/theme-state live; compositor state
// over vendi-ctl.            Run: quickshell -c vendibar-pro
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
    property color panel:  Qt.rgba(0.043, 0.043, 0.071, 0.96)   // #0b0b12
    property color fg:     "#cdd6f4"
    property color dim:    "#717189"
    property string mono:  "JetBrainsMonoNL Nerd Font"

    // geometry
    readonly property int stripH: 3
    readonly property int barH:   38
    readonly property int fillet: 12
    readonly property int bcr:    15
    readonly property int pad:    18

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
            color: "transparent"
            WlrLayershell.namespace: "vendibar-pro"

            // ── expansion state ─────────────────────────────────────────────
            property bool centerOpen: false
            property bool rightOpen: false
            function toggleCenter() { centerOpen = !centerOpen; if (centerOpen) rightOpen = false; }
            function toggleRight()  { rightOpen = !rightOpen;  if (rightOpen) centerOpen = false; }

            // notch dimensions, all springy
            property real lw: leftRow.implicitWidth + root.pad * 2
            property real cw: centerOpen ? 470 : centerRow.implicitWidth + root.pad * 2
            property real rw: rightOpen ? 400 : rightRow.implicitWidth + root.pad * 2
            property real ch: centerOpen ? 332 : root.barH
            property real rh: rightOpen ? 296 : root.barH
            Behavior on lw { NumberAnimation { duration: 220; easing.type: Easing.OutCubic } }
            Behavior on cw { NumberAnimation { duration: 260; easing.type: Easing.OutCubic } }
            Behavior on rw { NumberAnimation { duration: 260; easing.type: Easing.OutCubic } }
            Behavior on ch { NumberAnimation { duration: 280; easing.type: Easing.OutBack } }
            Behavior on rh { NumberAnimation { duration: 280; easing.type: Easing.OutBack } }
            onLwChanged: silhouette.requestPaint()
            onCwChanged: silhouette.requestPaint()
            onRwChanged: silhouette.requestPaint()
            onChChanged: silhouette.requestPaint()
            onRhChanged: silhouette.requestPaint()

            // the window grows with the tallest notch; the desktop never
            // reflows — expansions overlay it.
            implicitHeight: Math.ceil(Math.max(root.barH, ch, rh)) + 4
            exclusiveZone: root.barH

            // only the notches take input — the gaps are click-through
            mask: Region {
                x: 0; y: 0; width: panelWin.lw; height: root.barH
                Region {
                    x: (panelWin.width - panelWin.cw) / 2; y: 0
                    width: panelWin.cw; height: panelWin.ch
                }
                Region {
                    x: panelWin.width - panelWin.rw; y: 0
                    width: panelWin.rw; height: panelWin.rh
                }
            }

            // auto-close when the pointer wanders off an open panel
            HoverHandler { id: panelHover }
            Timer {
                running: (panelWin.centerOpen || panelWin.rightOpen) && !panelHover.hovered
                interval: 1600
                onTriggered: { panelWin.centerOpen = false; panelWin.rightOpen = false; }
            }

            // ── silhouette ──────────────────────────────────────────────────
            Canvas {
                id: silhouette
                anchors.fill: parent
                Connections {
                    target: root
                    function onPanelChanged() { silhouette.requestPaint() }
                }
                onPaint: {
                    const ctx = getContext("2d");
                    const w = width;
                    const s = root.stripH, r = root.fillet, b = root.bcr;
                    const lw = panelWin.lw, cw = panelWin.cw, rw = panelWin.rw;
                    const lh = root.barH, chh = panelWin.ch, rhh = panelWin.rh;
                    const cx = (w - cw) / 2;
                    const rx = w - rw;
                    ctx.reset();
                    ctx.beginPath();
                    ctx.moveTo(0, 0);
                    // left notch — flush with the screen corner
                    ctx.lineTo(0, lh - b);
                    ctx.arcTo(0, lh, b, lh, b);
                    ctx.lineTo(lw - b, lh);
                    ctx.arcTo(lw, lh, lw, lh - b, b);
                    ctx.lineTo(lw, s + r);
                    ctx.arc(lw + r, s + r, r, Math.PI, Math.PI * 1.5, false);
                    // center notch
                    ctx.lineTo(cx - r, s);
                    ctx.arc(cx - r, s + r, r, -Math.PI / 2, 0, false);
                    ctx.lineTo(cx, chh - b);
                    ctx.arcTo(cx, chh, cx + b, chh, b);
                    ctx.lineTo(cx + cw - b, chh);
                    ctx.arcTo(cx + cw, chh, cx + cw, chh - b, b);
                    ctx.lineTo(cx + cw, s + r);
                    ctx.arc(cx + cw + r, s + r, r, Math.PI, Math.PI * 1.5, false);
                    // right notch — flush with the right corner
                    ctx.lineTo(rx - r, s);
                    ctx.arc(rx - r, s + r, r, -Math.PI / 2, 0, false);
                    ctx.lineTo(rx, rhh - b);
                    ctx.arcTo(rx, rhh, rx + b, rhh, b);
                    ctx.lineTo(w - b, rhh);
                    ctx.arcTo(w, rhh, w, rhh - b, b);
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
            component Glyph: Text {
                color: root.dim
                font.family: root.mono
                font.pixelSize: 13
                verticalAlignment: Text.AlignVCenter
            }

            // ── left notch: shard · workspaces · title ──────────────────────
            RowLayout {
                id: leftRow
                x: root.pad
                y: root.stripH
                height: root.barH - root.stripH
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

            // ── center notch collapsed row: clock · date ────────────────────
            RowLayout {
                id: centerRow
                anchors.horizontalCenter: parent.horizontalCenter
                y: root.stripH
                height: root.barH - root.stripH
                spacing: 10
                opacity: panelWin.centerOpen ? 0 : 1
                Behavior on opacity { NumberAnimation { duration: 140 } }
                Mono { id: clockT; font.bold: true; font.pixelSize: 14 }
                Mono { id: dateT; color: root.dim }
                TapHandler { onTapped: panelWin.toggleCenter() }
            }
            Timer {
                interval: 1000; running: true; repeat: true; triggeredOnStart: true
                onTriggered: {
                    const now = new Date();
                    clockT.text = Qt.formatDateTime(now, "HH:mm");
                    dateT.text  = Qt.formatDateTime(now, "ddd d MMM");
                    bigClock.text = Qt.formatDateTime(now, "HH:mm");
                    bigDate.text  = Qt.formatDateTime(now, "dddd, d MMMM");
                }
            }

            // ── dashboard (expanded center notch) ───────────────────────────
            Item {
                id: dashboard
                x: (panelWin.width - panelWin.cw) / 2
                y: root.stripH
                width: panelWin.cw
                height: panelWin.ch - root.stripH
                clip: true
                visible: opacity > 0
                opacity: panelWin.centerOpen ? 1 : 0
                Behavior on opacity { NumberAnimation { duration: 180 } }
                TapHandler { onTapped: {} }   // swallow clicks inside

                // month offset for ‹ › navigation; resets on open
                property int monthOff: 0
                Connections {
                    target: panelWin
                    function onCenterOpenChanged() { if (panelWin.centerOpen) dashboard.monthOff = 0; }
                }

                ColumnLayout {
                    anchors.fill: parent
                    anchors.margins: 20
                    spacing: 10

                    RowLayout {
                        Layout.fillWidth: true
                        ColumnLayout {
                            spacing: 0
                            Mono { id: bigClock; font.pixelSize: 30; font.bold: true; color: root.fg }
                            Mono { id: bigDate; color: root.dim }
                        }
                        Item { Layout.fillWidth: true }
                        Mono {
                            text: "󰅖"
                            color: root.dim
                            MouseArea {
                                anchors.fill: parent
                                cursorShape: Qt.PointingHandCursor
                                onClicked: panelWin.centerOpen = false
                            }
                        }
                    }

                    Rectangle { Layout.fillWidth: true; height: 1; color: Qt.rgba(1,1,1,0.08) }

                    // calendar header + nav
                    RowLayout {
                        Layout.fillWidth: true
                        Mono {
                            id: calTitle
                            font.bold: true
                            color: root.accent
                            text: {
                                const d = new Date();
                                d.setDate(1); d.setMonth(d.getMonth() + dashboard.monthOff);
                                return Qt.formatDateTime(d, "MMMM yyyy");
                            }
                        }
                        Item { Layout.fillWidth: true }
                        Mono {
                            text: "‹"; font.pixelSize: 16; color: root.dim
                            MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor
                                        onClicked: dashboard.monthOff-- }
                        }
                        Mono {
                            text: "›"; font.pixelSize: 16; color: root.dim
                            MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor
                                        onClicked: dashboard.monthOff++ }
                        }
                    }

                    // calendar grid (Monday-first)
                    GridLayout {
                        Layout.fillWidth: true
                        columns: 7
                        rowSpacing: 2
                        columnSpacing: 0
                        Repeater {
                            model: ["Mo","Tu","We","Th","Fr","Sa","Su"]
                            Mono {
                                required property var modelData
                                text: modelData
                                color: root.dim
                                font.pixelSize: 10
                                Layout.fillWidth: true
                                horizontalAlignment: Text.AlignHCenter
                            }
                        }
                        Repeater {
                            model: {
                                const base = new Date();
                                base.setDate(1);
                                base.setMonth(base.getMonth() + dashboard.monthOff);
                                const off = (base.getDay() + 6) % 7;
                                const days = new Date(base.getFullYear(), base.getMonth() + 1, 0).getDate();
                                const today = new Date();
                                const isThisMonth = dashboard.monthOff === 0;
                                const cells = [];
                                for (let i = 0; i < off; i++) cells.push({ d: "", today: false });
                                for (let d = 1; d <= days; d++)
                                    cells.push({ d: String(d), today: isThisMonth && d === today.getDate() });
                                return cells;
                            }
                            Rectangle {
                                required property var modelData
                                Layout.fillWidth: true
                                height: 22
                                radius: 11
                                color: modelData.today ? root.accent : "transparent"
                                Mono {
                                    anchors.centerIn: parent
                                    text: parent.modelData.d
                                    font.pixelSize: 11
                                    font.bold: parent.modelData.today
                                    color: parent.modelData.today ? "#0b0b12"
                                         : parent.modelData.d === "" ? "transparent" : root.fg
                                }
                            }
                        }
                    }

                    Rectangle { Layout.fillWidth: true; height: 1; color: Qt.rgba(1,1,1,0.08) }

                    // media controls
                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 14
                        Mono {
                            text: root.musicStatus.length > 0
                                  ? (root.musicTrack.length > 36 ? root.musicTrack.slice(0, 36) + "…" : root.musicTrack)
                                  : "nothing playing"
                            color: root.dim
                            Layout.fillWidth: true
                        }
                        Glyph {
                            text: "󰒮"; font.pixelSize: 15
                            MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor
                                        onClicked: Quickshell.execDetached(["playerctl", "previous"]) }
                        }
                        Glyph {
                            text: root.musicStatus === "Playing" ? "󰏤" : "󰐊"
                            color: root.accent; font.pixelSize: 16
                            MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor
                                        onClicked: Quickshell.execDetached(["playerctl", "play-pause"]) }
                        }
                        Glyph {
                            text: "󰒭"; font.pixelSize: 15
                            MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor
                                        onClicked: Quickshell.execDetached(["playerctl", "next"]) }
                        }
                    }
                }
            }

            // ── right notch collapsed row ───────────────────────────────────
            RowLayout {
                id: rightRow
                anchors.right: parent.right
                anchors.rightMargin: root.pad
                y: root.stripH
                height: root.barH - root.stripH
                spacing: 12
                opacity: panelWin.rightOpen ? 0 : 1
                Behavior on opacity { NumberAnimation { duration: 140 } }

                RowLayout {
                    spacing: 8
                    visible: root.musicStatus.length > 0
                    Mono {
                        text: root.musicStatus === "Playing" ? "󰐊" : "󰏤"
                        color: root.accent
                        font.pixelSize: 13
                    }
                    Mono {
                        text: root.musicTrack.length > 26 ? root.musicTrack.slice(0, 26) + "…" : root.musicTrack
                        color: root.dim
                    }
                    TapHandler { onTapped: Quickshell.execDetached(["playerctl", "play-pause"]) }
                }
                Sep { visible: root.musicStatus.length > 0 }

                RowLayout {
                    spacing: 6
                    Glyph { text: "󰻠"; color: root.cpu > 85 ? "#f38ba8" : root.dim }
                    Mono { text: Math.round(root.cpu) + "%" }
                    Glyph { text: "󰍛"; color: root.mem > 85 ? "#f38ba8" : root.dim }
                    Mono { text: Math.round(root.mem) + "%" }
                    Glyph { text: root.netIcon }
                    Mono {
                        text: (root.muted ? "󰝟" : "󰕾") + " " + (root.volume < 0 ? "—" : root.volume + "%")
                        color: root.muted ? root.dim : root.fg
                    }
                    Mono {
                        visible: root.hasBattery
                        text: (root.charging ? "󰂄" : "󰁾") + " " + root.battery + "%"
                        color: root.battery <= 20 && !root.charging ? "#f38ba8" : root.fg
                    }
                    TapHandler { onTapped: panelWin.toggleRight() }
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

            // ── control center (expanded right notch) ───────────────────────
            Item {
                id: control
                x: panelWin.width - panelWin.rw
                y: root.stripH
                width: panelWin.rw
                height: panelWin.rh - root.stripH
                clip: true
                visible: opacity > 0
                opacity: panelWin.rightOpen ? 1 : 0
                Behavior on opacity { NumberAnimation { duration: 180 } }
                TapHandler { onTapped: {} }

                ColumnLayout {
                    anchors.fill: parent
                    anchors.margins: 20
                    spacing: 12

                    RowLayout {
                        Layout.fillWidth: true
                        Mono { text: "Control Center"; font.bold: true; color: root.accent }
                        Item { Layout.fillWidth: true }
                        Mono {
                            text: "󰅖"
                            color: root.dim
                            MouseArea {
                                anchors.fill: parent
                                cursorShape: Qt.PointingHandCursor
                                onClicked: panelWin.rightOpen = false
                            }
                        }
                    }

                    // volume slider
                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 10
                        Glyph {
                            text: root.muted ? "󰝟" : "󰕾"
                            color: root.muted ? root.dim : root.fg
                            MouseArea {
                                anchors.fill: parent
                                cursorShape: Qt.PointingHandCursor
                                onClicked: {
                                    Quickshell.execDetached(["wpctl", "set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"]);
                                    ccVolRefresh.start();
                                }
                            }
                        }
                        Rectangle {
                            id: volTrack
                            Layout.fillWidth: true
                            height: 8
                            radius: 4
                            color: Qt.rgba(1, 1, 1, 0.10)
                            Rectangle {
                                width: Math.max(8, parent.width * Math.max(0, root.volume) / 100)
                                height: parent.height
                                radius: 4
                                color: root.muted ? root.dim : root.accent
                                Behavior on width { NumberAnimation { duration: 80 } }
                            }
                            MouseArea {
                                anchors.fill: parent
                                anchors.margins: -6
                                cursorShape: Qt.PointingHandCursor
                                function setVol(mx) {
                                    const pct = Math.round(Math.max(0, Math.min(1, mx / volTrack.width)) * 100);
                                    root.volume = pct;
                                    Quickshell.execDetached(["wpctl", "set-volume", "-l", "1.0",
                                        "@DEFAULT_AUDIO_SINK@", pct + "%"]);
                                }
                                onPressed: m => setVol(m.x - 6)
                                onPositionChanged: m => { if (pressed) setVol(m.x - 6) }
                                onReleased: ccVolRefresh.start()
                            }
                        }
                        Mono { text: (root.volume < 0 ? "—" : root.volume + "%"); Layout.preferredWidth: 38 }
                        Timer { id: ccVolRefresh; interval: 150; onTriggered: volProc.running = true }
                    }

                    // cpu / mem bars
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 8
                        RowLayout {
                            Layout.fillWidth: true; spacing: 10
                            Glyph { text: "󰻠" }
                            Rectangle {
                                Layout.fillWidth: true; height: 8; radius: 4
                                color: Qt.rgba(1, 1, 1, 0.10)
                                Rectangle {
                                    width: parent.width * root.cpu / 100
                                    height: parent.height; radius: 4
                                    color: root.cpu > 85 ? "#f38ba8" : root.accent
                                    Behavior on width { NumberAnimation { duration: 300 } }
                                }
                            }
                            Mono { text: Math.round(root.cpu) + "%"; Layout.preferredWidth: 38 }
                        }
                        RowLayout {
                            Layout.fillWidth: true; spacing: 10
                            Glyph { text: "󰍛" }
                            Rectangle {
                                Layout.fillWidth: true; height: 8; radius: 4
                                color: Qt.rgba(1, 1, 1, 0.10)
                                Rectangle {
                                    width: parent.width * root.mem / 100
                                    height: parent.height; radius: 4
                                    color: root.mem > 85 ? "#f38ba8" : root.accent
                                    Behavior on width { NumberAnimation { duration: 300 } }
                                }
                            }
                            Mono { text: Math.round(root.mem) + "%"; Layout.preferredWidth: 38 }
                        }
                    }

                    Rectangle { Layout.fillWidth: true; height: 1; color: Qt.rgba(1,1,1,0.08) }

                    // quick actions
                    GridLayout {
                        Layout.fillWidth: true
                        columns: 2
                        rowSpacing: 8
                        columnSpacing: 8
                        component QuickAction: Rectangle {
                            property string glyph
                            property string label
                            property var run
                            Layout.fillWidth: true
                            height: 38
                            radius: 12
                            color: qaHover.hovered ? Qt.rgba(1, 1, 1, 0.10) : Qt.rgba(1, 1, 1, 0.05)
                            Behavior on color { ColorAnimation { duration: 120 } }
                            HoverHandler { id: qaHover }
                            RowLayout {
                                anchors.centerIn: parent
                                spacing: 8
                                Glyph { text: glyph; color: root.accent }
                                Mono { text: label }
                            }
                            TapHandler { onTapped: run() }
                        }
                        QuickAction {
                            glyph: "󰤨"; label: "Wi-Fi"
                            run: () => Quickshell.execDetached(["alacritty", "--class", "vendi-float", "-e", "vendi", "wifi"])
                        }
                        QuickAction {
                            glyph: "󰂯"; label: "Bluetooth"
                            run: () => Quickshell.execDetached(["alacritty", "--class", "vendi-float", "-e", "vendi", "bt"])
                        }
                        QuickAction {
                            glyph: "󰕾"; label: "Audio"
                            run: () => Quickshell.execDetached(["alacritty", "--class", "vendi-float", "-e", "vendi", "audio"])
                        }
                        QuickAction {
                            glyph: "󰐥"; label: "Power"
                            run: () => Quickshell.execDetached(["vendi-menu", "power"])
                        }
                    }

                    Item { Layout.fillHeight: true }
                }
            }
        }
    }
}
