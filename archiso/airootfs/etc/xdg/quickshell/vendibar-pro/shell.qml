// vendibar Pro — dynamic notch bar for vendiOS (quickshell/QML).
//
// One silhouette hugging the top edge: a thin strip with three notches
// flowing out of it. The notches are alive:
//   center — click the clock: dashboard (big clock, weather, calendar,
//            wallpaper picker, media with album art)
//   right  — click the stats: control center (volume, system, notification
//            history, quick actions). Notifications toast out of this notch
//            (vendibar-pro IS the notification daemon), and external volume
//            changes bulge it into a transient OSD.
//
// Native quickshell services: Pipewire (live volume), UPower (battery),
// Mpris (media), Notifications (org.freedesktop.Notifications), SystemTray.
// Theme accent follows ~/.config/vendi/theme-state live; compositor state
// over vendi-ctl.            Run: quickshell -c vendibar-pro
//
import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import Quickshell.Widgets
import Quickshell.Services.Pipewire
import Quickshell.Services.UPower
import Quickshell.Services.Mpris
import Quickshell.Services.Notifications
import Quickshell.Services.SystemTray
import QtQuick
import QtQuick.Layouts

ShellRoot {
    id: root

    // ── theme ────────────────────────────────────────────────────────────────
    property color accent: "#cba6f7"
    property color panel:  Qt.rgba(0.043, 0.043, 0.071, 0.96)   // #0b0b12
    property color fg:     "#cdd6f4"
    property color dim:    "#717189"
    property color alert:  "#f38ba8"
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

    // ── cpu / mem (proc files — no daemon needed) ────────────────────────────
    property real cpu: 0
    property real mem: 0
    property var cpuPrev: null

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
        onTriggered: { procStat.reload(); memInfo.reload(); }
    }

    // ── audio (pipewire, live — no polling) ──────────────────────────────────
    PwObjectTracker { objects: [Pipewire.defaultAudioSink] }
    property var sinkAudio: Pipewire.defaultAudioSink?.audio ?? null
    property int volume: sinkAudio ? Math.round(sinkAudio.volume * 100) : -1
    property bool muted: sinkAudio?.muted ?? false
    function setVolume(pct) {
        if (!sinkAudio) return;
        sinkAudio.muted = false;
        sinkAudio.volume = Math.max(0, Math.min(1, pct / 100));
    }

    // volume OSD: external changes bulge the right notch for a moment.
    // Armed late so the initial pipewire binding doesn't flash it at startup.
    property bool osdShow: false
    property bool osdArmed: false
    Timer { interval: 4000; running: true; onTriggered: root.osdArmed = true }
    Timer { id: osdTimer; interval: 1400; onTriggered: root.osdShow = false }
    Connections {
        target: root.sinkAudio
        function onVolumeChanged() { root.pokeOsd() }
        function onMutedChanged()  { root.pokeOsd() }
    }
    function pokeOsd() {
        if (!osdArmed) return;
        osdShow = true;
        osdTimer.restart();
    }

    // ── battery (upower) ─────────────────────────────────────────────────────
    property var batDev: UPower.displayDevice
    property bool hasBattery: (batDev?.isLaptopBattery ?? false)
    property int battery: {
        const p = batDev?.percentage ?? 0;
        return Math.round(p <= 1 ? p * 100 : p);
    }
    property bool charging: batDev
        ? batDev.state === UPowerDeviceState.Charging
        : false

    // ── network ──────────────────────────────────────────────────────────────
    property string netIcon: "󰤭"
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
    Timer {
        interval: 8000; running: true; repeat: true; triggeredOnStart: true
        onTriggered: netProc.running = true
    }

    // ── media (mpris) ────────────────────────────────────────────────────────
    property var player: null
    property bool musicPlaying: player
        ? player.playbackState === MprisPlaybackState.Playing
        : false
    property string musicTrack: {
        if (!player) return "";
        const artist = player.trackArtist || "";
        const title = player.trackTitle || "";
        return artist && title ? artist + " — " + title : (title || artist);
    }
    property real musicProgress: 0
    function pickPlayer() {
        const all = Mpris.players.values;
        return all.find(p => p.playbackState === MprisPlaybackState.Playing) ?? all[0] ?? null;
    }

    // ── weather (wttr.in) ────────────────────────────────────────────────────
    property string weather: ""
    Process {
        id: wxProc
        command: ["sh", "-c", "curl -sf --max-time 6 'https://wttr.in/?format=%c+%t' | head -c 64"]
        stdout: SplitParser {
            onRead: l => {
                const w = l.trim().replace(/\s+/g, " ");
                if (w && !w.includes("Unknown")) root.weather = w;
            }
        }
    }
    Timer {
        interval: 1800000; running: true; repeat: true; triggeredOnStart: true
        onTriggered: wxProc.running = true
    }
    Timer {   // retry fast until the first fix lands (boot races the network)
        interval: 90000; running: root.weather === ""; repeat: true
        onTriggered: wxProc.running = true
    }

    // ── wallpapers (~/Pictures/Wallpapers) ───────────────────────────────────
    property var wallpapers: []
    property string currentWall: ""
    Process {
        id: wpList
        command: ["sh", "-c",
            "ls -1 \"$HOME\"/Pictures/Wallpapers/*.png \"$HOME\"/Pictures/Wallpapers/*.jpg " +
            "\"$HOME\"/Pictures/Wallpapers/*.jpeg \"$HOME\"/Pictures/Wallpapers/*.webp 2>/dev/null"]
        running: true
        property var acc: []
        stdout: SplitParser { onRead: l => { if (l.trim()) wpList.acc.push(l.trim()); } }
        onStarted: acc = []
        onExited: { root.wallpapers = acc; acc = []; }
    }
    FileView {
        path: Quickshell.env("HOME") + "/.config/vendi/wallpaper"
        watchChanges: true
        onLoaded: root.currentWall = text().trim()
        onFileChanged: reload()
    }

    // ── notifications (we ARE the daemon) ────────────────────────────────────
    // toasts: live queue shown in the right notch (newest first served).
    // notifHistory: plain snapshots for the control center (safe after the
    // client withdraws the notification).
    property var toasts: []
    property var notifHistory: []

    NotificationServer {
        id: notifServer
        bodySupported: true
        actionsSupported: true
        imageSupported: true
        onNotification: notif => {
            notif.tracked = true;
            const t = {
                app:     notif.appName || "notification",
                summary: notif.summary || "",
                body:    (notif.body || "").replace(/<[^>]*>/g, ""),
                icon:    notif.appIcon || "",
                image:   notif.image || "",
                n:       notif,
            };
            root.toasts = root.toasts.concat([t]);
            toastTimer.restart();
            const when = Qt.formatDateTime(new Date(), "HH:mm");
            root.notifHistory = [{ app: t.app, summary: t.summary, when: when }]
                .concat(root.notifHistory).slice(0, 30);
        }
    }
    Timer {
        id: toastTimer
        interval: 5500
        repeat: true
        running: root.toasts.length > 0
        onTriggered: root.shiftToast(true)
    }
    function shiftToast(expire) {
        if (!toasts.length) return;
        const t = toasts[0];
        try { expire ? t.n.expire() : t.n.dismiss(); } catch (e) {}
        toasts = toasts.slice(1);
    }

    // ── 1s heartbeat: clocks, media progress, active player ─────────────────
    Timer {
        interval: 1000; running: true; repeat: true; triggeredOnStart: true
        onTriggered: {
            root.player = root.pickPlayer();
            root.musicProgress = (root.player && root.player.length > 0)
                ? Math.max(0, Math.min(1, root.player.position / root.player.length)) : 0;
        }
    }

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

            // right notch mode: control wins, then toasts, then volume OSD
            readonly property string rightMode:
                rightOpen ? "control"
                : root.toasts.length > 0 ? "toast"
                : root.osdShow ? "osd"
                : "idle"

            // notch dimensions, all springy. Idle notches grow a hair on
            // hover — the island invites the click.
            property real lw: leftRow.implicitWidth + root.pad * 2
            property real cw: centerOpen ? 480
                : centerRow.implicitWidth + root.pad * 2 + (centerHover.hovered ? 10 : 0)
            property real rw: rightMode === "control" ? 400
                : rightMode === "toast" ? 380
                : rightMode === "osd" ? 270
                : rightRow.implicitWidth + root.pad * 2 + (rightHover.hovered ? 10 : 0)
            property real ch: centerOpen ? 462 : root.barH
            property real rh: rightMode === "control"
                    ? 312 + (root.notifHistory.length > 0
                             ? 30 + Math.min(root.notifHistory.length, 3) * 22 : 0)
                : rightMode === "toast"
                    ? Math.max(root.barH, toastCol.implicitHeight + root.stripH + 26)
                : root.barH
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

            // ── center notch collapsed row: clock · date · weather ──────────
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
                Sep { visible: root.weather !== "" }
                Mono { text: root.weather; visible: root.weather !== ""; color: root.dim }
                TapHandler { onTapped: panelWin.toggleCenter() }
                HoverHandler { id: centerHover; cursorShape: Qt.PointingHandCursor }
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
                    function onCenterOpenChanged() {
                        if (panelWin.centerOpen) {
                            dashboard.monthOff = 0;
                            wpList.running = true;   // rescan the library
                        }
                    }
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
                        Mono { text: root.weather; visible: root.weather !== ""; color: root.dim }
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

                    // wallpaper picker — the library at ~/Pictures/Wallpapers
                    RowLayout {
                        Layout.fillWidth: true
                        Mono { text: "Wallpapers"; font.bold: true; color: root.accent }
                        Item { Layout.fillWidth: true }
                        Glyph {
                            text: "󰒝"; font.pixelSize: 14
                            visible: root.wallpapers.length > 1
                            MouseArea {
                                anchors.fill: parent
                                cursorShape: Qt.PointingHandCursor
                                onClicked: Quickshell.execDetached(["vendi-ctl", "wallpaper", "random"])
                            }
                        }
                    }
                    ListView {
                        Layout.fillWidth: true
                        Layout.preferredHeight: 56
                        orientation: ListView.Horizontal
                        spacing: 8
                        clip: true
                        visible: root.wallpapers.length > 0
                        model: root.wallpapers
                        delegate: Item {
                            id: wpThumb
                            required property var modelData
                            width: 96
                            height: 54
                            ClippingRectangle {
                                anchors.fill: parent
                                radius: 9
                                color: Qt.rgba(1, 1, 1, 0.05)
                                Image {
                                    anchors.fill: parent
                                    source: "file://" + wpThumb.modelData
                                    fillMode: Image.PreserveAspectCrop
                                    sourceSize.width: 192
                                    asynchronous: true
                                }
                            }
                            Rectangle {
                                anchors.fill: parent
                                radius: 9
                                color: "transparent"
                                border.width: 2
                                border.color: wpThumb.modelData === root.currentWall
                                    ? root.accent : Qt.rgba(1, 1, 1, 0.10)
                                Behavior on border.color { ColorAnimation { duration: 150 } }
                            }
                            TapHandler {
                                onTapped: Quickshell.execDetached(
                                    ["vendi-ctl", "wallpaper", wpThumb.modelData])
                            }
                            HoverHandler { cursorShape: Qt.PointingHandCursor }
                        }
                    }
                    Mono {
                        visible: root.wallpapers.length === 0
                        text: "drop images in ~/Pictures/Wallpapers"
                        color: root.dim
                    }

                    Rectangle { Layout.fillWidth: true; height: 1; color: Qt.rgba(1,1,1,0.08) }

                    // media — album art, track, controls, progress
                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 12
                        ClippingRectangle {
                            Layout.preferredWidth: 40
                            Layout.preferredHeight: 40
                            radius: 8
                            color: Qt.rgba(1, 1, 1, 0.05)
                            visible: (root.player?.trackArtUrl ?? "") !== ""
                            Image {
                                anchors.fill: parent
                                source: root.player?.trackArtUrl ?? ""
                                fillMode: Image.PreserveAspectCrop
                                sourceSize.width: 80
                                asynchronous: true
                            }
                        }
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 3
                            Mono {
                                text: root.musicTrack !== ""
                                      ? (root.musicTrack.length > 34 ? root.musicTrack.slice(0, 34) + "…" : root.musicTrack)
                                      : "nothing playing"
                                color: root.musicTrack !== "" ? root.fg : root.dim
                            }
                            Rectangle {
                                Layout.fillWidth: true
                                height: 3
                                radius: 1.5
                                color: Qt.rgba(1, 1, 1, 0.10)
                                visible: root.musicTrack !== ""
                                Rectangle {
                                    width: parent.width * root.musicProgress
                                    height: parent.height
                                    radius: 1.5
                                    color: root.accent
                                    Behavior on width { NumberAnimation { duration: 500 } }
                                }
                            }
                        }
                        Glyph {
                            text: "󰒮"; font.pixelSize: 15
                            MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor
                                        onClicked: root.player?.previous() }
                        }
                        Glyph {
                            text: root.musicPlaying ? "󰏤" : "󰐊"
                            color: root.accent; font.pixelSize: 16
                            MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor
                                        onClicked: root.player?.togglePlaying() }
                        }
                        Glyph {
                            text: "󰒭"; font.pixelSize: 15
                            MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor
                                        onClicked: root.player?.next() }
                        }
                    }

                    Item { Layout.fillHeight: true }
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
                opacity: panelWin.rightMode === "idle" ? 1 : 0
                Behavior on opacity { NumberAnimation { duration: 140 } }
                HoverHandler { id: rightHover }

                RowLayout {
                    spacing: 8
                    visible: root.musicTrack !== ""
                    Mono {
                        text: root.musicPlaying ? "󰐊" : "󰏤"
                        color: root.accent
                        font.pixelSize: 13
                    }
                    Mono {
                        text: root.musicTrack.length > 26 ? root.musicTrack.slice(0, 26) + "…" : root.musicTrack
                        color: root.dim
                    }
                    TapHandler { onTapped: root.player?.togglePlaying() }
                }
                Sep { visible: root.musicTrack !== "" }

                // system tray (icons only; click = activate)
                RowLayout {
                    spacing: 8
                    visible: SystemTray.items.values.length > 0
                    Repeater {
                        model: SystemTray.items
                        IconImage {
                            required property var modelData
                            implicitSize: 16
                            source: modelData.icon
                            Layout.alignment: Qt.AlignVCenter
                            TapHandler { onTapped: modelData.activate() }
                            HoverHandler { cursorShape: Qt.PointingHandCursor }
                        }
                    }
                }
                Sep { visible: SystemTray.items.values.length > 0 }

                RowLayout {
                    spacing: 6
                    Glyph { text: "󰻠"; color: root.cpu > 85 ? root.alert : root.dim }
                    Mono { text: Math.round(root.cpu) + "%" }
                    Glyph { text: "󰍛"; color: root.mem > 85 ? root.alert : root.dim }
                    Mono { text: Math.round(root.mem) + "%" }
                    Glyph { text: root.netIcon }
                    Mono {
                        text: (root.muted ? "󰝟" : "󰕾") + " " + (root.volume < 0 ? "—" : root.volume + "%")
                        color: root.muted ? root.dim : root.fg
                    }
                    Mono {
                        visible: root.hasBattery
                        text: (root.charging ? "󰂄" : "󰁾") + " " + root.battery + "%"
                        color: root.battery <= 20 && !root.charging ? root.alert : root.fg
                    }
                    Mono {
                        visible: root.notifHistory.length > 0
                        text: "󰂚 " + root.notifHistory.length
                        color: root.dim
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

            // ── volume OSD (transient bulge of the right notch) ─────────────
            RowLayout {
                id: osdRow
                anchors.right: parent.right
                anchors.rightMargin: root.pad
                y: root.stripH
                height: root.barH - root.stripH
                spacing: 10
                visible: opacity > 0
                opacity: panelWin.rightMode === "osd" ? 1 : 0
                Behavior on opacity { NumberAnimation { duration: 140 } }

                Glyph {
                    text: root.muted ? "󰝟" : root.volume > 60 ? "󰕾" : root.volume > 20 ? "󰖀" : "󰕿"
                    color: root.muted ? root.dim : root.accent
                    font.pixelSize: 15
                }
                Rectangle {
                    Layout.preferredWidth: 150
                    height: 6
                    radius: 3
                    color: Qt.rgba(1, 1, 1, 0.10)
                    Rectangle {
                        width: parent.width * Math.max(0, root.volume) / 100
                        height: parent.height
                        radius: 3
                        color: root.muted ? root.dim : root.accent
                        Behavior on width { NumberAnimation { duration: 100 } }
                    }
                }
                Mono { text: root.muted ? "muted" : root.volume + "%"; Layout.preferredWidth: 44 }
            }

            // ── notification toast (right notch swells around it) ───────────
            Item {
                id: toastBox
                x: panelWin.width - panelWin.rw
                y: root.stripH
                width: panelWin.rw
                height: panelWin.rh - root.stripH
                clip: true
                visible: opacity > 0
                opacity: panelWin.rightMode === "toast" ? 1 : 0
                Behavior on opacity { NumberAnimation { duration: 160 } }
                property var t: root.toasts.length > 0 ? root.toasts[0] : null

                ColumnLayout {
                    id: toastCol
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.top: parent.top
                    anchors.margins: 14
                    spacing: 6

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 10
                        IconImage {
                            visible: (toastBox.t?.icon ?? "") !== "" || (toastBox.t?.image ?? "") !== ""
                            implicitSize: 22
                            source: toastBox.t
                                ? (toastBox.t.image !== "" ? toastBox.t.image
                                   : Quickshell.iconPath(toastBox.t.icon, true))
                                : ""
                        }
                        Glyph {
                            visible: (toastBox.t?.icon ?? "") === "" && (toastBox.t?.image ?? "") === ""
                            text: "󰂚"; color: root.accent; font.pixelSize: 15
                        }
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 1
                            Mono {
                                Layout.fillWidth: true
                                text: toastBox.t?.summary ?? ""
                                font.bold: true
                                elide: Text.ElideRight
                            }
                            Mono {
                                text: toastBox.t?.app ?? ""
                                color: root.dim
                                font.pixelSize: 10
                            }
                        }
                        Mono {
                            visible: root.toasts.length > 1
                            text: "+" + (root.toasts.length - 1)
                            color: root.accent
                            font.pixelSize: 11
                        }
                        Mono {
                            text: "󰅖"
                            color: root.dim
                            MouseArea {
                                anchors.fill: parent
                                cursorShape: Qt.PointingHandCursor
                                onClicked: root.shiftToast(false)
                            }
                        }
                    }
                    Mono {
                        Layout.fillWidth: true
                        visible: (toastBox.t?.body ?? "") !== ""
                        text: toastBox.t?.body ?? ""
                        color: root.dim
                        wrapMode: Text.Wrap
                        maximumLineCount: 2
                        elide: Text.ElideRight
                    }
                    RowLayout {
                        visible: (toastBox.t?.n?.actions?.length ?? 0) > 0
                        spacing: 8
                        Repeater {
                            model: toastBox.t?.n?.actions ?? []
                            Rectangle {
                                required property var modelData
                                implicitWidth: actionLbl.implicitWidth + 20
                                implicitHeight: 22
                                radius: 11
                                color: actHover.hovered ? Qt.rgba(1, 1, 1, 0.14) : Qt.rgba(1, 1, 1, 0.07)
                                HoverHandler { id: actHover; cursorShape: Qt.PointingHandCursor }
                                Mono {
                                    id: actionLbl
                                    anchors.centerIn: parent
                                    text: parent.modelData.text || "open"
                                    font.pixelSize: 10
                                }
                                TapHandler {
                                    onTapped: {
                                        try { parent.modelData.invoke(); } catch (e) {}
                                        root.toasts = root.toasts.slice(1);
                                    }
                                }
                            }
                        }
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

                    // volume slider — writes straight to the pipewire node
                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 10
                        Glyph {
                            text: root.muted ? "󰝟" : "󰕾"
                            color: root.muted ? root.dim : root.fg
                            MouseArea {
                                anchors.fill: parent
                                cursorShape: Qt.PointingHandCursor
                                onClicked: { if (root.sinkAudio) root.sinkAudio.muted = !root.sinkAudio.muted; }
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
                                    root.setVolume(Math.round(
                                        Math.max(0, Math.min(1, mx / volTrack.width)) * 100));
                                }
                                onPressed: m => setVol(m.x - 6)
                                onPositionChanged: m => { if (pressed) setVol(m.x - 6) }
                            }
                        }
                        Mono { text: (root.volume < 0 ? "—" : root.volume + "%"); Layout.preferredWidth: 38 }
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
                                    color: root.cpu > 85 ? root.alert : root.accent
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
                                    color: root.mem > 85 ? root.alert : root.accent
                                    Behavior on width { NumberAnimation { duration: 300 } }
                                }
                            }
                            Mono { text: Math.round(root.mem) + "%"; Layout.preferredWidth: 38 }
                        }
                    }

                    // notification history
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 4
                        visible: root.notifHistory.length > 0
                        RowLayout {
                            Layout.fillWidth: true
                            Mono { text: "Notifications"; font.bold: true; color: root.accent; font.pixelSize: 11 }
                            Item { Layout.fillWidth: true }
                            Mono {
                                text: "clear"
                                color: root.dim
                                font.pixelSize: 10
                                MouseArea {
                                    anchors.fill: parent
                                    cursorShape: Qt.PointingHandCursor
                                    onClicked: root.notifHistory = []
                                }
                            }
                        }
                        Repeater {
                            model: root.notifHistory.slice(0, 3)
                            RowLayout {
                                required property var modelData
                                Layout.fillWidth: true
                                spacing: 8
                                Mono { text: modelData.when; color: root.dim; font.pixelSize: 10 }
                                Mono {
                                    Layout.fillWidth: true
                                    text: modelData.app + " · " + modelData.summary
                                    color: root.fg
                                    font.pixelSize: 11
                                    elide: Text.ElideRight
                                }
                            }
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
