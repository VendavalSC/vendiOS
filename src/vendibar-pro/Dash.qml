// vendi dashboard — what the clock notch becomes when you click it.
//
// The center notch swells into a five-room command center (the collapsed
// notch stays the quiet clock · weather island it always was):
//   Home       big clock · profile · calendar · media · quick settings
//   System     live gauges (cpu/ram/disk/load) · network rates · disks
//   Tasks      persistent kanban — to do / ongoing / done
//              (~/.config/vendi/tasks.json)
//   Wallpapers full-bleed grid picker with live previews
//   Config     themes with swatches · bar choice · keybind reference
//
// This is an Item, not a window: the bar's silhouette is the chrome, the
// dashboard just fills the expanded notch. shell.qml wires bar + close.
//
import Quickshell
import Quickshell.Io
import Quickshell.Widgets
import QtQuick
import QtQuick.Layouts

Item {
    id: dash

    property var bar                       // the ShellRoot — all live state
    property int tab: 0                    // 0 home · 1 system · 2 tasks · 3 walls · 4 config
    property bool typing: false            // a text field has focus — don't auto-close
    signal requestClose()

    readonly property color accent: bar?.accent ?? "#cba6f7"
    readonly property color fg:     bar?.fg ?? "#cdd6f4"
    readonly property color dim:    bar?.dim ?? "#717189"
    readonly property color alert:  bar?.alert ?? "#f38ba8"
    readonly property color good:   bar?.good ?? "#a6e3a1"
    readonly property string mono:  bar?.mono ?? "JetBrainsMonoNL Nerd Font"
    readonly property color cardBg: Qt.rgba(1, 1, 1, 0.04)
    readonly property color cardBr: Qt.rgba(1, 1, 1, 0.07)

    function refresh() {
        sysInfo.running = true;
        dfProc.running = true;
        netSample.running = true;
        tasksFile.reload();
        if (bar) bar.rescanWallpapers();
        forceActiveFocus();
    }

    // page turn: the new room slides in from the side you're heading toward
    function goTab(i) {
        if (i === tab) { refresh(); return; }
        pageFx.stop();
        pages.xoff = i > tab ? 36 : -36;
        pages.opacity = 0;
        tab = i;
        refresh();
        pageFx.restart();
    }
    ParallelAnimation {
        id: pageFx
        NumberAnimation { target: pages; property: "opacity"; to: 1; duration: 230; easing.type: Easing.OutCubic }
        NumberAnimation { target: pages; property: "xoff"; to: 0; duration: 260; easing.type: Easing.OutCubic }
    }

    Keys.onEscapePressed: dash.requestClose()

    // ── shared atoms ─────────────────────────────────────────────────────────
    component Mono: Text {
        color: dash.fg
        font.family: dash.mono
        font.pixelSize: 12
        verticalAlignment: Text.AlignVCenter
    }
    component Glyph: Text {
        color: dash.dim
        font.family: dash.mono
        font.pixelSize: 13
        verticalAlignment: Text.AlignVCenter
    }
    component Card: Rectangle {
        radius: 14
        color: dash.cardBg
        border.width: 1
        border.color: dash.cardBr
    }
    component CardTitle: Text {
        color: dash.dim
        font.family: dash.mono
        font.pixelSize: 10
        font.letterSpacing: 2
        font.bold: true
    }

    function human(b) {
        const u = ["B", "K", "M", "G", "T"];
        let i = 0;
        while (b >= 1024 && i < 4) { b /= 1024; i++; }
        return b.toFixed(b >= 10 || i === 0 ? 0 : 1) + u[i];
    }

    // ── system data (polled only while the System tab is on screen) ──────────
    property string kernel: ""
    property int nproc: 1
    property real load1: 0
    property real cpuGhz: 0
    Process {
        id: sysInfo
        command: ["sh", "-c",
            "uname -r; nproc; cat /proc/loadavg; grep -m1 'cpu MHz' /proc/cpuinfo || echo 'cpu MHz : 0'"]
        stdout: StdioCollector {
            onStreamFinished: {
                const l = text.trim().split("\n");
                if (l.length >= 3) {
                    dash.kernel = l[0];
                    dash.nproc = parseInt(l[1]) || 1;
                    dash.load1 = parseFloat(l[2].split(" ")[0]) || 0;
                }
                const m = /:\s*([\d.]+)/.exec(l[3] ?? "");
                if (m) dash.cpuGhz = parseFloat(m[1]) / 1000;
            }
        }
    }

    property real memUsedGb: 0
    property real memTotGb: 0
    FileView {
        id: memDetail
        path: "/proc/meminfo"
        onLoaded: {
            const t = /MemTotal:\s+(\d+)/.exec(text());
            const a = /MemAvailable:\s+(\d+)/.exec(text());
            if (t && a) {
                dash.memTotGb = parseInt(t[1]) / 1048576;
                dash.memUsedGb = (parseInt(t[1]) - parseInt(a[1])) / 1048576;
            }
        }
    }

    property var disks: []        // [{mount, pct, used, size}]
    Process {
        id: dfProc
        command: ["sh", "-c",
            "df -P -B1 -x tmpfs -x devtmpfs -x efivarfs -x overlay -x squashfs 2>/dev/null | tail -n +2"]
        stdout: StdioCollector {
            onStreamFinished: {
                const out = [];
                for (const line of text.trim().split("\n")) {
                    const f = line.trim().split(/\s+/);
                    if (f.length < 6 || f[0].startsWith("run")) continue;
                    const size = parseFloat(f[1]), used = parseFloat(f[2]);
                    if (!size) continue;
                    out.push({
                        mount: f.slice(5).join(" "),
                        pct: Math.round(100 * used / size),
                        used: used, size: size,
                    });
                }
                out.sort((a, b) => b.size - a.size);
                dash.disks = out.slice(0, 4);
            }
        }
    }
    readonly property var rootDisk: disks.find(d => d.mount === "/") ?? null

    property var netPrev: null    // { name, t, rx, tx }
    property string netIface: ""
    property real netUp: -1
    property real netDown: -1
    Process {
        id: netSample
        command: ["cat", "/proc/net/dev"]
        stdout: StdioCollector {
            onStreamFinished: {
                let best = null;
                for (const line of text.split("\n").slice(2)) {
                    const m = /^\s*(\S+):\s*(.+)$/.exec(line);
                    if (!m || m[1] === "lo") continue;
                    const f = m[2].trim().split(/\s+/).map(Number);
                    const cand = { name: m[1], rx: f[0], tx: f[8] };
                    if (!best || cand.rx + cand.tx > best.rx + best.tx) best = cand;
                }
                if (!best) return;
                const now = Date.now();
                if (dash.netPrev && dash.netPrev.name === best.name) {
                    const dt = (now - dash.netPrev.t) / 1000;
                    if (dt > 0.2) {
                        dash.netDown = Math.max(0, (best.rx - dash.netPrev.rx) / dt);
                        dash.netUp = Math.max(0, (best.tx - dash.netPrev.tx) / dt);
                    }
                }
                dash.netIface = best.name;
                dash.netPrev = { name: best.name, t: now, rx: best.rx, tx: best.tx };
            }
        }
    }
    Timer {
        interval: 2000
        running: dash.visible && dash.tab === 1
        repeat: true
        triggeredOnStart: true
        onTriggered: {
            netSample.running = true;
            dfProc.running = true;
            sysInfo.running = true;
            memDetail.reload();
        }
    }

    // ── tasks (kanban, persisted as json) ────────────────────────────────────
    FileView {
        id: tasksFile
        path: Quickshell.env("HOME") + "/.config/vendi/tasks.json"
        watchChanges: true
        onFileChanged: reload()
        JsonAdapter {
            id: td
            property var todo: []
            property var doing: []
            property var done: []
        }
    }
    function taskAdd(col, text) {
        const t = text.trim();
        if (!t) return;
        td[col] = td[col].concat([t]);
        tasksFile.writeAdapter();
    }
    function taskDel(col, idx) {
        td[col] = td[col].filter((_, i) => i !== idx);
        tasksFile.writeAdapter();
    }
    function taskMove(col, idx, dir) {
        const order = ["todo", "doing", "done"];
        const to = order[order.indexOf(col) + dir];
        if (!to) return;
        const item = td[col][idx];
        td[col] = td[col].filter((_, i) => i !== idx);
        td[to] = td[to].concat([item]);
        tasksFile.writeAdapter();
    }

    // ── theme / bar state (config tab) ───────────────────────────────────────
    property string themeNow: "mocha"
    FileView {
        path: Quickshell.env("HOME") + "/.config/vendi/theme-state"
        watchChanges: true
        onLoaded: {
            const m = /THEME=(\w+)/.exec(text());
            if (m) dash.themeNow = m[1];
        }
        onFileChanged: reload()
    }
    property string barNow: "pro"
    FileView {
        path: Quickshell.env("HOME") + "/.config/vendi/bar"
        watchChanges: true
        onLoaded: dash.barNow = text().trim() || "classic"
        onFileChanged: reload()
    }

    // ── clock ────────────────────────────────────────────────────────────────
    property string clockHM: ""
    property string clockS: ""
    property string clockDate: ""
    Timer {
        interval: 1000; running: dash.visible; repeat: true; triggeredOnStart: true
        onTriggered: {
            const now = new Date();
            dash.clockHM = Qt.formatDateTime(now, "HH:mm");
            dash.clockS = Qt.formatDateTime(now, "ss");
            dash.clockDate = Qt.formatDateTime(now, "dddd, d MMMM yyyy");
        }
    }

    // ── layout ───────────────────────────────────────────────────────────────
    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 20
        anchors.topMargin: 14
        spacing: 14

        // ── tab pills + close ───────────────────────────────────────────────
        RowLayout {
            Layout.fillWidth: true
            spacing: 8
            Item { Layout.fillWidth: true }
            Repeater {
                model: [
                    { g: "󰋜", t: "Home" },
                    { g: "󰍛", t: "System" },
                    { g: "󰄬", t: "Tasks" },
                    { g: "󰸉", t: "Wallpapers" },
                    { g: "󰒓", t: "Config" },
                ]
                Rectangle {
                    required property var modelData
                    required property int index
                    property bool current: dash.tab === index
                    implicitWidth: tabRow.implicitWidth + 30
                    implicitHeight: 32
                    radius: 16
                    color: current ? Qt.rgba(dash.accent.r, dash.accent.g, dash.accent.b, 0.18)
                         : tabHover.hovered ? Qt.rgba(1, 1, 1, 0.07) : "transparent"
                    Behavior on color { ColorAnimation { duration: 130 } }
                    HoverHandler { id: tabHover; cursorShape: Qt.PointingHandCursor }
                    TapHandler { onTapped: dash.goTab(index) }
                    RowLayout {
                        id: tabRow
                        anchors.centerIn: parent
                        spacing: 7
                        Glyph { text: modelData.g; color: current ? dash.accent : dash.dim }
                        Mono {
                            text: modelData.t
                            color: current ? dash.fg : dash.dim
                            font.bold: current
                        }
                    }
                }
            }
            Item { Layout.fillWidth: true }
            Mono {
                text: "󰅖"
                color: dash.dim
                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: dash.requestClose()
                }
            }
        }

        // ── pages ───────────────────────────────────────────────────────────
        StackLayout {
            id: pages
            Layout.fillWidth: true
            Layout.fillHeight: true
            currentIndex: dash.tab
            property real xoff: 0
            transform: Translate { x: pages.xoff }

            // ════ HOME ══════════════════════════════════════════════════════
            RowLayout {
                spacing: 14

                // left column — profile + calendar
                ColumnLayout {
                    Layout.preferredWidth: 248
                    Layout.maximumWidth: 248
                    Layout.fillHeight: true
                    spacing: 14

                    Card {
                        Layout.fillWidth: true
                        Layout.preferredHeight: 112
                        RowLayout {
                            anchors.fill: parent
                            anchors.margins: 16
                            spacing: 14
                            Rectangle {
                                Layout.preferredWidth: 52
                                Layout.preferredHeight: 52
                                radius: 26
                                color: Qt.rgba(dash.accent.r, dash.accent.g, dash.accent.b, 0.16)
                                VendiMark {
                                    anchors.centerIn: parent
                                    accent: dash.accent
                                    implicitWidth: 28
                                    implicitHeight: 28
                                }
                            }
                            ColumnLayout {
                                spacing: 3
                                Mono {
                                    text: dash.bar?.userName ?? "vendi"
                                    font.bold: true
                                    font.pixelSize: 15
                                }
                                Mono { text: "󰖳 vendiwm"; color: dash.dim; font.pixelSize: 10 }
                                Mono {
                                    text: "󰅐 up " + (dash.bar?.uptimeStr ?? "")
                                    color: dash.dim; font.pixelSize: 10
                                }
                            }
                            Item { Layout.fillWidth: true }
                        }
                    }

                    Card {
                        id: calCard
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        property int monthOff: 0
                        ColumnLayout {
                            anchors.fill: parent
                            anchors.margins: 16
                            spacing: 8
                            RowLayout {
                                Layout.fillWidth: true
                                Mono {
                                    font.bold: true
                                    color: dash.accent
                                    text: {
                                        const d = new Date();
                                        d.setDate(1);
                                        d.setMonth(d.getMonth() + calCard.monthOff);
                                        return Qt.formatDateTime(d, "MMMM yyyy");
                                    }
                                }
                                Item { Layout.fillWidth: true }
                                Mono {
                                    text: "‹"; font.pixelSize: 16; color: dash.dim
                                    MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor
                                                onClicked: calCard.monthOff-- }
                                }
                                Mono {
                                    text: "›"; font.pixelSize: 16; color: dash.dim
                                    MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor
                                                onClicked: calCard.monthOff++ }
                                }
                            }
                            GridLayout {
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                columns: 7
                                rowSpacing: 2
                                columnSpacing: 0
                                Repeater {
                                    model: ["Mo","Tu","We","Th","Fr","Sa","Su"]
                                    Mono {
                                        required property var modelData
                                        text: modelData
                                        color: dash.dim
                                        font.pixelSize: 10
                                        Layout.fillWidth: true
                                        horizontalAlignment: Text.AlignHCenter
                                    }
                                }
                                Repeater {
                                    model: {
                                        const base = new Date();
                                        base.setDate(1);
                                        base.setMonth(base.getMonth() + calCard.monthOff);
                                        const off = (base.getDay() + 6) % 7;
                                        const days = new Date(base.getFullYear(), base.getMonth() + 1, 0).getDate();
                                        const today = new Date();
                                        const isThis = calCard.monthOff === 0;
                                        const cells = [];
                                        for (let i = 0; i < off; i++) cells.push({ d: "", today: false });
                                        for (let d = 1; d <= days; d++)
                                            cells.push({ d: String(d), today: isThis && d === today.getDate() });
                                        return cells;
                                    }
                                    Rectangle {
                                        required property var modelData
                                        Layout.fillWidth: true
                                        Layout.fillHeight: true
                                        radius: 9
                                        color: modelData.today ? dash.accent : "transparent"
                                        Mono {
                                            anchors.centerIn: parent
                                            text: parent.modelData.d
                                            font.pixelSize: 11
                                            font.bold: parent.modelData.today
                                            color: parent.modelData.today ? "#0b0b12"
                                                 : parent.modelData.d === "" ? "transparent" : dash.fg
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // center column — clock + media
                ColumnLayout {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    spacing: 14

                    Card {
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        ColumnLayout {
                            anchors.centerIn: parent
                            spacing: 4
                            RowLayout {
                                Layout.alignment: Qt.AlignHCenter
                                spacing: 8
                                Mono {
                                    text: dash.clockHM
                                    font.pixelSize: 62
                                    font.bold: true
                                }
                                Mono {
                                    text: dash.clockS
                                    color: dash.dim
                                    font.pixelSize: 20
                                    Layout.alignment: Qt.AlignBottom
                                    Layout.bottomMargin: 11
                                }
                            }
                            Mono {
                                text: dash.clockDate
                                color: dash.dim
                                Layout.alignment: Qt.AlignHCenter
                            }
                            Mono {
                                visible: (dash.bar?.weather ?? "") !== ""
                                text: (dash.bar?.weather ?? "")
                                      + ((dash.bar?.weatherCond ?? "") !== ""
                                         ? "  ·  " + dash.bar.weatherCond : "")
                                color: dash.dim
                                font.pixelSize: 11
                                Layout.alignment: Qt.AlignHCenter
                                Layout.topMargin: 6
                            }
                        }
                    }

                    // media — art washes the card, controls float over it
                    Card {
                        Layout.fillWidth: true
                        Layout.preferredHeight: 142
                        ClippingRectangle {
                            anchors.fill: parent
                            radius: parent.radius
                            color: "transparent"
                            Image {
                                anchors.fill: parent
                                source: dash.bar?.player?.trackArtUrl ?? ""
                                fillMode: Image.PreserveAspectCrop
                                sourceSize.width: 640
                                asynchronous: true
                                opacity: 0.14
                                visible: (dash.bar?.player?.trackArtUrl ?? "") !== ""
                            }
                        }
                        RowLayout {
                            anchors.fill: parent
                            anchors.margins: 16
                            spacing: 16
                            ClippingRectangle {
                                Layout.preferredWidth: 92
                                Layout.preferredHeight: 92
                                Layout.alignment: Qt.AlignVCenter
                                radius: 12
                                color: Qt.rgba(1, 1, 1, 0.06)
                                Image {
                                    anchors.fill: parent
                                    source: dash.bar?.player?.trackArtUrl ?? ""
                                    fillMode: Image.PreserveAspectCrop
                                    sourceSize.width: 184
                                    asynchronous: true
                                    visible: (dash.bar?.player?.trackArtUrl ?? "") !== ""
                                }
                                Glyph {
                                    anchors.centerIn: parent
                                    text: "󰝚"
                                    font.pixelSize: 28
                                    visible: (dash.bar?.player?.trackArtUrl ?? "") === ""
                                }
                            }
                            ColumnLayout {
                                Layout.fillWidth: true
                                spacing: 5
                                Mono {
                                    Layout.fillWidth: true
                                    text: (dash.bar?.player?.trackTitle ?? "") || "Nothing playing"
                                    font.bold: true
                                    font.pixelSize: 14
                                    elide: Text.ElideRight
                                    color: dash.bar?.player ? dash.fg : dash.dim
                                }
                                Mono {
                                    Layout.fillWidth: true
                                    text: dash.bar?.player?.trackArtist
                                        || "music shows up here when it plays"
                                    color: dash.dim
                                    font.pixelSize: 11
                                    elide: Text.ElideRight
                                }
                                Rectangle {
                                    id: seekTrack
                                    Layout.fillWidth: true
                                    Layout.topMargin: 4
                                    height: 5
                                    radius: 2.5
                                    color: Qt.rgba(1, 1, 1, 0.10)
                                    visible: (dash.bar?.player ?? null) !== null
                                    Rectangle {
                                        width: parent.width * (dash.bar?.musicProgress ?? 0)
                                        height: parent.height
                                        radius: 2.5
                                        color: dash.accent
                                        Behavior on width { NumberAnimation { duration: 500 } }
                                    }
                                    MouseArea {
                                        anchors.fill: parent
                                        anchors.margins: -6
                                        cursorShape: Qt.PointingHandCursor
                                        onClicked: m => {
                                            const p = dash.bar?.player;
                                            if (p && p.canSeek && p.length > 0)
                                                p.position = Math.max(0, Math.min(1,
                                                    (m.x - 6) / seekTrack.width)) * p.length;
                                        }
                                    }
                                }
                                RowLayout {
                                    Layout.alignment: Qt.AlignHCenter
                                    Layout.topMargin: 2
                                    spacing: 26
                                    Glyph {
                                        text: "󰒮"; font.pixelSize: 17
                                        color: dash.fg
                                        MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor
                                                    onClicked: dash.bar?.player?.previous() }
                                    }
                                    Rectangle {
                                        Layout.preferredWidth: 32
                                        Layout.preferredHeight: 32
                                        radius: 16
                                        color: Qt.rgba(dash.accent.r, dash.accent.g, dash.accent.b, 0.18)
                                        Glyph {
                                            anchors.centerIn: parent
                                            text: (dash.bar?.musicPlaying ?? false) ? "󰏤" : "󰐊"
                                            color: dash.accent
                                            font.pixelSize: 15
                                        }
                                        MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor
                                                    onClicked: dash.bar?.player?.togglePlaying() }
                                    }
                                    Glyph {
                                        text: "󰒭"; font.pixelSize: 17
                                        color: dash.fg
                                        MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor
                                                    onClicked: dash.bar?.player?.next() }
                                    }
                                }
                            }
                        }
                    }
                }

                // right column — weather + quick settings + volume
                ColumnLayout {
                    Layout.preferredWidth: 224
                    Layout.maximumWidth: 224
                    Layout.fillHeight: true
                    spacing: 14

                    Card {
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        ColumnLayout {
                            anchors.fill: parent
                            anchors.margins: 14
                            spacing: 10
                            CardTitle { text: "QUICK SETTINGS" }
                            GridLayout {
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                columns: 2
                                rowSpacing: 8
                                columnSpacing: 8
                                component Tile: Rectangle {
                                    property string glyph
                                    property string label
                                    property bool active: false
                                    property var run
                                    Layout.fillWidth: true
                                    Layout.fillHeight: true
                                    radius: 12
                                    color: active ? Qt.rgba(dash.accent.r, dash.accent.g, dash.accent.b, 0.20)
                                         : tileHover.hovered ? Qt.rgba(1, 1, 1, 0.09) : Qt.rgba(1, 1, 1, 0.04)
                                    Behavior on color { ColorAnimation { duration: 120 } }
                                    HoverHandler { id: tileHover; cursorShape: Qt.PointingHandCursor }
                                    TapHandler { onTapped: run() }
                                    ColumnLayout {
                                        anchors.centerIn: parent
                                        spacing: 4
                                        Glyph {
                                            text: glyph
                                            color: active ? dash.accent : dash.fg
                                            font.pixelSize: 17
                                            Layout.alignment: Qt.AlignHCenter
                                        }
                                        Mono {
                                            text: label
                                            font.pixelSize: 9
                                            color: dash.dim
                                            Layout.alignment: Qt.AlignHCenter
                                        }
                                    }
                                }
                                Tile {
                                    glyph: (dash.bar?.dnd ?? false) ? "󰂛" : "󰂚"
                                    label: (dash.bar?.dnd ?? false) ? "Silenced" : "DND"
                                    active: dash.bar?.dnd ?? false
                                    run: () => { if (dash.bar) dash.bar.dnd = !dash.bar.dnd; }
                                }
                                Tile {
                                    glyph: "󰻃"
                                    label: (dash.bar?.recording ?? false) ? "Recording" : "Record"
                                    active: dash.bar?.recording ?? false
                                    run: () => {
                                        if (!dash.bar) return;
                                        if (!dash.bar.hasRecorder)
                                            dash.bar.notify("Screen recording",
                                                "wf-recorder is not installed");
                                        else if (dash.bar.recording) dash.bar.stopRecord();
                                        else { dash.requestClose(); dash.bar.startRecord(); }
                                    }
                                }
                                Tile {
                                    glyph: "󰤨"; label: "Wi-Fi"
                                    run: () => Quickshell.execDetached(["alacritty", "--class", "vendi-float", "-e", "vendi", "wifi"])
                                }
                                Tile {
                                    glyph: "󰂯"; label: "Bluetooth"
                                    run: () => Quickshell.execDetached(["alacritty", "--class", "vendi-float", "-e", "vendi", "bt"])
                                }
                                Tile {
                                    glyph: "󰕾"; label: "Audio"
                                    run: () => Quickshell.execDetached(["alacritty", "--class", "vendi-float", "-e", "vendi", "audio"])
                                }
                                Tile {
                                    glyph: "󰹑"; label: "Shot"
                                    run: () => {
                                        dash.requestClose();
                                        Quickshell.execDetached(["sh", "-c",
                                            "sleep 0.3; grim -g \"$(slurp)\" - | wl-copy -t image/png"]);
                                    }
                                }
                                Tile {
                                    glyph: "󰈊"; label: "Pick color"
                                    run: () => {
                                        dash.requestClose();
                                        Quickshell.execDetached(["sh", "-c",
                                            "sleep 0.3; grim -g \"$(slurp -p)\" -t ppm - | " +
                                            "python3 -c 'import sys;d=sys.stdin.buffer.read();" +
                                            "print(\"#%02x%02x%02x\"%(d[-3],d[-2],d[-1]))' | wl-copy"]);
                                    }
                                }
                                Tile {
                                    glyph: "󰌾"; label: "Lock"
                                    run: () => {
                                        dash.requestClose();
                                        Quickshell.execDetached(["vendi-ctl", "lock"]);
                                    }
                                }
                            }
                            // volume slider
                            RowLayout {
                                Layout.fillWidth: true
                                spacing: 8
                                Glyph {
                                    text: (dash.bar?.muted ?? false) ? "󰝟" : "󰕾"
                                    MouseArea {
                                        anchors.fill: parent
                                        cursorShape: Qt.PointingHandCursor
                                        onClicked: {
                                            if (dash.bar?.sinkAudio)
                                                dash.bar.sinkAudio.muted = !dash.bar.sinkAudio.muted;
                                        }
                                    }
                                }
                                Rectangle {
                                    id: homeVol
                                    Layout.fillWidth: true
                                    height: 7
                                    radius: 3.5
                                    color: Qt.rgba(1, 1, 1, 0.10)
                                    Rectangle {
                                        width: Math.max(7, parent.width * Math.max(0, dash.bar?.volume ?? 0) / 100)
                                        height: parent.height
                                        radius: 3.5
                                        color: (dash.bar?.muted ?? false) ? dash.dim : dash.accent
                                        Behavior on width { NumberAnimation { duration: 80 } }
                                    }
                                    MouseArea {
                                        anchors.fill: parent
                                        anchors.margins: -6
                                        cursorShape: Qt.PointingHandCursor
                                        function setVol(mx) {
                                            if (dash.bar) dash.bar.setVolume(Math.round(
                                                Math.max(0, Math.min(1, mx / homeVol.width)) * 100));
                                        }
                                        onPressed: m => setVol(m.x - 6)
                                        onPositionChanged: m => { if (pressed) setVol(m.x - 6) }
                                    }
                                }
                                Mono {
                                    text: (dash.bar?.volume ?? -1) < 0 ? "—" : (dash.bar?.volume ?? 0) + "%"
                                    font.pixelSize: 10
                                    Layout.preferredWidth: 30
                                }
                            }
                        }
                    }
                }
            }

            // ════ SYSTEM ══════════════════════════════════════════════════
            ColumnLayout {
                spacing: 14

                component Gauge: Card {
                    id: gauge
                    property string title
                    property real value: 0
                    property string big: Math.round(value) + "%"
                    property string sub: ""
                    property color gcolor: dash.accent
                    Layout.fillWidth: true
                    Layout.preferredHeight: 164
                    onValueChanged: gCanvas.requestPaint()
                    onGcolorChanged: gCanvas.requestPaint()
                    CardTitle { text: gauge.title; x: 16; y: 14 }
                    Canvas {
                        id: gCanvas
                        anchors.centerIn: parent
                        anchors.verticalCenterOffset: 8
                        width: 102; height: 102
                        onPaint: {
                            const ctx = getContext("2d");
                            const c = width / 2, r = c - 6;
                            ctx.reset();
                            ctx.lineWidth = 8;
                            ctx.lineCap = "round";
                            ctx.beginPath();
                            ctx.arc(c, c, r, 0, Math.PI * 2);
                            ctx.strokeStyle = "rgba(255,255,255,0.07)";
                            ctx.stroke();
                            const v = Math.max(0, Math.min(1, gauge.value / 100));
                            if (v > 0.005) {
                                ctx.beginPath();
                                ctx.arc(c, c, r, -Math.PI / 2, -Math.PI / 2 + v * Math.PI * 2);
                                ctx.strokeStyle = gauge.gcolor;
                                ctx.stroke();
                            }
                        }
                        ColumnLayout {
                            anchors.centerIn: parent
                            spacing: 0
                            Mono {
                                text: gauge.big
                                font.bold: true
                                font.pixelSize: 19
                                Layout.alignment: Qt.AlignHCenter
                            }
                            Mono {
                                text: gauge.sub
                                color: dash.dim
                                font.pixelSize: 9
                                Layout.alignment: Qt.AlignHCenter
                                visible: text !== ""
                            }
                        }
                    }
                }

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 14
                    Gauge {
                        title: "CPU"
                        value: dash.bar?.cpu ?? 0
                        sub: dash.cpuGhz > 0 ? dash.cpuGhz.toFixed(2) + " GHz" : ""
                        gcolor: dash.accent
                    }
                    Gauge {
                        title: "RAM"
                        value: dash.bar?.mem ?? 0
                        sub: dash.memUsedGb.toFixed(1) + " / " + dash.memTotGb.toFixed(1) + " GB"
                        gcolor: "#b8a8f0"
                    }
                    Gauge {
                        title: "DISK /"
                        value: dash.rootDisk?.pct ?? 0
                        sub: dash.rootDisk
                             ? dash.human(dash.rootDisk.used) + " / " + dash.human(dash.rootDisk.size)
                             : ""
                        gcolor: "#f0b88a"
                    }
                    Gauge {
                        title: "LOAD"
                        value: 100 * dash.load1 / Math.max(1, dash.nproc)
                        big: dash.load1.toFixed(2)
                        sub: dash.nproc + " cores"
                        gcolor: "#8ad0f0"
                    }
                }

                RowLayout {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    spacing: 14

                    Card {
                        Layout.preferredWidth: 290
                        Layout.fillHeight: true
                        ColumnLayout {
                            anchors.fill: parent
                            anchors.margins: 16
                            spacing: 11
                            CardTitle { text: "NETWORK" }
                            RowLayout {
                                spacing: 10
                                Glyph { text: dash.bar?.netIcon ?? "󰤭"; font.pixelSize: 16; color: dash.fg }
                                Mono { text: dash.netIface || "no link"; color: dash.dim }
                            }
                            RowLayout {
                                Layout.fillWidth: true
                                spacing: 18
                                ColumnLayout {
                                    spacing: 2
                                    Mono { text: "󰕒 upload"; color: dash.dim; font.pixelSize: 10 }
                                    Mono {
                                        text: dash.netUp < 0 ? "—" : dash.human(dash.netUp) + "/s"
                                        color: dash.good
                                        font.bold: true
                                        font.pixelSize: 14
                                    }
                                }
                                ColumnLayout {
                                    spacing: 2
                                    Mono { text: "󰇚 download"; color: dash.dim; font.pixelSize: 10 }
                                    Mono {
                                        text: dash.netDown < 0 ? "—" : dash.human(dash.netDown) + "/s"
                                        color: dash.accent
                                        font.bold: true
                                        font.pixelSize: 14
                                    }
                                }
                            }
                            Rectangle { Layout.fillWidth: true; height: 1; color: dash.cardBr }
                            CardTitle { text: "MACHINE" }
                            Mono { text: "󰌽 " + dash.kernel; color: dash.dim; font.pixelSize: 11 }
                            Mono {
                                text: "󰒋 " + (dash.bar?.userName ?? "") + "@" + (dash.bar?.hostName ?? "")
                                color: dash.dim; font.pixelSize: 11
                            }
                            Mono {
                                text: "󰅐 up " + (dash.bar?.uptimeStr ?? "")
                                color: dash.dim; font.pixelSize: 11
                            }
                            Item { Layout.fillHeight: true }
                        }
                    }

                    Card {
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        ColumnLayout {
                            anchors.fill: parent
                            anchors.margins: 16
                            spacing: 10
                            CardTitle { text: "DISKS" }
                            Repeater {
                                model: dash.disks
                                ColumnLayout {
                                    id: diskRow
                                    required property var modelData
                                    Layout.fillWidth: true
                                    spacing: 4
                                    RowLayout {
                                        Layout.fillWidth: true
                                        Mono { text: diskRow.modelData.mount; font.pixelSize: 11 }
                                        Item { Layout.fillWidth: true }
                                        Mono {
                                            text: dash.human(diskRow.modelData.used) + " / "
                                                + dash.human(diskRow.modelData.size)
                                                + "  ·  " + diskRow.modelData.pct + "%"
                                            color: dash.dim
                                            font.pixelSize: 10
                                        }
                                    }
                                    Rectangle {
                                        Layout.fillWidth: true
                                        height: 6
                                        radius: 3
                                        color: Qt.rgba(1, 1, 1, 0.08)
                                        Rectangle {
                                            width: parent.width * diskRow.modelData.pct / 100
                                            height: parent.height
                                            radius: 3
                                            color: diskRow.modelData.pct > 88 ? dash.alert : dash.accent
                                        }
                                    }
                                }
                            }
                            Item { Layout.fillHeight: true }
                        }
                    }
                }
            }

            // ════ TASKS ═══════════════════════════════════════════════════
            RowLayout {
                spacing: 14

                component TaskCol: Card {
                    id: tcol
                    property string title
                    property string key
                    property color tint: dash.accent
                    property var items: []
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    ColumnLayout {
                        anchors.fill: parent
                        anchors.margins: 14
                        spacing: 10
                        RowLayout {
                            Layout.fillWidth: true
                            spacing: 8
                            Rectangle { width: 8; height: 8; radius: 4; color: tcol.tint }
                            Mono { text: tcol.title; font.bold: true }
                            Mono {
                                text: String(tcol.items.length)
                                color: dash.dim
                                font.pixelSize: 10
                            }
                            Item { Layout.fillWidth: true }
                        }
                        ListView {
                            Layout.fillWidth: true
                            Layout.fillHeight: true
                            spacing: 8
                            clip: true
                            model: tcol.items
                            delegate: Rectangle {
                                required property var modelData
                                required property int index
                                width: ListView.view.width
                                height: taskTxt.implicitHeight + 30
                                radius: 10
                                color: taskHover.hovered ? Qt.rgba(1, 1, 1, 0.08) : Qt.rgba(1, 1, 1, 0.04)
                                border.width: 1
                                border.color: dash.cardBr
                                HoverHandler { id: taskHover }
                                Mono {
                                    id: taskTxt
                                    anchors {
                                        left: parent.left; right: parent.right
                                        top: parent.top
                                        leftMargin: 12; rightMargin: 12; topMargin: 8
                                    }
                                    text: modelData
                                    font.pixelSize: 11
                                    wrapMode: Text.Wrap
                                }
                                RowLayout {
                                    anchors {
                                        right: parent.right; bottom: parent.bottom
                                        rightMargin: 10; bottomMargin: 4
                                    }
                                    spacing: 10
                                    opacity: taskHover.hovered ? 1 : 0.25
                                    Behavior on opacity { NumberAnimation { duration: 120 } }
                                    Glyph {
                                        text: "󰁍"; font.pixelSize: 11
                                        visible: tcol.key !== "todo"
                                        MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor
                                                    onClicked: dash.taskMove(tcol.key, index, -1) }
                                    }
                                    Glyph {
                                        text: tcol.key === "doing" ? "󰄬" : "󰁔"
                                        color: tcol.key === "doing" ? dash.good : dash.dim
                                        font.pixelSize: 11
                                        visible: tcol.key !== "done"
                                        MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor
                                                    onClicked: dash.taskMove(tcol.key, index, 1) }
                                    }
                                    Glyph {
                                        text: "󰅖"; font.pixelSize: 11
                                        MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor
                                                    onClicked: dash.taskDel(tcol.key, index) }
                                    }
                                }
                            }
                        }
                        Rectangle {
                            Layout.fillWidth: true
                            height: 32
                            radius: 10
                            color: Qt.rgba(1, 1, 1, 0.05)
                            border.width: 1
                            border.color: taskInput.activeFocus
                                ? Qt.rgba(dash.accent.r, dash.accent.g, dash.accent.b, 0.5) : dash.cardBr
                            TextInput {
                                id: taskInput
                                anchors.fill: parent
                                anchors.leftMargin: 12
                                anchors.rightMargin: 12
                                verticalAlignment: TextInput.AlignVCenter
                                color: dash.fg
                                font.family: dash.mono
                                font.pixelSize: 11
                                clip: true
                                onActiveFocusChanged: dash.typing = activeFocus
                                onAccepted: { dash.taskAdd(tcol.key, text); text = ""; }
                                Mono {
                                    anchors.fill: parent
                                    verticalAlignment: Text.AlignVCenter
                                    visible: taskInput.text === "" && !taskInput.activeFocus
                                    text: "+ add"
                                    color: dash.dim
                                    font.pixelSize: 11
                                }
                            }
                        }
                    }
                }

                TaskCol { title: "To Do";   key: "todo";  tint: dash.accent; items: td.todo }
                TaskCol { title: "Ongoing"; key: "doing"; tint: "#f0b88a";   items: td.doing }
                TaskCol { title: "Done";    key: "done";  tint: dash.good;   items: td.done }
            }

            // ════ WALLPAPERS ══════════════════════════════════════════════
            ColumnLayout {
                spacing: 12

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 10
                    Mono {
                        text: (dash.bar?.wallpapers?.length ?? 0) + " in ~/Pictures/Wallpapers"
                        color: dash.dim
                    }
                    Item { Layout.fillWidth: true }
                    component WallBtn: Rectangle {
                        property string glyph
                        property string label
                        property var run
                        implicitWidth: wbRow.implicitWidth + 26
                        implicitHeight: 30
                        radius: 15
                        color: wbHover.hovered ? Qt.rgba(1, 1, 1, 0.10) : Qt.rgba(1, 1, 1, 0.05)
                        Behavior on color { ColorAnimation { duration: 120 } }
                        HoverHandler { id: wbHover; cursorShape: Qt.PointingHandCursor }
                        TapHandler { onTapped: run() }
                        RowLayout {
                            id: wbRow
                            anchors.centerIn: parent
                            spacing: 7
                            Glyph { text: glyph; color: dash.accent; font.pixelSize: 12 }
                            Mono { text: label; font.pixelSize: 11 }
                        }
                    }
                    WallBtn {
                        glyph: "󰒝"; label: "Shuffle"
                        run: () => Quickshell.execDetached(["vendi-ctl", "wallpaper", "random"])
                    }
                    WallBtn {
                        glyph: "󰸌"; label: "Gradient"
                        run: () => Quickshell.execDetached(["vendi-ctl", "wallpaper", "default"])
                    }
                }

                GridView {
                    id: wallGrid
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    clip: true
                    cellWidth: Math.floor(width / 3)
                    cellHeight: Math.floor(cellWidth * 0.56)
                    model: dash.bar?.wallpapers ?? []
                    delegate: Item {
                        id: wallCell
                        required property var modelData
                        width: wallGrid.cellWidth
                        height: wallGrid.cellHeight
                        property bool current: modelData === (dash.bar?.currentWall ?? "")
                        Rectangle {
                            anchors.fill: parent
                            anchors.margins: 6
                            radius: 14
                            color: Qt.rgba(1, 1, 1, 0.04)
                            border.width: wallCell.current ? 2 : 1
                            border.color: wallCell.current ? dash.accent : dash.cardBr
                            Behavior on border.color { ColorAnimation { duration: 150 } }
                            scale: wallHover.hovered ? 1.025 : 1
                            Behavior on scale { NumberAnimation { duration: 140; easing.type: Easing.OutCubic } }
                            ClippingRectangle {
                                anchors.fill: parent
                                anchors.margins: 2
                                radius: 12
                                color: "transparent"
                                Image {
                                    anchors.fill: parent
                                    source: "file://" + wallCell.modelData
                                    fillMode: Image.PreserveAspectCrop
                                    sourceSize.width: 440
                                    asynchronous: true
                                }
                                // name plate, melting up from the bottom edge
                                Rectangle {
                                    anchors {
                                        left: parent.left; right: parent.right
                                        bottom: parent.bottom
                                    }
                                    height: 26
                                    gradient: Gradient {
                                        GradientStop { position: 0; color: "transparent" }
                                        GradientStop { position: 1; color: "#c8000000" }
                                    }
                                    Mono {
                                        anchors {
                                            left: parent.left; right: parent.right
                                            bottom: parent.bottom
                                            leftMargin: 10; rightMargin: 10; bottomMargin: 4
                                        }
                                        text: {
                                            const n = wallCell.modelData.split("/").pop();
                                            return n.replace(/\.(png|jpe?g|webp)$/i, "");
                                        }
                                        font.pixelSize: 9
                                        elide: Text.ElideRight
                                    }
                                }
                                Rectangle {
                                    visible: wallCell.current
                                    anchors { top: parent.top; right: parent.right; margins: 8 }
                                    width: 20; height: 20; radius: 10
                                    color: dash.accent
                                    Mono {
                                        anchors.centerIn: parent
                                        text: "󰄬"
                                        color: "#0b0b12"
                                        font.pixelSize: 11
                                    }
                                }
                            }
                            HoverHandler { id: wallHover; cursorShape: Qt.PointingHandCursor }
                            TapHandler {
                                onTapped: Quickshell.execDetached(
                                    ["vendi-ctl", "wallpaper", wallCell.modelData])
                            }
                        }
                    }
                }
                Mono {
                    visible: (dash.bar?.wallpapers?.length ?? 0) === 0
                    text: "drop images in ~/Pictures/Wallpapers"
                    color: dash.dim
                    Layout.alignment: Qt.AlignHCenter
                }
            }

            // ════ CONFIG ══════════════════════════════════════════════════
            RowLayout {
                spacing: 14

                ColumnLayout {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    spacing: 14

                    Card {
                        Layout.fillWidth: true
                        Layout.preferredHeight: themeCol.implicitHeight + 32
                        ColumnLayout {
                            id: themeCol
                            anchors.fill: parent
                            anchors.margins: 16
                            spacing: 10
                            CardTitle { text: "THEME" }
                            GridLayout {
                                Layout.fillWidth: true
                                columns: 3
                                rowSpacing: 8
                                columnSpacing: 8
                                Repeater {
                                    model: [
                                        { id: "mocha",   name: "Mocha",   sw: ["#1e1e2e", "#cba6f7", "#cdd6f4"] },
                                        { id: "latte",   name: "Latte",   sw: ["#eff1f5", "#8839ef", "#4c4f69"] },
                                        { id: "gruvbox", name: "Gruvbox", sw: ["#282828", "#d79921", "#ebdbb2"] },
                                        { id: "mono",    name: "Mono",    sw: ["#000000", "#ffffff", "#888888"] },
                                        { id: "think",   name: "Think",   sw: ["#000000", "#e22128", "#e6e6e6"] },
                                        { id: "dynamic", name: "Dynamic", sw: [] },
                                    ]
                                    Rectangle {
                                        id: themeCard
                                        required property var modelData
                                        property bool current: dash.themeNow === modelData.id
                                        Layout.fillWidth: true
                                        implicitHeight: 52
                                        radius: 12
                                        color: current ? Qt.rgba(dash.accent.r, dash.accent.g, dash.accent.b, 0.16)
                                             : thHover.hovered ? Qt.rgba(1, 1, 1, 0.08) : Qt.rgba(1, 1, 1, 0.04)
                                        Behavior on color { ColorAnimation { duration: 120 } }
                                        border.width: 1
                                        border.color: current
                                            ? Qt.rgba(dash.accent.r, dash.accent.g, dash.accent.b, 0.5) : dash.cardBr
                                        HoverHandler { id: thHover; cursorShape: Qt.PointingHandCursor }
                                        TapHandler {
                                            onTapped: Quickshell.execDetached(["vendi", "theme", themeCard.modelData.id])
                                        }
                                        ColumnLayout {
                                            anchors.centerIn: parent
                                            spacing: 5
                                            RowLayout {
                                                Layout.alignment: Qt.AlignHCenter
                                                spacing: 4
                                                visible: themeCard.modelData.sw.length > 0
                                                Repeater {
                                                    model: themeCard.modelData.sw
                                                    Rectangle {
                                                        required property var modelData
                                                        width: 12; height: 12; radius: 6
                                                        color: modelData
                                                        border.width: 1
                                                        border.color: Qt.rgba(1, 1, 1, 0.18)
                                                    }
                                                }
                                            }
                                            Glyph {
                                                visible: themeCard.modelData.sw.length === 0
                                                text: "󰸉"
                                                color: dash.accent
                                                font.pixelSize: 13
                                                Layout.alignment: Qt.AlignHCenter
                                            }
                                            Mono {
                                                text: themeCard.modelData.name
                                                font.pixelSize: 10
                                                Layout.alignment: Qt.AlignHCenter
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    Card {
                        Layout.fillWidth: true
                        Layout.preferredHeight: barCol.implicitHeight + 32
                        ColumnLayout {
                            id: barCol
                            anchors.fill: parent
                            anchors.margins: 16
                            spacing: 10
                            CardTitle { text: "BAR" }
                            RowLayout {
                                Layout.fillWidth: true
                                spacing: 8
                                Repeater {
                                    model: [
                                        { id: "classic", name: "Classic", desc: "floating · minimal" },
                                        { id: "pro",     name: "Pro",     desc: "dynamic island" },
                                    ]
                                    Rectangle {
                                        id: barCard
                                        required property var modelData
                                        property bool current: dash.barNow === modelData.id
                                        Layout.fillWidth: true
                                        implicitHeight: 50
                                        radius: 12
                                        color: current ? Qt.rgba(dash.accent.r, dash.accent.g, dash.accent.b, 0.16)
                                             : barHover.hovered ? Qt.rgba(1, 1, 1, 0.08) : Qt.rgba(1, 1, 1, 0.04)
                                        border.width: 1
                                        border.color: current
                                            ? Qt.rgba(dash.accent.r, dash.accent.g, dash.accent.b, 0.5) : dash.cardBr
                                        HoverHandler { id: barHover; cursorShape: Qt.PointingHandCursor }
                                        TapHandler {
                                            onTapped: Quickshell.execDetached(["vendi", "bar", barCard.modelData.id])
                                        }
                                        ColumnLayout {
                                            anchors.centerIn: parent
                                            spacing: 2
                                            Mono {
                                                text: barCard.modelData.name
                                                font.bold: true
                                                font.pixelSize: 11
                                                Layout.alignment: Qt.AlignHCenter
                                            }
                                            Mono {
                                                text: barCard.modelData.desc
                                                color: dash.dim
                                                font.pixelSize: 9
                                                Layout.alignment: Qt.AlignHCenter
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    Card {
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        ColumnLayout {
                            anchors.fill: parent
                            anchors.margins: 16
                            spacing: 8
                            CardTitle { text: "SYSTEM" }
                            component CfgRow: Rectangle {
                                property string glyph
                                property string label
                                property var run
                                Layout.fillWidth: true
                                implicitHeight: 34
                                radius: 10
                                color: cfgHover.hovered ? Qt.rgba(1, 1, 1, 0.08) : Qt.rgba(1, 1, 1, 0.04)
                                Behavior on color { ColorAnimation { duration: 120 } }
                                HoverHandler { id: cfgHover; cursorShape: Qt.PointingHandCursor }
                                TapHandler { onTapped: run() }
                                RowLayout {
                                    anchors.fill: parent
                                    anchors.leftMargin: 12
                                    spacing: 10
                                    Glyph { text: parent.parent.glyph; color: dash.accent; font.pixelSize: 13 }
                                    Mono { text: parent.parent.label; font.pixelSize: 11 }
                                    Item { Layout.fillWidth: true }
                                }
                            }
                            CfgRow {
                                glyph: "󰈔"; label: "Edit wm config"
                                run: () => {
                                    dash.requestClose();
                                    Quickshell.execDetached(["sh", "-c",
                                        "alacritty -e \"${EDITOR:-nano}\" \"$HOME/.config/vendi/config\""]);
                                }
                            }
                            CfgRow {
                                glyph: "󰑓"; label: "Reload wm config"
                                run: () => Quickshell.execDetached(["vendi-ctl", "reload"])
                            }
                            CfgRow {
                                glyph: "󰕾"; label: "Audio settings"
                                run: () => {
                                    dash.requestClose();
                                    Quickshell.execDetached(["alacritty", "--class", "vendi-float", "-e", "vendi", "audio"]);
                                }
                            }
                            Item { Layout.fillHeight: true }
                        }
                    }
                }

                Card {
                    Layout.preferredWidth: 330
                    Layout.fillHeight: true
                    ColumnLayout {
                        anchors.fill: parent
                        anchors.margins: 16
                        spacing: 10
                        CardTitle { text: "KEYBINDS" }
                        ListView {
                            Layout.fillWidth: true
                            Layout.fillHeight: true
                            clip: true
                            spacing: 5
                            model: [
                                { k: "Super + Enter",          a: "Terminal" },
                                { k: "Super + Space",          a: "Spotlight search" },
                                { k: "Super + Alt + Space",    a: "Actions menu" },
                                { k: "Super + D",              a: "Dashboard" },
                                { k: "Super + B",              a: "Browser" },
                                { k: "Super + Q",              a: "Close window" },
                                { k: "Super + F",              a: "Fullscreen" },
                                { k: "Super + H / V",          a: "Split direction" },
                                { k: "Super + Arrows",         a: "Focus direction" },
                                { k: "Super + Shift + Arrows", a: "Move window" },
                                { k: "Super + Ctrl + Arrows",  a: "Resize window" },
                                { k: "Super + 1–9",            a: "Go to workspace" },
                                { k: "Super + Shift + 1–9",    a: "Move to workspace" },
                                { k: "Super + Shift + Space",  a: "Toggle floating" },
                                { k: "Super + O",              a: "Overview" },
                                { k: "Super + Escape",         a: "Lock screen" },
                                { k: "Super + K",              a: "All keybinds" },
                            ]
                            delegate: RowLayout {
                                required property var modelData
                                width: ListView.view.width
                                spacing: 10
                                Rectangle {
                                    implicitWidth: kbKey.implicitWidth + 16
                                    implicitHeight: 22
                                    radius: 6
                                    color: Qt.rgba(1, 1, 1, 0.07)
                                    border.width: 1
                                    border.color: dash.cardBr
                                    Mono {
                                        id: kbKey
                                        anchors.centerIn: parent
                                        text: modelData.k
                                        font.pixelSize: 9
                                        color: dash.accent
                                    }
                                }
                                Mono {
                                    Layout.fillWidth: true
                                    text: modelData.a
                                    color: dash.dim
                                    font.pixelSize: 10
                                    elide: Text.ElideRight
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
