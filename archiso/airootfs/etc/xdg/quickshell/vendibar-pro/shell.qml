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
import QtQuick.Effects

ShellRoot {
    id: root

    // Lock-screen handshake (vendilock drives this over IPC):
    //   hide   — left/right notches melt into the strip (animated); the
    //            center notch stays put, ready to become the lock blob.
    //   vanish — everything disappears instantly; the lock surface draws
    //            its blob exactly where the center notch was (seamless swap).
    //   restore — chrome returns, gliding down from the top edge.
    property bool modulesHidden: false
    property real centerW: 236
    property bool chromeGone: false
    signal chromeReturn()
    // Spotlight search — not a window: the center notch morphs into it
    // (Launcher.qml fills the expanded notch; panelWin drives the state).
    signal searchToggle(string mode)
    IpcHandler {
        target: "launcher"
        function toggle(): void { root.searchToggle("search"); }
        function actions(): void { root.searchToggle("actions"); }
        function clip(): void { root.searchToggle("clip"); }
    }
    // Dashboard (the expanded center notch) — super+d via vendi-launcher dash.
    signal dashToggle()
    signal dashOpen(int tab)
    // Now-playing card (compact center notch) — album click, or a keybind.
    signal mediaToggle()
    // Control center (right notch) — opened by the compositor's top-edge swipe.
    signal controlToggle()
    signal controlGoto(string page)
    IpcHandler {
        target: "dash"
        function toggle(): void { root.dashToggle(); }
        function open(tab: int): void { root.dashOpen(tab); }
        function media(): void { root.mediaToggle(); }
    }
    // vendi AI — super+a expands the center notch into the Siri panel.
    signal aiToggle()
    signal aiSet(bool on)
    IpcHandler {
        target: "ai"
        function toggle(): void { root.aiToggle(); }
        function open(): void { root.aiSet(true); }
        function close(): void { root.aiSet(false); }
    }
    // Tools tab state (focus timer, notes, calculator) — one copy for every
    // screen's dashboard; the running focus timer also shows on the island.
    ToolState { id: toolState }
    readonly property var tools: toolState
    // `quickshell -c vendibar-pro ipc call focus toggle` — start/pause the
    // focus timer from a keybind or script.
    IpcHandler {
        target: "focus"
        function toggle(): void { toolState.focusToggle(); }
        function reset(): void { toolState.focusReset(); }
        function skip(): void { toolState.focusAdvance(false); }
    }

    // ── downloads — a pill on the right island while a browser downloads ────
    // Browsers don't publish progress anywhere a bar can read, but they all
    // write into a temp file next to the target (Firefox/yt-dlp `.part`,
    // Chromium `.crdownload`). Watch ~/Downloads for those: live size + rate
    // while they grow, then a "done" flash with the finished file (click it
    // to open). No total → no fake percentage.
    property string dlDir: ""
    property var dlFiles: []               // [{name, size}] in-flight temp files
    property real dlBytes: 0
    property real dlRate: 0                // bytes/s, smoothed
    property var dlPrev: null              // {t, bytes, names}
    property string dlDoneName: ""         // flashes for a few seconds when one lands
    property string dlDonePath: ""
    property bool dlDoneShow: false
    readonly property bool dlActive: dlFiles.length > 0
    function dlShort(n) {
        n = n.replace(/\.(part|crdownload)$/, "");
        return n.length > 22 ? n.slice(0, 13) + "…" + n.slice(-8) : n;
    }
    function dlHuman(b) {
        const u = ["B", "K", "M", "G"];
        let i = 0;
        while (b >= 1024 && i < 3) { b /= 1024; i++; }
        return b.toFixed(b >= 10 || i === 0 ? 0 : 1) + u[i];
    }
    Process {
        id: dlPoll
        // line 1: the downloads dir; line 2: newest regular file (for Chromium,
        // whose temp name has nothing to do with the final one); then temp files.
        command: ["sh", "-c",
            "d=$(xdg-user-dir DOWNLOAD 2>/dev/null); [ -d \"$d\" ] || d=\"$HOME/Downloads\"; echo \"$d\"; "
          + "[ -d \"$d\" ] || exit 0; "
          + "ls -t \"$d\" 2>/dev/null | grep -vE '\\.(part|crdownload)$' | head -1; "
          + "find \"$d\" -maxdepth 1 -type f \\( -name '*.part' -o -name '*.crdownload' \\) -printf '%s\\t%f\\n'"]
        stdout: StdioCollector {
            onStreamFinished: {
                const l = text.split("\n");
                root.dlDir = l[0] || "";
                const newest = l[1] || "";
                const files = [];
                let bytes = 0;
                for (const line of l.slice(2)) {
                    const tab = line.indexOf("\t");
                    if (tab < 0) continue;
                    const size = parseFloat(line.slice(0, tab)) || 0;
                    files.push({ name: line.slice(tab + 1), size: size });
                    bytes += size;
                }
                const now = Date.now();
                const prev = root.dlPrev;
                if (prev && files.length > 0) {
                    const dt = (now - prev.t) / 1000;
                    const inst = Math.max(0, bytes - prev.bytes) / Math.max(dt, 0.2);
                    root.dlRate = root.dlRate > 0 ? root.dlRate * 0.6 + inst * 0.4 : inst;
                } else if (files.length === 0) root.dlRate = 0;
                // a temp file vanished → that download finished (or was cancelled:
                // then there's no finished file, so nothing flashes)
                if (prev) {
                    const names = files.map(f => f.name);
                    for (const gone of prev.names.filter(n => names.indexOf(n) < 0)) {
                        const fin = gone.replace(/\.(part|crdownload)$/, "");
                        const pick = gone.endsWith(".part") ? fin : newest;
                        root.dlDoneShow = false;
                        dlDoneTimer.stop();
                        if (pick) {
                            root.dlDoneName = pick;
                            root.dlDonePath = root.dlDir + "/" + pick;
                            dlDoneCheck.running = true;
                        }
                    }
                }
                root.dlFiles = files;
                root.dlBytes = bytes;
                root.dlPrev = { t: now, bytes: bytes, names: files.map(f => f.name) };
            }
        }
    }
    // confirm the finished file really exists and isn't a 0-byte placeholder
    // (a cancelled Firefox download leaves one behind) before flashing it
    Process {
        id: dlDoneCheck
        command: ["sh", "-c", "sleep 0.4; [ -s \"$1\" ] && echo ok", "_", root.dlDonePath]
        stdout: StdioCollector {
            onStreamFinished: {
                if (text.trim() === "ok") { root.dlDoneShow = true; dlDoneTimer.restart(); }
            }
        }
    }
    Timer {
        id: dlDoneTimer
        interval: 6000
        onTriggered: root.dlDoneShow = false
    }
    Timer {
        // quick while something's downloading, lazy otherwise
        interval: root.dlActive ? 1000 : 2500
        running: true
        repeat: true
        triggeredOnStart: true
        onTriggered: dlPoll.running = true
    }

    function rescanWallpapers() { wpList.running = true; }

    // ── screen recording (wf-recorder) — red pill in the collapsed notch ────
    property bool hasRecorder: false
    Process {
        command: ["sh", "-c", "command -v wf-recorder >/dev/null && echo yes || echo no"]
        running: true
        stdout: SplitParser { onRead: l => root.hasRecorder = l.trim() === "yes" }
    }
    property bool recording: false
    property double recStart: 0
    property int recSecs: 0
    function startRecord() {
        Quickshell.execDetached(["sh", "-c",
            "mkdir -p \"$HOME/Videos\"; exec wf-recorder -f \"$HOME/Videos/rec-$(date +%Y%m%d-%H%M%S).mp4\""]);
        recording = true;
        recStart = Date.now();
        recSecs = 0;
    }
    function stopRecord() {
        Quickshell.execDetached(["pkill", "-INT", "-x", "wf-recorder"]);
        recording = false;
    }
    // catches recordings started/stopped outside the bar too
    Process {
        id: recCheck
        command: ["sh", "-c", "pgrep -x wf-recorder >/dev/null && echo yes || echo no"]
        stdout: SplitParser {
            onRead: l => {
                const on = l.trim() === "yes";
                if (on && !root.recording) { root.recording = true; root.recStart = Date.now(); }
                else if (!on && root.recording) root.recording = false;
            }
        }
    }
    Timer {
        interval: 5000; running: root.hasRecorder; repeat: true
        onTriggered: recCheck.running = true
    }

    IpcHandler {
        target: "panel"
        function hide(): void { root.modulesHidden = true; }
        function vanish(): void { root.chromeGone = true; }
        function centerWidth(): string { return String(Math.round(root.centerW)); }
        function restore(): void {
            root.modulesHidden = false;
            if (root.chromeGone) { root.chromeGone = false; root.chromeReturn(); }
        }
        // Compositor top-edge touch swipe → open the control center. panelWin's
        // id isn't in scope here (it lives in the panel delegate), so go via a
        // root signal the panel connects to, like dashToggle/searchToggle.
        function showControl(): void { root.controlToggle(); }
        // Open the control center directly on a page ("main"/"wifi"/"bluetooth").
        function gotoPage(page: string): void { root.controlGoto(page); }
        // Preview the drawn battery on machines without one (testing/demo).
        function batteryDemo(pct: int, charging: bool): void {
            root.batDemo = pct; root.batDemoCharging = charging;
            root.batteryNotch(pct, charging);
        }
        function batteryDemoOff(): void { root.batDemo = -1; }
        // Night light changed (vendi night) — pulse the bar pill. temp in Kelvin,
        // 6500 = off.
        function night(temp: int): void { root.nightNotch(temp); }
        // Do Not Disturb: "on" / "off" / "" (toggle). A confirming toast slips
        // through before the silence (notify() bypasses the gate).
        function dnd(mode: string): void {
            root.dnd = (mode === "on") ? true : (mode === "off") ? false : !root.dnd;
            root.notify(root.dnd ? "Do Not Disturb" : "Notifications on", "");
        }
        // Voice typing feedback: "listening" | "transcribing" | "off"/"".
        function voice(state: string): void {
            root.voiceState = (state === "off") ? "" : state;
            if (root.voiceState !== "") voiceGuard.restart();
        }
    }

    // ── theme ────────────────────────────────────────────────────────────────
    property color rawAccent: "#cba6f7"
    // Light bar tint (`vendi bar light`). Purely the bar's own surface — it is
    // deliberately independent of `vendi appearance`, which themes GTK apps.
    property bool light: false
    FileView {
        path: Quickshell.env("HOME") + "/.config/vendi/bar-light"
        watchChanges: true
        onLoaded: root.light = text().trim() === "1"
        onFileChanged: reload()
    }
    // Surface overlay. Every hover/card/divider in the bar layers translucent
    // WHITE over a dark panel; on a cream panel that washes out to nothing, so
    // light mode flips them to translucent black. One helper keeps the ~50 call
    // sites free of conditionals. Black at the same alpha reads a touch heavier
    // than white, hence the 0.9.
    function surf(a: real): color {
        return root.light ? Qt.rgba(0, 0, 0, a * 0.9) : Qt.rgba(1, 1, 1, a);
    }
    // Theme accents are tuned for a dark backdrop — Mocha's #cba6f7 on cream is
    // barely legible. Keep the hue (that's the theme's identity) but cap the
    // lightness in light mode so accented text and glyphs stay readable.
    property color accent: light && rawAccent.hslLightness > 0.45
        ? Qt.hsla(rawAccent.hslHue, Math.max(rawAccent.hslSaturation, 0.45), 0.42, 1.0)
        : rawAccent
    // Bar/notch background: a near-black base tinted a little toward the theme
    // accent so the whole bar shifts with the theme (warm on gruvbox, red on
    // think, …), not just the text. Light mode swaps it for a cream carrying
    // the same accent tint. Fully solid either way. Re-evaluates live whenever
    // `accent` changes.
    property color panel:  light
        ? Qt.rgba(0.97 - rawAccent.r * 0.05,
                  0.96 - rawAccent.g * 0.05,
                  0.94 - rawAccent.b * 0.05, 1.0)
        : Qt.rgba(0.05 + rawAccent.r * 0.06,
                  0.05 + rawAccent.g * 0.06,
                  0.07 + rawAccent.b * 0.06, 1.0)
    property color fg:     light ? "#1c1c26" : "#cdd6f4"
    property color dim:    light ? "#6c6c80" : "#717189"
    // Status colours are pastels picked for a dark backdrop; on cream they read
    // as barely-there washes, so light mode uses deeper cousins of the same hues.
    property color alert:  light ? "#c1123c" : "#f38ba8"
    property color good:   light ? "#2e7d32" : "#a6e3a1"
    property color warn:   light ? "#9a6700" : "#f9e2af"
    property string mono:  "JetBrainsMonoNL Nerd Font"

    // geometry
    readonly property int stripH: 3
    readonly property int barH:   44
    readonly property int fillet: 12
    readonly property int bcr:    15
    readonly property int pad:    18

    FileView {
        path: Quickshell.env("HOME") + "/.config/vendi/theme-state"
        watchChanges: true
        onLoaded: {
            const m = /ACCENT_HEX=([0-9a-fA-F]{6})/.exec(text());
            if (m) root.rawAccent = "#" + m[1];
        }
        onFileChanged: reload()
    }

    // Night-light state (the saved colour temperature). Drives the persistent
    // corner moon — set silently here; the transient pill comes from panel.night.
    FileView {
        path: Quickshell.env("HOME") + "/.config/vendi/night"
        watchChanges: true
        onLoaded: {
            const t = parseInt(text().trim());
            if (!isNaN(t)) { root.nightTemp = t; root.nightOn = t < 6500; }
        }
        onFileChanged: reload()
    }

    // Active keyboard layout (short code, e.g. "US"/"ES"), published live by
    // vendiwm to $XDG_RUNTIME_DIR/vendiwm-kblayout. Empty = single layout → the
    // corner indicator hides. Cycle with the cycle-kb-layout keybind.
    property string kbLayout: ""
    property bool   kbLayoutInit: false   // skip the OSD on the first read
    property bool   kbOsd: false
    Timer { id: kbOsdTimer; interval: 1600; onTriggered: root.kbOsd = false }
    // Short BCP-47-style tag for the popup; falls back to the raw code.
    readonly property var kbNames: ({ "US": "en-US", "ES": "es-ES", "GB": "en-GB",
        "FR": "fr-FR", "DE": "de-DE", "IT": "it-IT", "PT": "pt-PT", "LATAM": "es-419" })
    function kbLayoutName(code) { return root.kbNames[code] || code; }
    FileView {
        path: (Quickshell.env("XDG_RUNTIME_DIR") || "/tmp") + "/vendiwm-kblayout"
        watchChanges: true
        onLoaded: {
            const v = text().trim();
            // Only flash the notch on an actual switch, not the initial load.
            if (root.kbLayoutInit && v !== "" && v !== root.kbLayout) {
                root.kbOsd = true; kbOsdTimer.restart();
            }
            root.kbLayout = v;
            root.kbLayoutInit = true;
        }
        onFileChanged: reload()
    }

    // Claude Code state — polled (auto-detects a running session; no wiring).
    Process {
        id: claudeProc
        command: ["vendi-claude-status"]
        property string buf: ""
        stdout: SplitParser { onRead: line => claudeProc.buf += line + "\n" }
        onStarted: buf = ""
        onExited: {
            const t = claudeProc.buf;
            if (((/STATE=(.*)/.exec(t) || [])[1] || "off") === "off") {
                root.claudeActive = false; root.claudeWorking = false; return;
            }
            root.claudeModel   = (/MODEL=(.*)/.exec(t)  || [])[1] || "";
            root.claudeUsage   = (/USAGE=(.*)/.exec(t)  || [])[1] || "";
            root.claudeVerb    = (/VERB=(.*)/.exec(t)   || [])[1] || "";
            root.claudeWorking = ((/STATE=(.*)/.exec(t) || [])[1] || "") === "working";
            root.claudeActive  = true;
        }
    }
    Timer {
        interval: 3000; running: true; repeat: true; triggeredOnStart: true
        onTriggered: claudeProc.running = true
    }

    // ── compositor state ─────────────────────────────────────────────────────
    property int activeWs: 1
    property var wsList: [{ id: 1, windows: 0 }]
    property string title: ""
    property bool overviewActive: false   // exposé is open (drives Overview chrome)

    // ── primary screen: the only one with a bar ─────────────────────────────
    // One bar, on one monitor (the other monitor gets its full height for
    // windows, and there's never a question of which bar a shortcut opens).
    // Chosen in Dashboard → Displays (~/.config/vendi/primary-output);
    // default and fallback: the laptop's built-in panel, which never unplugs.
    property string primaryPref: ""
    FileView {
        path: Quickshell.env("HOME") + "/.config/vendi/primary-output"
        watchChanges: true
        printErrors: false
        onLoaded: root.primaryPref = text().trim()
        onLoadFailed: root.primaryPref = ""
        onFileChanged: reload()
    }
    readonly property var screenList: Array.prototype.slice.call(Quickshell.screens)
    readonly property string primaryScreen: {
        const names = screenList.map(s => s.name);
        if (primaryPref !== "" && names.indexOf(primaryPref) >= 0) return primaryPref;
        const builtin = names.find(n => /^(Embedded|eDP|LVDS)/.test(n));
        return builtin ?? (names[0] ?? "");
    }
    function setPrimary(name) {
        Quickshell.execDetached(["sh", "-c",
            "mkdir -p \"$HOME/.config/vendi\" && printf '%s\\n' \"$1\" > \"$HOME/.config/vendi/primary-output\"",
            "_", name]);
    }

    // Monitor that has focus (the pointer's) — shortcuts open the island
    // there only, not on every screen at once.
    property string focusedOutput: ""
    function applyWorkspaces(active, list) {
        activeWs = active;
        focusedOutput = (list.find(w => w.id === active) || {}).output || "";
        // output/visible: which monitor a desk lives on and whether that
        // monitor is showing it — each screen's bar lists only its own desks.
        wsList = list.map(w => ({ id: w.id, windows: w.windows ?? 0,
                                  output: w.output ?? "", visible: w.visible ?? (w.id === active) }));
    }

    Process {
        id: wmSub
        command: ["vendi-ctl", "subscribe", "workspace", "window", "overview"]
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
                    else if (ev.event === "overview")
                        root.overviewActive = ev.active === true;
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
                    wsSnap.acc.push({ id: parseInt(m[2]), windows: 0, output: "", visible: m[1] === "*" });
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
    // Output / input device lists for the control-center audio picker.
    property var audioSinks: Pipewire.nodes
        ? Pipewire.nodes.values.filter(n => n && n.audio && n.isSink && !n.isStream) : []
    property var audioSources: Pipewire.nodes
        ? Pipewire.nodes.values.filter(n => n && n.audio && !n.isSink && !n.isStream
            && n.name && !n.name.includes("monitor")) : []
    // Keep the listed device nodes bound so their state stays live.
    PwObjectTracker { objects: root.audioSinks.concat(root.audioSources) }
    // Mic in use: any app capturing audio (a recording stream exists).
    property bool micInUse: Pipewire.nodes
        ? Pipewire.nodes.values.some(n => n && n.properties
            && n.properties["media.class"] === "Stream/Input/Audio") : false
    // First connected Bluetooth audio device that reports a battery level.
    property var btDevice: {
        if (!UPower.devices) return null;
        for (const d of UPower.devices.values) {
            if (!d || !d.isPresent) continue;
            if (d.type === UPowerDeviceType.Headset
                || d.type === UPowerDeviceType.Headphones
                || d.type === UPowerDeviceType.BluetoothGeneric)
                return d;
        }
        return null;
    }
    // Clamp the displayed volume at 100 — pipewire can report >1.0 if something
    // over-amplified the sink; the bar should never show 130%.
    property int volume: sinkAudio ? Math.min(100, Math.round(sinkAudio.volume * 100)) : -1
    property bool muted: sinkAudio?.muted ?? false
    function setVolume(pct) {
        if (!sinkAudio) return;
        sinkAudio.muted = false;
        sinkAudio.volume = Math.max(0, Math.min(1, pct / 100));
    }

    // ── backlight (brightnessctl) ────────────────────────────────────────────
    // brightnessctl -m → "name,class,current,percent,max" (percent has a % sign).
    // -1 means no backlight device (desktop / VM) — the slider hides itself.
    property int brightness: -1
    property bool hasBacklight: brightness >= 0
    Process {
        id: brightnessGet
        command: ["brightnessctl", "-m", "-c", "backlight"]
        running: true
        stdout: StdioCollector {
            onStreamFinished: {
                const f = text.trim().split(",");
                if (f.length >= 4) {
                    const pct = parseInt(f[3]);
                    if (!isNaN(pct)) root.brightness = pct;
                }
            }
        }
    }
    function setBrightness(pct) {
        const v = Math.max(1, Math.min(100, Math.round(pct)));
        root.brightness = v;            // optimistic — slider tracks the drag
        Quickshell.execDetached(["brightnessctl", "set", v + "%"]);
    }

    // ── keyboard backlight (leds class, *kbd_backlight*) ──────────────────────
    // Often only a handful of steps (0..max), so we drive it by raw level and
    // present it as a percentage of max. Empty kbdDev → no device → slider hides.
    property string kbdDev: ""
    property int kbdMax: 0
    property int kbdLevel: 0
    property bool hasKbdBacklight: kbdDev !== "" && kbdMax > 0
    property int kbdPct: kbdMax > 0 ? Math.round(100 * kbdLevel / kbdMax) : 0
    Process {
        id: kbdGet
        command: ["sh", "-c",
            "d=$(ls /sys/class/leds 2>/dev/null | grep -i kbd_backlight | head -1); " +
            "[ -z \"$d\" ] && exit 0; " +
            "echo \"$d\"; cat /sys/class/leds/$d/brightness; cat /sys/class/leds/$d/max_brightness"]
        running: true
        stdout: StdioCollector {
            onStreamFinished: {
                const l = text.trim().split("\n");
                if (l.length >= 3) {
                    root.kbdDev = l[0];
                    root.kbdLevel = parseInt(l[1]) || 0;
                    root.kbdMax = parseInt(l[2]) || 0;
                }
            }
        }
    }
    function setKbdBacklight(pct) {
        if (!hasKbdBacklight) return;
        const lvl = Math.round(Math.max(0, Math.min(1, pct / 100)) * kbdMax);
        root.kbdLevel = lvl;            // optimistic
        Quickshell.execDetached(["brightnessctl", "-d", kbdDev, "set", String(lvl)]);
    }

    // volume OSD: external changes bulge the right notch for a moment.
    // Armed late so the initial pipewire binding doesn't flash it at startup.
    property bool osdShow: false
    property bool osdArmed: false
    property string osdKind: "volume"   // "volume" | "brightness"
    Timer { interval: 4000; running: true; onTriggered: root.osdArmed = true }
    Timer { id: osdTimer; interval: 1400; onTriggered: root.osdShow = false }
    Connections {
        target: root.sinkAudio
        function onVolumeChanged() { root.pokeOsd("volume") }
        function onMutedChanged()  { root.pokeOsd("volume") }
    }
    function pokeOsd(kind) {
        if (!osdArmed) return;
        osdKind = kind || "volume";
        osdShow = true;
        osdTimer.restart();
    }

    // ── screen-brightness OSD ─────────────────────────────────────────────────
    // The XF86MonBrightness keys run brightnessctl, which writes the backlight
    // sysfs file. Watch that file so the bar flashes a brightness OSD (and keeps
    // root.brightness live) — there's no D-Bus signal for backlight changes.
    property string backlightPath: ""
    property int    backlightMax: 1
    Process {
        id: backlightFind
        running: true
        command: ["sh", "-c",
            "d=$(ls /sys/class/backlight 2>/dev/null | head -1); " +
            "[ -n \"$d\" ] && { echo \"/sys/class/backlight/$d\"; cat \"/sys/class/backlight/$d/max_brightness\"; }"]
        stdout: StdioCollector {
            onStreamFinished: {
                const l = text.trim().split("\n");
                if (l.length >= 2) {
                    root.backlightPath = l[0];
                    root.backlightMax = parseInt(l[1]) || 1;
                }
            }
        }
    }
    FileView {
        path: root.backlightPath !== "" ? root.backlightPath + "/brightness" : ""
        watchChanges: root.backlightPath !== ""
        onFileChanged: reload()
        onLoaded: {
            const v = parseInt(text().trim());
            if (isNaN(v) || root.backlightMax <= 0) return;
            const pct = Math.round(100 * v / root.backlightMax);
            const changed = pct !== root.brightness;
            root.brightness = pct;            // keep the slider live + reactive
            if (changed) root.pokeOsd("brightness");
        }
    }

    // ── battery (upower) ─────────────────────────────────────────────────────
    property var batDev: UPower.displayDevice
    property bool hasBattery: (batDev?.isLaptopBattery ?? false)
    property int battery: {
        const p = batDev?.percentage ?? 0;
        return Math.round(p <= 1 ? p * 100 : p);
    }
    property int batWarned: 100   // lowest threshold already announced
    onBatteryChanged: {
        if (charging) { batWarned = 100; return; }
        for (const th of [20, 10]) {
            if (battery <= th && batWarned > th) {
                batWarned = th;
                root.batteryNotch(battery, false);
                break;
            }
        }
    }
    // Plugged-in state comes from the AC adapter in sysfs, not from the
    // battery's upower state. Measured on plug-in: `AC/online` flips instantly,
    // but the battery goes discharging → pending-charge → charging and only
    // reaches Charging ~2s later — and with a ThinkPad charge threshold holding
    // the battery below 100% it stays "Not charging"/pending-charge
    // indefinitely, so a `state === Charging` test could miss the plug entirely.
    // The adapter node is found by type=="Mains" rather than hardcoding "AC"
    // (it's ADP1/ACAD on plenty of machines).
    property bool acOnline: false
    property string acPath: ""
    // Event-driven, not polled: udev emits a power_supply event the moment the
    // cable goes in, so the island reacts immediately instead of waiting out a
    // poll tick. `udevadm monitor --udev` needs no root. The helper prints the
    // adapter's `online` once at startup and again on every event; QML only
    // reacts to real transitions, so duplicate prints are harmless.
    Process {
        running: true
        command: ["sh", "-c",
            "p=''; for d in /sys/class/power_supply/*; do " +
            "[ \"$(cat \"$d/type\" 2>/dev/null)\" = Mains ] && { p=\"$d/online\"; break; }; done; " +
            "[ -n \"$p\" ] || exit 0; echo \"P=$p\"; cat \"$p\"; " +
            "stdbuf -oL udevadm monitor --udev --subsystem-match=power_supply 2>/dev/null | " +
            "while read -r l; do case \"$l\" in *change*|*add*|*remove*) cat \"$p\";; esac; done"]
        stdout: SplitParser {
            onRead: l => {
                const t = l.trim();
                if (t.startsWith("P=")) { root.acPath = t.slice(2); return; }
                if (t === "0" || t === "1") root.acOnline = (t === "1");
            }
        }
    }
    // Safety net only — if a udev event is ever missed the state still
    // reconciles. Deliberately slow; the monitor above is what makes it feel
    // instant.
    FileView {
        id: acFile
        path: root.acPath
        onLoaded: root.acOnline = text().trim() === "1"
    }
    Timer {
        interval: 10000; running: root.acPath !== ""; repeat: true
        onTriggered: acFile.reload()
    }
    property bool charging: root.acOnline
    onChargingChanged: if (charging && hasBattery) root.batteryNotch(battery, true)
    // Demo override (IPC `panel batteryDemo <pct> <charging>`) so the drawn
    // battery can be previewed on machines with no battery. -1 = off.
    property int  batDemo: -1
    property bool batDemoCharging: false
    property int  batShow:       batDemo >= 0 ? batDemo : battery
    property bool batChargeShow:  batDemo >= 0 ? batDemoCharging : charging
    property bool batVisible:     hasBattery || batDemo >= 0

    // Battery badge (iOS style): a solid colour-coded pill with the % inside —
    // compact, no battery-icon outline. Green when charging, red at ≤20%,
    // white/light otherwise. No animation — the colour is the whole cue.
    component BatteryIndicator: BatteryBadge {
        fg: root.fg
        good: root.good
        alert: root.alert
        textColor: root.panel
        mono: root.mono
        h: 12.5
    }

    // ── network ──────────────────────────────────────────────────────────────
    property string netIcon: "󰤭"
    property bool   vpnUp: false      // a VPN / WireGuard tunnel is active
    Process {
        id: netProc
        // Note whether wifi / ethernet are actually connected, then pick the icon
        // on exit (wifi wins). A bare ":connected" substring match used to let the
        // "loopback:connected" line overwrite the wifi icon with the ethernet one.
        // Also flags an active VPN (device type wireguard/tun, or an active vpn
        // connection) so the corner can show a shield.
        property bool wifiUp: false
        property bool ethUp:  false
        property bool vpn:    false
        command: ["sh", "-c",
            "nmcli -t -f TYPE,STATE d 2>/dev/null | grep -v unmanaged; " +
            "nmcli -t -f TYPE,STATE c show --active 2>/dev/null"]
        stdout: SplitParser {
            onRead: line => {
                if (line.startsWith("wifi:connected")) netProc.wifiUp = true;
                else if (line.startsWith("ethernet:connected")) netProc.ethUp = true;
                if (line.startsWith("vpn:") || line.startsWith("wireguard:") || line.startsWith("tun:"))
                    netProc.vpn = true;
            }
        }
        onStarted: { wifiUp = false; ethUp = false; vpn = false; }
        onExited: {
            root.netIcon = netProc.wifiUp ? "󰤨" : netProc.ethUp ? "󰈀" : "󰤭";
            root.vpnUp = netProc.vpn;
        }
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

    // ── real audio visualizer (cava → 4 bars) ────────────────────────────────
    // cava streams raw ascii levels (4 values, 0..1000 each) on stdout; we feed
    // the 4-bar equalizer in the notch. Only runs while music is playing.
    property var vizLevels: [0, 0, 0, 0]
    Process {
        id: cavaProc
        running: root.musicPlaying
        command: ["sh", "-c",
            "printf '[general]\\nframerate=60\\nbars=4\\n[output]\\nmethod=raw\\nraw_target=/dev/stdout\\ndata_format=ascii\\nascii_max_range=1000\\nchannels=mono\\n' > /tmp/vendi-cava.conf; exec cava -p /tmp/vendi-cava.conf"]
        stdout: SplitParser {
            onRead: line => {
                const t = line.trim();
                if (!t) return;
                const p = t.split(";");
                if (p.length < 4) return;
                root.vizLevels = [
                    (+p[0] || 0) / 1000, (+p[1] || 0) / 1000,
                    (+p[2] || 0) / 1000, (+p[3] || 0) / 1000
                ];
            }
        }
        onRunningChanged: if (!running) root.vizLevels = [0, 0, 0, 0]
    }

    // ── weather (wttr.in) ────────────────────────────────────────────────────
    property string weather: ""        // "☁️ +24°C" — bar + dashboard header
    property string weatherCond: ""    // "Partly cloudy"
    Process {
        id: wxProc
        command: ["vendi-weather"]
        stdout: SplitParser {
            onRead: l => {
                if (l.includes("Unknown")) return;
                const p = l.split("|").map(s => s.trim().replace(/\s+/g, " "));
                if (p.length >= 2 && p[1]) {
                    root.weather = (p[0] ? p[0] + " " : "") + p[1];
                    root.weatherCond = p[2] ?? "";
                }
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

    // ── identity / uptime (dashboard header) ─────────────────────────────────
    property string userName: Quickshell.env("USER") || "vendi"
    property string hostName: ""
    property string uptimeStr: ""
    FileView {
        path: "/etc/hostname"
        onLoaded: root.hostName = text().trim()
    }
    FileView {
        id: upFile
        path: "/proc/uptime"
        onLoaded: {
            const s = parseFloat(text().split(" ")[0]);
            const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60);
            root.uptimeStr = (h > 0 ? h + "h " : "") + m + "m";
        }
    }
    Timer {
        interval: 60000; running: true; repeat: true; triggeredOnStart: true
        onTriggered: upFile.reload()
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

    // ── screensaver videos (~/Videos/Screensavers) ───────────────────────────
    property var screensavers: []
    property string currentScreensaver: ""
    function rescanScreensavers() { ssList.running = true; }
    Process {
        id: ssList
        command: ["sh", "-c",
            "ls -1 \"$HOME\"/Videos/Screensavers/*.mp4 \"$HOME\"/Videos/Screensavers/*.mkv " +
            "\"$HOME\"/Videos/Screensavers/*.webm \"$HOME\"/Videos/Screensavers/*.mov " +
            "\"$HOME\"/Videos/Screensavers/*.m4v \"$HOME\"/Videos/Screensavers/*.avi 2>/dev/null"]
        running: true
        property var acc: []
        stdout: SplitParser { onRead: l => { if (l.trim()) ssList.acc.push(l.trim()); } }
        onStarted: acc = []
        onExited: { root.screensavers = acc; acc = []; }
    }
    FileView {
        path: Quickshell.env("HOME") + "/.config/vendi/screensaver"
        watchChanges: true
        onLoaded: root.currentScreensaver = text().trim()
        onFileChanged: reload()
    }

    // ── notifications (we ARE the daemon) ────────────────────────────────────
    // toasts: live queue shown in the right notch (newest first served).
    // notifHistory: plain snapshots for the control center (safe after the
    // client withdraws the notification).
    property var toasts: []
    property var notifHistory: []
    // Do Not Disturb: notifications still land in history, but no toast
    // interrupts. Toggled from the control center.
    property bool dnd: false

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
            if (!root.dnd) {
                root.toasts = root.toasts.concat([t]);
                toastTimer.restart();
            }
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
    // the bar's own voice — synthetic toast, no client behind it
    function notify(summary, body) {
        toasts = toasts.concat([{
            app: "vendi", icon: "", image: "", n: null,
            summary: summary, body: body,
        }]);
        toastTimer.restart();
    }

    // iOS-style battery notch animation (NOT a notification): the right notch
    // bulges into a "Charging" / "Low Battery" pill for a moment, then springs
    // back. Fires on charger plug-in and on low battery (≤20%).
    property bool batOsd: false
    property int  batOsdPct: 100
    property bool batOsdCharging: false
    Timer { id: batOsdTimer; interval: 4000; onTriggered: root.batOsd = false }
    function batteryNotch(pct, chg) {
        batOsdPct = pct; batOsdCharging = chg;
        batOsd = true; batOsdTimer.restart();
    }

    // Night-light pill: the left wing bulges into "Night 4000K" / "Night Off"
    // for a moment when the colour temperature changes (panel.night IPC), then
    // springs back — same iOS-island feel as the battery pill.
    property bool nightOsd: false
    property bool nightOn:  false
    property int  nightTemp: 6500
    // Warmth colour for the night pill: warm orange at 2500K → pale at 6500K.
    readonly property color nightTone: {
        const f = Math.max(0, Math.min(1, (nightTemp - 2500) / 4000));
        return Qt.rgba(1.0, 0.55 + 0.32 * f, 0.32 + 0.55 * f, 1.0);
    }
    Timer { id: nightOsdTimer; interval: 2600; onTriggered: root.nightOsd = false }
    function nightNotch(temp) {
        nightTemp = temp; nightOn = temp < 6500;
        nightOsd = true; nightOsdTimer.restart();
    }

    // ── vendi-buds: AirPods (+ generic BT headset) ──────────────────────────
    // Polls the daemon's state file, same convention as wallpaper/screensaver/
    // night above — no IPC client needed just to display this.
    property bool   budsConnected: false
    property string budsName: ""
    property string budsKind: "generic"    // "airpods" | "generic"
    property string budsNoiseMode: ""      // "off"|"anc"|"transparency"|"adaptive"|""
    property var    budsBattery: ({})      // {left,right,case} -> {level,charging} | null
    // The single most-actionable number for the transient notch pill: the
    // lower of the two earbuds (whichever needs charging sooner matters more
    // than an average). Case battery is shown in the control-center card only.
    readonly property int budsPct: {
        const l = budsBattery.left,  lv = l ? l.level : undefined;
        const r = budsBattery.right, rv = r ? r.level : undefined;
        if (lv !== undefined && rv !== undefined) return Math.min(lv, rv);
        if (lv !== undefined) return lv;
        if (rv !== undefined) return rv;
        return -1;
    }
    FileView {
        path: Quickshell.env("HOME") + "/.config/vendi/buds-state.json"
        watchChanges: true
        onLoaded: root.applyBudsState(JSON.parse(text()))
        onFileChanged: reload()
    }
    function applyBudsState(s) {
        const wasConnected = root.budsConnected;
        root.budsConnected  = !!s.connected;
        root.budsName       = s.name || "";
        root.budsKind       = s.kind || "generic";
        root.budsNoiseMode  = s.noise_mode || "";
        root.budsBattery    = s.battery || {};
        if (root.budsConnected && !wasConnected) root.budsNotch();
    }
    property bool budsOsd: false
    Timer { id: budsOsdTimer; interval: 4000; onTriggered: root.budsOsd = false }
    function budsNotch() { budsOsd = true; budsOsdTimer.restart(); }

    // ── voice typing feedback (vendi voice via panel.voice IPC) ─────────────
    property string voiceState: ""   // "" | "listening" | "transcribing"
    // Auto-clear if the CLI never sends "off" (e.g. it crashed mid-record).
    Timer { id: voiceGuard; interval: 120000; onTriggered: root.voiceState = "" }

    // ── Claude Code gadget ──────────────────────────────────────────────────
    // Fed by vendi-claude-status (Claude Code statusLine + hooks) writing
    // ~/.config/vendi/claude. Idle: usage · model. Working: the verb pulses.
    property bool   claudeActive:  false
    property bool   claudeWorking: false
    property string claudeModel:   ""
    property string claudeUsage:   ""
    property string claudeVerb:    ""

    // ── 1s heartbeat: clocks, media progress, active player ─────────────────
    Timer {
        interval: 1000; running: true; repeat: true; triggeredOnStart: true
        onTriggered: {
            root.player = root.pickPlayer();
            root.musicProgress = (root.player && root.player.length > 0)
                ? Math.max(0, Math.min(1, root.player.position / root.player.length)) : 0;
            if (root.recording)
                root.recSecs = Math.max(0, Math.floor((Date.now() - root.recStart) / 1000));
        }
    }

    // ── the bar ──────────────────────────────────────────────────────────────
    Variants {
        // the bar lives on the primary screen only
        model: root.screenList.filter(s => s.name === root.primaryScreen)
        PanelWindow {
            id: panelWin
            required property var modelData
            screen: modelData
            anchors { top: true; left: true; right: true }
            color: "transparent"
            WlrLayershell.namespace: "vendibar-pro"
            // Search grabs the keyboard outright (type immediately); the
            // dashboard's task fields just need click-to-focus.
            WlrLayershell.keyboardFocus: (searchOpen || aiOpen)
                ? WlrKeyboardFocus.Exclusive
                : (centerOpen || rightOpen) ? WlrKeyboardFocus.OnDemand
                : WlrKeyboardFocus.None

            // ── expansion state ─────────────────────────────────────────────
            property bool centerOpen: false
            property bool rightOpen: false
            property bool powerOpen: false
            // Spotlight search lives in the center notch too (morphs it).
            property bool searchOpen: false
            property bool aiOpen: false
            property string searchMode: "search"
            // Dedicated now-playing card (album click) — a compact center morph,
            // separate from the full dashboard.
            property bool mediaOpen: false
            function toggleCenter() { centerOpen = !centerOpen; if (centerOpen) { rightOpen = false; powerOpen = false; searchOpen = false; mediaOpen = false; } }
            function openDash(t)    { dashItem.goTab(t); if (!centerOpen) toggleCenter(); }
            function toggleMedia()  { mediaOpen = !mediaOpen; if (mediaOpen) { centerOpen = false; rightOpen = false; powerOpen = false; searchOpen = false; } }
            function toggleRight()  { rightOpen = !rightOpen;  if (rightOpen) { centerOpen = false; powerOpen = false; searchOpen = false; mediaOpen = false; } }
            function togglePower()  { powerOpen = !powerOpen;  if (powerOpen) { centerOpen = false; rightOpen = false; searchOpen = false; mediaOpen = false; } }
            function openSearch(m)  { searchMode = m; searchOpen = true; centerOpen = false; rightOpen = false; powerOpen = false; mediaOpen = false; }
            function closeSearch()  { searchOpen = false; }
            function openAi()  { aiOpen = true; centerOpen = false; rightOpen = false; powerOpen = false; searchOpen = false; }
            function closeAi() { aiOpen = false; }
            // Only ever one panel open at a time — opening any one closes the
            // rest, however it was opened (toggle, keybind, click). Guards only
            // fire on the true edge, so there's no feedback loop.
            onCenterOpenChanged: if (centerOpen) { rightOpen = false; powerOpen = false; searchOpen = false; mediaOpen = false; aiOpen = false; }
            onRightOpenChanged:  { if (rightOpen)  { centerOpen = false; powerOpen = false; searchOpen = false; mediaOpen = false; aiOpen = false; } else { control.ccPage = "main"; } }
            onPowerOpenChanged:  if (powerOpen)  { centerOpen = false; rightOpen = false; searchOpen = false; mediaOpen = false; aiOpen = false; }
            onSearchOpenChanged: if (searchOpen) { centerOpen = false; rightOpen = false; powerOpen = false; mediaOpen = false; aiOpen = false; }
            onMediaOpenChanged:  if (mediaOpen)  { centerOpen = false; rightOpen = false; powerOpen = false; searchOpen = false; aiOpen = false; }
            onAiOpenChanged:     if (aiOpen)     { centerOpen = false; rightOpen = false; powerOpen = false; searchOpen = false; mediaOpen = false; }

            // right notch mode: power menu wins, then control center, then
            // toasts, then the volume OSD
            readonly property string rightMode:
                powerOpen ? "power"
                : rightOpen ? "control"
                // While the island is open, never bulge the right notch for a
                // toast or volume/brightness OSD — it would reach into the
                // expanded island and merge. The dashboard carries its own
                // volume slider, so the OSD is redundant there anyway.
                : centerExpanded ? "idle"
                : root.toasts.length > 0 ? "toast"
                : root.osdShow ? "osd"
                : "idle"

            // notch dimensions, all springy. Idle notches grow a hair on
            // hover — the island invites the click.
            // While the center island is expanded the side pills drop their
            // text (window title / battery %) and keep just their icons, so
            // they shrink a little and stop kissing the island — they don't
            // vanish, and spring back when it collapses. (Scaling on a tiny
            // screen is never going to be roomy; this just adds breathing space.)
            property bool centerExpanded: centerOpen || searchOpen || mediaOpen || aiOpen
            // Side notches (clock/date/weather/workspaces) retreat for the
            // dashboard, search and media card, but STAY while the AI panel is
            // open — the AI notch only takes the center, so the clock keeps
            // showing as a header above the answer.
            property bool sideRetract: centerOpen || searchOpen || mediaOpen
            // The side notches retreat the instant the island opens, but on
            // *close* they must regrow first and only then reveal their text /
            // icons — otherwise the content pops in over a half-sprung notch.
            // `sideHidden` hides instantly on open and lifts after the spring
            // has settled on close; the collapsible content keys its opacity to
            // it (while its `visible` still keys to centerExpanded for width).
            property bool sideHidden: false
            onSideRetractChanged: {
                if (sideRetract) { sideHidden = true; sideRevealTimer.stop(); }
                else sideRevealTimer.restart();
            }
            Timer {
                id: sideRevealTimer; interval: 520
                onTriggered: if (!panelWin.sideRetract) panelWin.sideHidden = false
            }
            property real lw: root.modulesHidden ? 0
                : leftRow.implicitWidth + root.pad * 2
            property real cw: centerOpen ? Math.min(880, panelWin.width - 120)
                : searchOpen ? Math.min(640, panelWin.width - 120)
                : mediaOpen ? Math.min(460, panelWin.width - 120)
                : aiOpen ? Math.min(660, panelWin.width - 120)
                : centerRow.implicitWidth + root.pad * 2 + (centerHover.hovered ? 10 : 0)
            property real rw: root.modulesHidden ? 0
                : rightMode === "control" ? 400
                : rightMode === "power" ? 240
                : rightMode === "toast" ? 380
                : rightMode === "osd" ? 270
                : rightRow.implicitWidth + root.pad * 2 + (rightHover.hovered ? 10 : 0)
            // 620 is the dashboard's design height; a page that genuinely needs
            // more (Config, whose cards are content-sized) reports it rather
            // than being clipped. Still capped to the screen.
            property real ch: centerOpen
                ? Math.min(Math.max(620, dashItem.wantHeight), panelWin.screen.height - 100)
                : searchOpen ? Math.min(panelWin.screen.height - 80, root.stripH + searchItem.wantHeight)
                : mediaOpen ? root.stripH + 150
                : aiOpen ? Math.min(panelWin.screen.height - 80, root.barH + aiItem.wantHeight)
                : root.barH
            property real rh: rightMode === "control"
                    ? (control.ccPage === "audio" ? control.audioPageH
                       : control.ccPage !== "main" ? 470
                       : 312 + (root.batVisible ? 30 : 0)
                       + (root.notifHistory.length > 0
                             ? 30 + Math.min(root.notifHistory.length, 3) * 22 : 0)
                       // vendi-buds card: bound to its OWN real measured
                       // height (+12 for the extra ColumnLayout spacing gap
                       // its insertion adds) rather than a guessed constant —
                       // a hardcoded number silently drifts out of sync every
                       // time the card's content changes (font size, image
                       // size, text wrapping) and starts clipping the
                       // Wi-Fi/Bluetooth/Audio/Notifs buttons below it again.
                       + (root.budsConnected ? budsCard.implicitHeight + 12 : 0))
                : rightMode === "power" ? 224
                : rightMode === "toast"
                    ? Math.max(root.barH, toastCol.implicitHeight + root.stripH + 26)
                : root.barH
            // Spring physics, not timed curves — interruptible and velocity
            // aware, so redirecting mid-flight (open→close→open) flows instead
            // of restarting. Tuned for the iOS dynamic-island feel: quick to
            // move, a whisper of overshoot, clean settle. `epsilon` is in px so
            // it stops cleanly without a long crawling tail.
            // epsilon is the stop threshold in px. Kept large so the spring
            // doesn't crawl the asymptotic last few percent — that tail is
            // invisible on a 270px-tall reveal but reads as a clunky slow-down
            // on open. Cutting it makes open arrive as crisply as close.
            Behavior on lw { SpringAnimation { spring: 9.6; damping: 0.64; mass: 0.70; epsilon: 2.5 } }
            Behavior on cw { SpringAnimation { spring: 7.6; damping: 0.66; mass: 0.82; epsilon: 2.5 } }
            // Right notch travels less than the center, so the same spring
            // finishes quicker and reads as too fast — soften it to match.
            Behavior on rw { SpringAnimation { spring: 7.4; damping: 0.64; mass: 0.78; epsilon: 2.5 } }
            Behavior on ch { SpringAnimation { spring: 7.6; damping: 0.66; mass: 0.80; epsilon: 4.0 } }
            Behavior on rh { SpringAnimation { spring: 7.4; damping: 0.64; mass: 0.78; epsilon: 4.0 } }
            onLwChanged: silhouette.requestPaint()
            onCwChanged: { silhouette.requestPaint(); root.centerW = cw; }
            Component.onCompleted: root.centerW = cw
            onRwChanged: silhouette.requestPaint()
            onChChanged: silhouette.requestPaint()
            onRhChanged: silhouette.requestPaint()

            // the window grows with the tallest notch; the desktop never
            // reflows — expansions overlay it.
            // +room below the notch for the AI glow's bottom bleed (the input
            // mask stays on the notch, so this extra area doesn't catch clicks).
            implicitHeight: Math.ceil(Math.max(root.barH, ch, rh)) + 4 + (aiOpen ? 34 : 0)
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

            // auto-close when the pointer wanders off an open panel (the big
            // dashboard gets a longer leash, and never closes mid-typing)
            HoverHandler { id: panelHover }
            Timer {
                running: (panelWin.centerOpen || panelWin.rightOpen || panelWin.powerOpen || panelWin.mediaOpen)
                         && !panelHover.hovered && !dashItem.typing
                interval: panelWin.centerOpen ? 3200 : 1600
                onTriggered: {
                    panelWin.centerOpen = false;
                    panelWin.rightOpen = false;
                    panelWin.powerOpen = false;
                    panelWin.mediaOpen = false;
                }
            }
            // Keybind / IPC shortcuts act on the focused monitor's bar only
            // (every screen has its own bar; they used to all open at once).
            // Unknown focus (old compositor, first frame) → every bar reacts.
            // (with a single bar it's always the one to react)
            readonly property bool focusedScreen: true
            Connections {
                target: root
                function onDashToggle() { if (panelWin.focusedScreen) panelWin.toggleCenter(); }
                function onMediaToggle() { if (panelWin.focusedScreen) panelWin.toggleMedia(); }
                function onControlToggle() { if (panelWin.focusedScreen && !panelWin.rightOpen) panelWin.toggleRight(); }
                function onControlGoto(page) {
                    if (!panelWin.focusedScreen) return;
                    control.ccPage = page; if (!panelWin.rightOpen) panelWin.toggleRight();
                }
                function onDashOpen(tab) {
                    if (!panelWin.focusedScreen) return;
                    dashItem.goTab(tab);
                    if (!panelWin.centerOpen) panelWin.toggleCenter();
                }
                function onSearchToggle(mode) {
                    if (!panelWin.focusedScreen) return;
                    if (panelWin.searchOpen && panelWin.searchMode === mode)
                        panelWin.closeSearch();
                    else
                        panelWin.openSearch(mode);
                }
                function onAiToggle() {
                    if (panelWin.aiOpen) panelWin.closeAi();
                    else if (panelWin.focusedScreen) panelWin.openAi();
                }
                function onAiSet(on) {
                    if (!on) panelWin.closeAi();
                    else if (panelWin.focusedScreen) panelWin.openAi();
                }
            }

            // ── chrome: every visual, sliding as one piece ──────────────────
            // vendilock hides the bar through this — the whole silhouette
            // (and its contents) glides off the top edge and back.
            Item {
            id: chrome
            anchors.fill: parent
            visible: !root.chromeGone
            transform: Translate { id: chromeSlide; y: 0 }
            Connections {
                target: root
                function onChromeReturn() { chromeDrop.restart(); }
            }
            NumberAnimation {
                id: chromeDrop
                target: chromeSlide; property: "y"
                from: -(root.barH + 30); to: 0
                duration: 340; easing.type: Easing.OutCubic
            }

            // ── AI notch glow — a cool accent bloom that breathes behind the
            //    center notch while the AI panel is open (bleeds past the edges)
            Item {
                id: aiGlowSrc
                visible: false
                layer.enabled: true
                x: (panelWin.width - panelWin.cw) / 2 - 36
                y: root.stripH - 30
                width: panelWin.cw + 72
                height: panelWin.ch - root.stripH + 56
                Rectangle {
                    id: aiGlowFill
                    anchors.fill: parent; anchors.margins: 34
                    radius: 30
                    // Vivify the theme accent for the glow: force decent saturation
                    // and a mid lightness so washed-out/monochrome accents (e.g. a
                    // pale dynamic-wallpaper accent) don't blur into white/gray.
                    readonly property real gh: root.accent.hslHue
                    readonly property real gs: Math.max(root.accent.hslSaturation, 0.62)
                    readonly property real gl: Math.min(Math.max(root.accent.hslLightness, 0.46), 0.60)
                    gradient: Gradient {
                        orientation: Gradient.Horizontal
                        GradientStop { position: 0.0; color: Qt.hsla(aiGlowFill.gh, aiGlowFill.gs, aiGlowFill.gl, 1) }
                        GradientStop { position: 1.0; color: Qt.hsla((aiGlowFill.gh + 0.12) % 1.0, aiGlowFill.gs, aiGlowFill.gl, 1) }
                    }
                }
            }
            MultiEffect {
                source: aiGlowSrc
                x: aiGlowSrc.x; y: aiGlowSrc.y
                width: aiGlowSrc.width; height: aiGlowSrc.height
                blurEnabled: true; blur: 1.0; blurMax: 44; autoPaddingEnabled: true
                opacity: panelWin.aiOpen ? 0.55 : 0
                Behavior on opacity { NumberAnimation { duration: 380; easing.type: Easing.InOutSine } }
                transformOrigin: Item.Center
                SequentialAnimation on scale {
                    running: panelWin.aiOpen; loops: Animation.Infinite
                    NumberAnimation { from: 1.0; to: 1.035; duration: 1900; easing.type: Easing.InOutSine }
                    NumberAnimation { from: 1.035; to: 1.0; duration: 1900; easing.type: Easing.InOutSine }
                }
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
                    const s = root.stripH, r = root.fillet;
                    // Expanded panels round wider than the idle notches.
                    const b  = root.bcr;
                    const bc = panelWin.ch > root.barH + 10 ? 24 : root.bcr;
                    const br = panelWin.rh > root.barH + 10 ? 24 : root.bcr;
                    const lw = panelWin.lw, cw = panelWin.cw, rw = panelWin.rw;
                    // Clamp the animated heights: the springy close (OutBack)
                    // undershoots below barH, which folds the path into a
                    // self-intersection and blanks the whole silhouette.
                    const lh = root.barH;
                    const chh = Math.max(lh, panelWin.ch);
                    const rhh = Math.max(lh, panelWin.rh);
                    const cx = (w - cw) / 2;
                    const rx = w - rw;
                    ctx.reset();
                    ctx.beginPath();
                    ctx.moveTo(0, 0);
                    // left notch — flat against the screen edge; only the
                    // inner bottom corner rounds
                    ctx.lineTo(0, lh);
                    ctx.lineTo(lw - b, lh);
                    ctx.arcTo(lw, lh, lw, lh - b, b);
                    ctx.lineTo(lw, s + r);
                    ctx.arc(lw + r, s + r, r, Math.PI, Math.PI * 1.5, false);
                    // center notch
                    ctx.lineTo(cx - r, s);
                    ctx.arc(cx - r, s + r, r, -Math.PI / 2, 0, false);
                    ctx.lineTo(cx, chh - bc);
                    ctx.arcTo(cx, chh, cx + bc, chh, bc);
                    ctx.lineTo(cx + cw - bc, chh);
                    ctx.arcTo(cx + cw, chh, cx + cw, chh - bc, bc);
                    ctx.lineTo(cx + cw, s + r);
                    ctx.arc(cx + cw + r, s + r, r, Math.PI, Math.PI * 1.5, false);
                    // right notch — flush with the right corner
                    ctx.lineTo(rx - r, s);
                    ctx.arc(rx - r, s + r, r, -Math.PI / 2, 0, false);
                    ctx.lineTo(rx, rhh - br);
                    ctx.arcTo(rx, rhh, rx + br, rhh, br);
                    ctx.lineTo(w, rhh);
                    ctx.lineTo(w, 0);
                    ctx.closePath();
                    ctx.fillStyle = root.panel;
                    ctx.fill();
                    // No rim/border: a lighter edge made overlapping notches
                    // read as a seam. Flat fill only, so any overlap just
                    // blends into one solid shape.
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
                color: root.surf(0.10)
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
                opacity: root.modulesHidden ? 0 : 1
                Behavior on opacity { NumberAnimation { duration: 200 } }

                VendiMark {
                    accent: root.accent
                    implicitWidth: 17
                    implicitHeight: 17
                    Layout.alignment: Qt.AlignVCenter
                }

                RowLayout {
                    spacing: 5
                    Repeater {
                        // every desk, on every monitor (the one bar speaks for both)
                        model: root.wsList
                        Rectangle {
                            required property var modelData
                            // the desk this screen is showing; it's solid accent
                            // only while it also has focus (pointer on this screen)
                            property bool current: modelData.visible === true
                            property bool focusedDesk: modelData.id === root.activeWs
                            Layout.alignment: Qt.AlignVCenter
                            // Drive the RowLayout's spacing through preferredWidth
                            // (not `width`) so the wide active pill pushes its
                            // neighbours over instead of overlapping them.
                            Layout.preferredWidth: current ? 30 : 19
                            Layout.preferredHeight: 19
                            radius: 9.5
                            color: current ? (focusedDesk ? root.accent
                                                    : Qt.rgba(root.accent.r, root.accent.g, root.accent.b, 0.35))
                                 : modelData.windows > 0 ? root.surf(0.14)
                                 : root.surf(0.05)
                            Behavior on Layout.preferredWidth { NumberAnimation { duration: 200; easing.type: Easing.OutBack } }
                            Behavior on color { ColorAnimation { duration: 150 } }
                            Mono {
                                anchors.centerIn: parent
                                text: parent.modelData.id
                                color: parent.current && parent.focusedDesk ? "#0b0b12"
                                     : parent.current ? root.fg : root.dim
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

                Sep {
                    visible: root.title.length > 0 && !panelWin.sideRetract
                    opacity: panelWin.sideHidden ? 0 : 1
                    Behavior on opacity { NumberAnimation { duration: 150 } }
                }
                Mono {
                    text: root.title.length > 42 ? root.title.slice(0, 42) + "…" : root.title
                    visible: root.title.length > 0 && !panelWin.sideRetract
                    color: root.dim
                    opacity: panelWin.sideHidden ? 0 : 1
                    Behavior on opacity { NumberAnimation { duration: 150 } }
                }
            }

            // ── center notch collapsed row: clock · date · weather ──────────
            RowLayout {
                id: centerRow
                anchors.horizontalCenter: parent.horizontalCenter
                y: root.stripH
                height: root.barH - root.stripH
                spacing: 10
                opacity: (panelWin.centerOpen || panelWin.searchOpen || panelWin.mediaOpen) ? 0 : 1
                // opacity 0 still eats clicks — the dashboard / search / media card
                // live in this exact strip, so actually drop the row from input
                visible: opacity > 0
                Behavior on opacity { NumberAnimation { duration: 140 } }
                property color batTone: root.batOsdCharging ? root.good : root.alert
                // battery alert, left wing: "Charging" / "Low Battery" — flanks
                // the clock and expands the notch to the sides, iOS-island style.
                Mono {
                    // Fade in rather than appearing at full opacity the instant
                    // the notch starts springing open — popping fully-formed
                    // text into a still-expanding island is what read as jumpy.
                    visible: opacity > 0.01
                    opacity: root.batOsd ? 1 : 0
                    Behavior on opacity { NumberAnimation { duration: 200; easing.type: Easing.OutCubic } }
                    text: root.batOsdCharging ? "Charging" : "Low Battery"
                    font.bold: true
                    color: root.fg
                }
                // night-light, left wing: the label (the moon + warmth swatch
                // are on the right wing, so the island stays symmetric).
                Mono {
                    visible: root.nightOsd
                    text: root.nightOn ? ("Night " + root.nightTemp + "K") : "Night Off"
                    font.bold: true
                    color: root.fg
                }
                // keyboard-layout switch, left wing: a keyboard glyph (the layout
                // name is on the right wing, flanking the clock).
                Glyph {
                    visible: root.kbOsd
                    text: "󰌌"
                    font.pixelSize: 15
                    color: root.accent
                    Layout.alignment: Qt.AlignVCenter
                }
                // voice typing, left wing: pulsing mic + "Speak now" while
                // listening, "Transcribing…" while it works. Clear feedback so
                // you know when vendi voice is recording.
                Row {
                    visible: root.voiceState !== ""
                    spacing: 6
                    Layout.alignment: Qt.AlignVCenter
                    Glyph {
                        anchors.verticalCenter: parent.verticalCenter
                        text: "󰍬"
                        font.pixelSize: 14
                        color: root.voiceState === "listening" ? "#f25c5c" : root.accent
                        SequentialAnimation on opacity {
                            running: root.voiceState === "listening"
                            loops: Animation.Infinite
                            NumberAnimation { to: 0.3; duration: 550; easing.type: Easing.InOutSine }
                            NumberAnimation { to: 1.0; duration: 550; easing.type: Easing.InOutSine }
                        }
                    }
                    Mono {
                        anchors.verticalCenter: parent.verticalCenter
                        text: root.voiceState === "transcribing" ? "Transcribing…" : "Speak now"
                        font.bold: true
                        color: root.fg
                    }
                }
                // screen-recording pill — blinking red dot + elapsed time;
                // click it to stop the recording (brainshell-style).
                Item {
                    visible: root.recording
                    implicitWidth: recRow.implicitWidth
                    implicitHeight: 18
                    Row {
                        id: recRow
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 6
                        Rectangle {
                            width: 8; height: 8; radius: 4
                            color: "#f25c5c"
                            anchors.verticalCenter: parent.verticalCenter
                            SequentialAnimation on opacity {
                                running: root.recording
                                loops: Animation.Infinite
                                NumberAnimation { to: 0.25; duration: 700 }
                                NumberAnimation { to: 1; duration: 700 }
                            }
                        }
                        Mono {
                            anchors.verticalCenter: parent.verticalCenter
                            text: Math.floor(root.recSecs / 60).toString().padStart(2, "0")
                                  + ":" + (root.recSecs % 60).toString().padStart(2, "0")
                            color: "#f25c5c"
                            font.pixelSize: 11
                        }
                    }
                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: root.stopRecord()
                    }
                }
                // focus timer, left wing: a tiny progress ring + the phase
                // ("Focus"/"Break"); the countdown sits on the right wing. Both
                // wings share one width so the clock stays dead center.
                // Click either to jump straight to the Focus tool.
                Item {
                    id: focusL
                    readonly property bool show: root.tools.focusActive && !panelWin.centerExpanded
                    readonly property color tint: root.tools.focusPhase === "focus" ? root.accent : root.good
                    readonly property real wingW: Math.max(focusLRow.implicitWidth, focusR.textW)
                    visible: show
                    implicitWidth: wingW
                    implicitHeight: 18
                    opacity: panelWin.sideHidden ? 0 : 1
                    Behavior on opacity { NumberAnimation { duration: 150 } }
                    Row {
                        id: focusLRow
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 6
                        Canvas {
                            id: focusDial
                            width: 13; height: 13
                            anchors.verticalCenter: parent.verticalCenter
                            readonly property real frac: root.tools.focusPhaseLen > 0
                                ? root.tools.focusLeft / root.tools.focusPhaseLen : 0
                            onFracChanged: requestPaint()
                            Connections { target: focusL; function onTintChanged() { focusDial.requestPaint() } }
                            onPaint: {
                                const c = getContext("2d");
                                c.reset();
                                c.lineWidth = 2;
                                c.strokeStyle = root.surf(0.18);
                                c.beginPath(); c.arc(6.5, 6.5, 5, 0, 2 * Math.PI); c.stroke();
                                c.strokeStyle = focusL.tint;
                                c.lineCap = "round";
                                c.beginPath();
                                c.arc(6.5, 6.5, 5, -Math.PI / 2, -Math.PI / 2 + 2 * Math.PI * Math.max(0, frac));
                                c.stroke();
                            }
                        }
                        Mono {
                            anchors.verticalCenter: parent.verticalCenter
                            text: root.tools.focusPhase === "focus" ? "Focus" : "Break"
                            color: root.fg
                            opacity: root.tools.focusRunning ? 1 : 0.55
                            font.pixelSize: 12
                            font.bold: true
                        }
                    }
                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: { dashItem.tool = 3; panelWin.openDash(2); }
                    }
                }
                // media island, left wing: a tiny 4-bar equalizer reacting to
                // the music (real audio levels — art on the other wing). Click it
                // to expand the now-playing card on the dashboard.
                Item {
                    visible: root.musicPlaying
                    implicitWidth: 20
                    implicitHeight: 18
                    Row {
                        anchors.centerIn: parent
                        spacing: 2
                        Repeater {
                            model: [0, 1, 2, 3]
                            Rectangle {
                                required property int modelData
                                width: 3
                                radius: 1.5
                                color: root.accent
                                anchors.verticalCenter: parent.verticalCenter
                                // real audio-reactive level (0..1) → 4..14 px
                                height: 4 + Math.max(0, Math.min(1, root.vizLevels[modelData] ?? 0)) * 10
                                Behavior on height { NumberAnimation { duration: 90; easing.type: Easing.OutQuad } }
                            }
                        }
                    }
                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: panelWin.toggleMedia()
                    }
                }
                // Claude Code gadget, left wing: session usage % (auto-detected
                // when Claude Code is running). The model / "Cooking…" verb is on
                // the right wing, so it flanks the clock like the battery island.
                Row {
                    visible: root.claudeActive && !panelWin.centerExpanded
                    spacing: 8
                    opacity: panelWin.sideHidden ? 0 : 1
                    Behavior on opacity { NumberAnimation { duration: 150 } }
                    Image {
                        anchors.verticalCenter: parent.verticalCenter
                        source: Qt.resolvedUrl("claude.svg")
                        sourceSize.width: 11; sourceSize.height: 11
                        smooth: true
                    }
                    Mono {
                        anchors.verticalCenter: parent.verticalCenter
                        text: root.claudeUsage
                        // soft red once the 5h usage hits 90%+ (theme alert, not pure red)
                        color: ((parseInt(root.claudeUsage) || 0) >= 90) ? root.alert : root.fg
                        font.pixelSize: 13
                    }
                }
                // vendi-buds connect pill, left wing: a small picture of the
                // earbuds (AirPods get their own icon; anything else gets a
                // generic headphones icon — no per-vendor art beyond that).
                Image {
                    visible: root.budsOsd
                    Layout.alignment: Qt.AlignVCenter
                    source: Qt.resolvedUrl(root.budsKind === "airpods" ? "airpods.png" : "headphones-generic.svg")
                    fillMode: Image.PreserveAspectFit
                    sourceSize.width: 16; sourceSize.height: 16
                    smooth: true
                }
                // date · time · weather — the bold clock sits in the middle,
                // flanked by the dim date on the left and weather on the right.
                Mono { id: dateT; color: root.dim }
                Mono { id: clockT; font.bold: true; font.pixelSize: 14 }
                Mono { text: root.weather; visible: root.weather !== ""; color: root.dim }
                // Claude Code gadget, right wing: the model, or a pulsing
                // "Cooking…" while Claude is working.
                Row {
                    visible: root.claudeActive && !panelWin.centerExpanded
                    spacing: 5
                    opacity: panelWin.sideHidden ? 0 : 1
                    Behavior on opacity { NumberAnimation { duration: 150 } }
                    Rectangle {
                        visible: root.claudeWorking
                        width: 6; height: 6; radius: 3; color: root.accent
                        anchors.verticalCenter: parent.verticalCenter
                        SequentialAnimation on opacity {
                            running: root.claudeWorking; loops: Animation.Infinite
                            NumberAnimation { to: 0.3; duration: 600; easing.type: Easing.InOutSine }
                            NumberAnimation { to: 1.0; duration: 600; easing.type: Easing.InOutSine }
                        }
                    }
                    Mono {
                        anchors.verticalCenter: parent.verticalCenter
                        text: root.claudeWorking ? ((root.claudeVerb || "Cooking") + "…") : root.claudeModel
                        color: root.claudeWorking ? root.accent : root.fg
                        font.bold: root.claudeWorking
                    }
                }
                // media island, right wing: the album art, rounded. Click it to
                // expand the now-playing card on the dashboard.
                ClippingRectangle {
                    visible: root.musicPlaying
                    implicitWidth: 20
                    implicitHeight: 20
                    radius: 6
                    color: root.surf(0.06)
                    Image {
                        anchors.fill: parent
                        source: root.player?.trackArtUrl ?? ""
                        fillMode: Image.PreserveAspectCrop
                        asynchronous: true
                    }
                    Mono {
                        anchors.centerIn: parent
                        visible: !(root.player && root.player.trackArtUrl)
                        text: "󰝚"
                        font.pixelSize: 11
                        color: root.accent
                    }
                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: panelWin.toggleMedia()
                    }
                }
                // focus timer, right wing: the countdown (see focusL).
                Item {
                    id: focusR
                    readonly property real textW: focusRText.implicitWidth
                    visible: focusL.show
                    implicitWidth: focusL.wingW
                    implicitHeight: 18
                    opacity: panelWin.sideHidden ? 0 : 1
                    Behavior on opacity { NumberAnimation { duration: 150 } }
                    Mono {
                        id: focusRText
                        anchors.left: parent.left
                        anchors.verticalCenter: parent.verticalCenter
                        text: root.tools.focusClock
                        color: focusL.tint
                        opacity: root.tools.focusRunning ? 1 : 0.55
                        font.pixelSize: 12
                        font.bold: true
                    }
                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: { dashItem.tool = 3; panelWin.openDash(2); }
                    }
                }
                // recording, right wing: a red waveform so the notch stays
                // symmetric with the timer pill on the left.
                Item {
                    visible: root.recording
                    implicitWidth: 24
                    implicitHeight: 18
                    Row {
                        anchors.centerIn: parent
                        spacing: 2
                        Repeater {
                            model: [0, 1, 2, 3, 4]
                            Rectangle {
                                required property int modelData
                                width: 3
                                radius: 1.5
                                color: "#f25c5c"
                                height: 5
                                anchors.verticalCenter: parent.verticalCenter
                                SequentialAnimation on height {
                                    running: root.recording
                                    loops: Animation.Infinite
                                    NumberAnimation { to: 15 - (modelData % 3) * 3; duration: 240 + modelData * 60; easing.type: Easing.InOutSine }
                                    NumberAnimation { to: 5 + (modelData % 2) * 3;  duration: 280 + modelData * 40; easing.type: Easing.InOutSine }
                                    NumberAnimation { to: 12 - modelData;           duration: 220 + modelData * 70; easing.type: Easing.InOutSine }
                                    NumberAnimation { to: 4;                        duration: 260 + modelData * 50; easing.type: Easing.InOutSine }
                                }
                            }
                        }
                    }
                }
                // battery alert, right wing: percentage + a real battery icon
                // (rounded-rectangle body, level fill, little square terminal).
                Row {
                    visible: opacity > 0.01
                    opacity: root.batOsd ? 1 : 0
                    Behavior on opacity { NumberAnimation { duration: 200; easing.type: Easing.OutCubic } }
                    spacing: 6
                    Layout.alignment: Qt.AlignVCenter
                    Mono {
                        text: root.batOsdPct + "%"
                        font.bold: true
                        color: centerRow.batTone
                        anchors.verticalCenter: parent.verticalCenter
                    }
                    Item {
                        width: 25; height: 12
                        anchors.verticalCenter: parent.verticalCenter
                        Rectangle {
                            id: batBody
                            width: 22; height: 12; radius: 2.5
                            color: "transparent"
                            border.width: 1.5
                            border.color: centerRow.batTone
                            Rectangle {
                                anchors.left: parent.left
                                anchors.leftMargin: 2
                                anchors.verticalCenter: parent.verticalCenter
                                height: parent.height - 4
                                width: Math.max(2, (parent.width - 4) * Math.max(0, Math.min(100, root.batOsdPct)) / 100)
                                radius: 1
                                color: centerRow.batTone
                                Behavior on width { NumberAnimation { duration: 220 } }
                            }
                        }
                        // little square terminal on the right
                        Rectangle {
                            anchors.left: batBody.right
                            anchors.leftMargin: 1
                            anchors.verticalCenter: batBody.verticalCenter
                            width: 2.5; height: 4; radius: 0.5
                            color: centerRow.batTone
                        }
                    }
                }
                // vendi-buds connect pill, right wing: just the percentage —
                // deliberately no battery-icon graphic here (unlike the laptop
                // battery pill above), per how this was spec'd: the picture is
                // the left wing's job, this side is only the number.
                Mono {
                    visible: root.budsOsd && root.budsPct >= 0
                    text: root.budsPct + "%"
                    font.bold: true
                    color: root.fg
                }
                // night-light, right wing: moon glyph + a warmth swatch (warm
                // orange → pale), symmetric with the temperature label on the left.
                Row {
                    visible: root.nightOsd
                    spacing: 6
                    Layout.alignment: Qt.AlignVCenter
                    Mono {
                        text: "\u{f0594}"   // weather-night (moon)
                        font.pixelSize: 13
                        color: root.nightOn ? root.nightTone : root.fg
                        anchors.verticalCenter: parent.verticalCenter
                    }
                    Rectangle {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 22; height: 12; radius: 3
                        color: root.nightOn ? root.nightTone : "transparent"
                        border.width: root.nightOn ? 0 : 1.5
                        border.color: root.fg
                        opacity: root.nightOn ? 0.9 : 0.6
                        Behavior on color { ColorAnimation { duration: 220 } }
                    }
                }
                // keyboard-layout switch, right wing: the layout's friendly name.
                Mono {
                    visible: root.kbOsd
                    text: root.kbLayoutName(root.kbLayout)
                    font.bold: true
                    color: root.fg
                    Layout.alignment: Qt.AlignVCenter
                }
                TapHandler { onTapped: panelWin.toggleCenter() }
                HoverHandler { id: centerHover; cursorShape: Qt.PointingHandCursor }
            }
            Timer {
                interval: 1000; running: true; repeat: true; triggeredOnStart: true
                onTriggered: {
                    const now = new Date();
                    clockT.text = Qt.formatDateTime(now, "HH:mm");
                    dateT.text  = Qt.formatDateTime(now, "ddd d MMM");
                }
            }

            // ── dashboard (expanded center notch) — the five-room command
            //    center lives in Dash.qml ─────────────────────────────────────
            Item {
                id: dashboardBox
                x: (panelWin.width - panelWin.cw) / 2
                y: root.stripH
                width: panelWin.cw
                height: panelWin.ch - root.stripH
                clip: true
                visible: opacity > 0
                opacity: panelWin.centerOpen ? 1 : 0
                Behavior on opacity { NumberAnimation { duration: 180 } }
                TapHandler { onTapped: {} }   // swallow clicks inside

                Dash {
                    id: dashItem
                    anchors.fill: parent
                    bar: root
                    onRequestClose: panelWin.centerOpen = false
                }
                Connections {
                    target: panelWin
                    function onCenterOpenChanged() {
                        if (panelWin.centerOpen) dashItem.refresh();
                    }
                }
            }

            // ── now-playing card (compact center morph) — opened by clicking the
            //    album art / visualizer in the collapsed island ─────────────────
            Item {
                id: mediaBox
                x: (panelWin.width - panelWin.cw) / 2
                y: root.stripH
                width: panelWin.cw
                height: panelWin.ch - root.stripH
                clip: true
                visible: opacity > 0
                opacity: panelWin.mediaOpen ? 1 : 0
                Behavior on opacity { NumberAnimation { duration: 180 } }
                TapHandler { onTapped: {} }   // swallow clicks inside

                // album art washes the card faintly behind the content
                ClippingRectangle {
                    anchors.fill: parent
                    radius: 18
                    color: "transparent"
                    Image {
                        anchors.fill: parent
                        source: root.player?.trackArtUrl ?? ""
                        fillMode: Image.PreserveAspectCrop
                        sourceSize.width: 640
                        asynchronous: true
                        opacity: 0.14
                        visible: (root.player?.trackArtUrl ?? "") !== ""
                    }
                }
                RowLayout {
                    anchors.fill: parent
                    anchors.margins: 16
                    spacing: 16
                    ClippingRectangle {
                        Layout.preferredWidth: 104; Layout.preferredHeight: 104
                        Layout.alignment: Qt.AlignVCenter
                        radius: 14
                        color: root.surf(0.06)
                        Image {
                            anchors.fill: parent
                            source: root.player?.trackArtUrl ?? ""
                            fillMode: Image.PreserveAspectCrop
                            sourceSize.width: 220; asynchronous: true
                            visible: (root.player?.trackArtUrl ?? "") !== ""
                        }
                        Glyph {
                            anchors.centerIn: parent; text: "󰝚"; font.pixelSize: 32
                            color: root.accent
                            visible: (root.player?.trackArtUrl ?? "") === ""
                        }
                    }
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 5
                        Item { Layout.fillHeight: true }
                        Mono {
                            Layout.fillWidth: true
                            text: (root.player?.trackTitle ?? "") || "Nothing playing"
                            font.bold: true; font.pixelSize: 16; elide: Text.ElideRight
                            color: root.player ? root.fg : root.dim
                        }
                        Mono {
                            Layout.fillWidth: true
                            text: root.player?.trackArtist || "music shows up here when it plays"
                            color: root.dim; font.pixelSize: 12; elide: Text.ElideRight
                        }
                        Rectangle {
                            id: mSeek
                            Layout.fillWidth: true; Layout.topMargin: 6
                            height: 5; radius: 2.5
                            color: root.surf(0.10)
                            visible: (root.player ?? null) !== null
                            Rectangle {
                                width: parent.width * (root.musicProgress ?? 0)
                                height: parent.height; radius: 2.5; color: root.accent
                                Behavior on width { NumberAnimation { duration: 500 } }
                            }
                            MouseArea {
                                anchors.fill: parent; anchors.margins: -6
                                cursorShape: Qt.PointingHandCursor
                                onClicked: m => {
                                    const p = root.player;
                                    if (p && p.canSeek && p.length > 0)
                                        p.position = Math.max(0, Math.min(1,
                                            (m.x - 6) / mSeek.width)) * p.length;
                                }
                            }
                        }
                        RowLayout {
                            Layout.alignment: Qt.AlignHCenter
                            Layout.topMargin: 4
                            spacing: 28
                            Glyph {
                                text: "󰒮"; font.pixelSize: 19; color: root.fg
                                MouseArea { anchors.fill: parent; anchors.margins: -6
                                    cursorShape: Qt.PointingHandCursor
                                    onClicked: root.player?.previous() }
                            }
                            Rectangle {
                                Layout.preferredWidth: 38; Layout.preferredHeight: 38; radius: 19
                                color: Qt.rgba(root.accent.r, root.accent.g, root.accent.b, 0.18)
                                Glyph {
                                    anchors.centerIn: parent
                                    text: (root.musicPlaying ?? false) ? "󰏤" : "󰐊"
                                    color: root.accent; font.pixelSize: 17
                                }
                                MouseArea { anchors.fill: parent
                                    cursorShape: Qt.PointingHandCursor
                                    onClicked: root.player?.togglePlaying() }
                            }
                            Glyph {
                                text: "󰒭"; font.pixelSize: 19; color: root.fg
                                MouseArea { anchors.fill: parent; anchors.margins: -6
                                    cursorShape: Qt.PointingHandCursor
                                    onClicked: root.player?.next() }
                            }
                        }
                        Item { Layout.fillHeight: true }
                    }
                }
            }

            // ── spotlight search (expanded center notch) — the notch itself
            //    becomes the search box; Launcher.qml fills it ────────────────
            Item {
                id: searchBox
                x: (panelWin.width - panelWin.cw) / 2
                y: root.stripH
                width: panelWin.cw
                height: panelWin.ch - root.stripH
                clip: true
                visible: opacity > 0
                opacity: panelWin.searchOpen ? 1 : 0
                Behavior on opacity { NumberAnimation { duration: 150 } }
                TapHandler { onTapped: {} }   // swallow clicks inside

                Launcher {
                    id: searchItem
                    anchors.fill: parent
                    active: panelWin.searchOpen
                    mode: panelWin.searchMode
                    light: root.light
                    accent: root.accent
                    panel: root.panel
                    fg: root.fg
                    dim: root.dim
                    mono: root.mono
                    onRequestClose: panelWin.closeSearch()
                }
            }

            // ── vendi AI (expanded center notch) — the notch becomes the Siri
            //    panel; AiContent.qml fills it (super+a) ───────────────────────
            Item {
                id: aiBox
                x: (panelWin.width - panelWin.cw) / 2
                y: root.barH                       // below the clock/date header
                width: panelWin.cw
                height: panelWin.ch - root.barH
                clip: true
                visible: opacity > 0
                opacity: panelWin.aiOpen ? 1 : 0
                Behavior on opacity { NumberAnimation { duration: 150 } }
                TapHandler { onTapped: {} }   // swallow clicks inside

                AiContent {
                    id: aiItem
                    anchors.fill: parent
                    active: panelWin.aiOpen
                    light: root.light
                    accent: root.accent
                    panelColor: root.panel
                    fg: root.fg
                    dim: root.dim
                    mono: root.mono
                    onRequestClose: panelWin.closeAi()
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
                opacity: root.modulesHidden ? 0
                       : panelWin.rightMode === "idle" ? 1 : 0
                Behavior on opacity { NumberAnimation { duration: 160 } }
                HoverHandler { id: rightHover }
                // Scroll anywhere on the corner cluster to nudge the volume.
                WheelHandler {
                    onWheel: event => {
                        const step = event.angleDelta.y > 0 ? 5 : -5;
                        root.setVolume(Math.max(0, Math.min(100, root.volume + step)));
                    }
                }


                // downloads pill: a bobbing arrow + size · rate while a browser
                // downloads; a green check + the file name when one lands.
                // Click: open the finished file, or the downloads folder.
                Item {
                    id: dlPill
                    readonly property bool done: root.dlDoneShow && !root.dlActive
                    visible: (root.dlActive || done) && !panelWin.sideRetract
                    implicitWidth: dlRow.implicitWidth
                    implicitHeight: 18
                    Layout.alignment: Qt.AlignVCenter
                    opacity: panelWin.sideHidden ? 0 : 1
                    Behavior on opacity { NumberAnimation { duration: 150 } }
                    RowLayout {
                        id: dlRow
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 5
                        Item {
                            implicitWidth: 14
                            implicitHeight: 18
                            Glyph {
                                id: dlArrow
                                anchors.horizontalCenter: parent.horizontalCenter
                                y: 1
                                text: dlPill.done ? "󰄬" : "󰇚"
                                color: dlPill.done ? root.good : root.accent
                                font.pixelSize: 14
                                SequentialAnimation on y {
                                    running: root.dlActive
                                    loops: Animation.Infinite
                                    NumberAnimation { from: -1; to: 3; duration: 520; easing.type: Easing.InOutSine }
                                    NumberAnimation { from: 3; to: -1; duration: 520; easing.type: Easing.InOutSine }
                                }
                            }
                        }
                        Mono {
                            text: dlPill.done ? root.dlShort(root.dlDoneName)
                                : root.dlFiles.length > 1
                                    ? root.dlFiles.length + " files · " + root.dlHuman(root.dlBytes)
                                    : root.dlShort(root.dlFiles[0]?.name ?? "") + " · " + root.dlHuman(root.dlBytes)
                            color: dlPill.done ? root.fg : root.dim
                            font.pixelSize: 11
                        }
                        Mono {
                            visible: root.dlActive && root.dlRate > 0
                            text: root.dlHuman(root.dlRate) + "/s"
                            color: root.accent
                            font.pixelSize: 11
                            font.bold: true
                        }
                    }
                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: {
                            Quickshell.execDetached(["xdg-open", dlPill.done ? root.dlDonePath : root.dlDir]);
                            root.dlDoneShow = false;
                        }
                    }
                }
                Sep {
                    visible: dlPill.visible
                    opacity: panelWin.sideHidden ? 0 : 1
                    Behavior on opacity { NumberAnimation { duration: 150 } }
                }

                // system tray (icons only; click = activate)
                RowLayout {
                    spacing: 8
                    visible: SystemTray.items.values.length > 0 && !panelWin.sideRetract
                    opacity: panelWin.sideHidden ? 0 : 1
                    Behavior on opacity { NumberAnimation { duration: 150 } }
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
                Sep {
                    visible: SystemTray.items.values.length > 0 && !panelWin.sideRetract
                    opacity: panelWin.sideHidden ? 0 : 1
                    Behavior on opacity { NumberAnimation { duration: 150 } }
                }

                // quiet icon cluster — no numbers in the corner; the control
                // center carries the detail. Hidden while the island is open so
                // the right pill collapses to just the power button.
                RowLayout {
                    spacing: 11
                    visible: !panelWin.sideRetract
                    opacity: panelWin.sideHidden ? 0 : 1
                    Behavior on opacity { NumberAnimation { duration: 150 } }
                    Glyph { text: root.netIcon; font.pixelSize: 14 }
                    // Keyboard layout — only shown with multiple layouts (e.g. US/ES).
                    Mono {
                        visible: root.kbLayout !== ""
                        text: root.kbLayout
                        color: root.fg
                        font.pixelSize: 11
                        font.bold: true
                        Layout.alignment: Qt.AlignVCenter
                    }
                    // VPN shield — only present while a tunnel is up.
                    Glyph {
                        visible: root.vpnUp
                        text: "󰦝"
                        color: root.accent
                        font.pixelSize: 14
                    }
                    // Night-light moon — present while night light is on.
                    Glyph {
                        visible: root.nightOn
                        text: "󰖔"
                        color: root.nightTone
                        font.pixelSize: 14
                    }
                    // Mic in use — red, while any app is capturing audio.
                    Glyph {
                        visible: root.micInUse
                        text: "󰍬"
                        color: "#f25c5c"
                        font.pixelSize: 14
                    }
                    // Bluetooth device battery (headset / earbuds…).
                    Row {
                        visible: root.btDevice !== null
                        spacing: 3
                        Layout.alignment: Qt.AlignVCenter
                        Glyph {
                            anchors.verticalCenter: parent.verticalCenter
                            text: "󰋋"; font.pixelSize: 14
                            color: (root.btDevice && root.btDevice.percentage <= 20) ? root.alert : root.fg
                        }
                        Mono {
                            anchors.verticalCenter: parent.verticalCenter
                            text: root.btDevice ? Math.round(root.btDevice.percentage) + "%" : ""
                            color: root.dim; font.pixelSize: 11
                        }
                    }
                    Glyph {
                        text: root.muted ? "󰝟" : root.volume > 60 ? "󰕾" : root.volume > 20 ? "󰖀" : "󰕿"
                        color: root.muted ? root.dim : root.fg
                        font.pixelSize: 14
                    }
                    BatteryIndicator {
                        visible: root.batVisible
                        pct: root.batShow
                        charging: root.batChargeShow
                        Layout.alignment: Qt.AlignVCenter
                    }
                    Glyph {
                        // bell-off while Do Not Disturb is on, else bell / bell-outline.
                        text: root.dnd ? "󰂛" : (root.notifHistory.length > 0 ? "󰂚" : "󰂜")
                        color: root.dnd ? root.accent
                             : (root.notifHistory.length > 0 ? root.fg : root.dim)
                        font.pixelSize: 14
                    }
                    TapHandler { onTapped: panelWin.toggleRight() }
                }

                Sep {
                    visible: !panelWin.sideRetract
                    opacity: panelWin.sideHidden ? 0 : 1
                    Behavior on opacity { NumberAnimation { duration: 150 } }
                }
                Mono {
                    text: "󰐥"
                    color: root.accent
                    font.pixelSize: 14
                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: panelWin.togglePower()
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

                readonly property bool bri: root.osdKind === "brightness"
                Glyph {
                    text: osdRow.bri ? "󰃟"
                        : root.muted ? "󰝟" : root.volume > 60 ? "󰕾" : root.volume > 20 ? "󰖀" : "󰕿"
                    color: (!osdRow.bri && root.muted) ? root.dim : root.accent
                    font.pixelSize: 15
                }
                Rectangle {
                    Layout.preferredWidth: 150
                    height: 6
                    radius: 3
                    color: root.surf(0.10)
                    Rectangle {
                        width: parent.width * Math.max(0, Math.min(100, osdRow.bri ? root.brightness : root.volume)) / 100
                        height: parent.height
                        radius: 3
                        color: (!osdRow.bri && root.muted) ? root.dim : root.accent
                        Behavior on width { NumberAnimation { duration: 100 } }
                    }
                }
                Mono {
                    text: osdRow.bri ? root.brightness + "%"
                        : root.muted ? "muted" : root.volume + "%"
                    Layout.preferredWidth: 44
                }
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
                                color: actHover.hovered ? root.surf(0.14) : root.surf(0.07)
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

            // ── power menu (right notch, native — lock · suspend · restart ·
            //    shut down) ──────────────────────────────────────────────────
            Item {
                id: powerPanel
                x: panelWin.width - panelWin.rw
                y: root.stripH
                width: panelWin.rw
                height: panelWin.rh - root.stripH
                clip: true
                visible: opacity > 0
                opacity: panelWin.rightMode === "power" ? 1 : 0
                Behavior on opacity { NumberAnimation { duration: 180 } }
                TapHandler { onTapped: {} }

                ColumnLayout {
                    anchors.fill: parent
                    anchors.margins: 14
                    spacing: 5

                    component PowerRow: Rectangle {
                        property string glyph
                        property string label
                        property bool danger: false
                        property var run
                        Layout.fillWidth: true
                        height: 40
                        radius: 11
                        color: prHover.hovered
                            ? (danger ? Qt.rgba(0.953, 0.545, 0.659, 0.18) : root.surf(0.10))
                            : root.surf(0.05)
                        Behavior on color { ColorAnimation { duration: 120 } }
                        HoverHandler { id: prHover; cursorShape: Qt.PointingHandCursor }
                        RowLayout {
                            anchors.fill: parent
                            anchors.leftMargin: 14
                            spacing: 12
                            Glyph {
                                text: glyph
                                color: danger && prHover.hovered ? root.alert : root.accent
                                font.pixelSize: 15
                            }
                            Mono { text: label }
                            Item { Layout.fillWidth: true }
                        }
                        TapHandler {
                            onTapped: { panelWin.powerOpen = false; run(); }
                        }
                    }

                    PowerRow {
                        glyph: "󰌾"; label: "Lock"
                        run: () => Quickshell.execDetached(["vendi-ctl", "lock"])
                    }
                    PowerRow {
                        glyph: "󰒲"; label: "Suspend"
                        run: () => Quickshell.execDetached(["systemctl", "suspend"])
                    }
                    PowerRow {
                        glyph: "󰜉"; label: "Restart"; danger: true
                        run: () => Quickshell.execDetached(["systemctl", "reboot"])
                    }
                    PowerRow {
                        glyph: "󰐥"; label: "Shut down"; danger: true
                        run: () => Quickshell.execDetached(["systemctl", "poweroff"])
                    }
                    Item { Layout.fillHeight: true }
                }
            }

            // ── control center (expanded right notch) ───────────────────────
            Item {
                id: control
                // Which page the control center is showing: the main grid, or a
                // Wi-Fi / Bluetooth sub-page slid in over it. Reset to main when
                // the center closes (see onRightOpenChanged).
                property string ccPage: "main"
                // ── radio power state (Wi-Fi / Bluetooth hard on-off) ──────────
                property bool wifiRadio: true
                property bool btRadio: true
                function setWifiRadio(on) {
                    wifiRadio = on;
                    Quickshell.execDetached(["nmcli", "radio", "wifi", on ? "on" : "off"]);
                    if (on) wifiRefresh.restart();
                }
                function setBtRadio(on) {
                    btRadio = on;
                    Quickshell.execDetached(["bluetoothctl", "power", on ? "on" : "off"]);
                    if (on) btRefresh.restart();
                }
                Process {
                    id: radioState
                    running: panelWin.rightOpen
                    command: ["sh","-c",
                        "echo wifi=$(nmcli radio wifi 2>/dev/null); " +
                        "echo bt=$(bluetoothctl show 2>/dev/null | grep -m1 Powered | grep -qi yes && echo enabled || echo disabled)"]
                    stdout: SplitParser {
                        onRead: line => {
                            const t = line.trim();
                            if (t.startsWith("wifi=")) control.wifiRadio = t.endsWith("enabled");
                            else if (t.startsWith("bt=")) control.btRadio = t.endsWith("enabled");
                        }
                    }
                }
                // re-poll radio state whenever the control center opens
                Connections {
                    target: panelWin
                    function onRightOpenChanged() { if (panelWin.rightOpen) radioState.running = true; }
                }
                // small pill toggle reused by the Wi-Fi / Bluetooth headers
                component RadioToggle: Rectangle {
                    id: rt
                    property bool on: false
                    signal toggled(bool value)
                    width: 38; height: 20; radius: 10
                    color: on ? root.good : root.surf(0.14)
                    Behavior on color { ColorAnimation { duration: 160 } }
                    Rectangle {
                        // White reads on a dark track or a saturated "on" one;
                        // on a cream track in the off state it vanishes.
                        width: 16; height: 16; radius: 8
                        color: (rt.on || !root.light) ? "#ffffff" : root.fg
                        anchors.verticalCenter: parent.verticalCenter
                        x: rt.on ? parent.width - width - 2 : 2
                        Behavior on x { NumberAnimation { duration: 160; easing.type: Easing.OutCubic } }
                    }
                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: rt.toggled(!rt.on)
                    }
                }
                // Audio page sizes to its device lists (no big empty panel).
                readonly property int audioPageH: Math.min(470,
                    150 + (Math.max(1, root.audioSinks.length)
                         + Math.max(1, root.audioSources.length)) * 36)
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
                    opacity: control.ccPage === "main" ? 1 : 0
                    visible: opacity > 0
                    transform: Translate {
                        x: control.ccPage === "main" ? 0 : -28
                        Behavior on x { NumberAnimation { duration: 200; easing.type: Easing.OutCubic } }
                    }
                    Behavior on opacity { NumberAnimation { duration: 160 } }

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
                            color: root.surf(0.10)
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

                    // screen brightness slider — only when there's a backlight device
                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 10
                        visible: root.hasBacklight
                        Glyph { text: "󰃟"; color: root.fg }
                        Rectangle {
                            id: briTrack
                            Layout.fillWidth: true
                            height: 8
                            radius: 4
                            color: root.surf(0.10)
                            Rectangle {
                                width: Math.max(8, parent.width * Math.max(0, root.brightness) / 100)
                                height: parent.height
                                radius: 4
                                color: root.accent
                                Behavior on width { NumberAnimation { duration: 80 } }
                            }
                            MouseArea {
                                anchors.fill: parent
                                anchors.margins: -6
                                cursorShape: Qt.PointingHandCursor
                                function setBri(mx) {
                                    root.setBrightness(
                                        Math.max(0, Math.min(1, mx / briTrack.width)) * 100);
                                }
                                onPressed: m => setBri(m.x - 6)
                                onPositionChanged: m => { if (pressed) setBri(m.x - 6) }
                            }
                        }
                        Mono { text: root.brightness + "%"; Layout.preferredWidth: 38 }
                    }

                    // keyboard backlight slider — only when a *kbd_backlight led exists
                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 10
                        visible: root.hasKbdBacklight
                        Glyph { text: "󰌌"; color: root.fg }
                        Rectangle {
                            id: kbdTrack
                            Layout.fillWidth: true
                            height: 8
                            radius: 4
                            color: root.surf(0.10)
                            Rectangle {
                                width: Math.max(8, parent.width * Math.max(0, root.kbdPct) / 100)
                                height: parent.height
                                radius: 4
                                color: root.accent
                                Behavior on width { NumberAnimation { duration: 80 } }
                            }
                            MouseArea {
                                anchors.fill: parent
                                anchors.margins: -6
                                cursorShape: Qt.PointingHandCursor
                                function setKbd(mx) {
                                    root.setKbdBacklight(
                                        Math.max(0, Math.min(1, mx / kbdTrack.width)) * 100);
                                }
                                onPressed: m => setKbd(m.x - 6)
                                onPositionChanged: m => { if (pressed) setKbd(m.x - 6) }
                            }
                        }
                        Mono { text: root.kbdPct + "%"; Layout.preferredWidth: 38 }
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

                    // vendi-buds: appears only while something's connected, between
                    // the sliders/notifications above and the quick-actions grid
                    // below — right = picture + name, left = noise-mode buttons
                    // (AirPods only — protocol's unknown for anything else) and,
                    // under them, L/R/case battery — centered under the button
                    // grid, with the device name trailing right after it.
                    RowLayout {
                        id: budsCard
                        Layout.fillWidth: true
                        visible: root.budsConnected
                        spacing: 12

                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 6
                            component NoiseBtn: Rectangle {
                                property string mode
                                property string label
                                readonly property bool active: root.budsNoiseMode === mode
                                Layout.fillWidth: true
                                implicitHeight: 28
                                radius: 8
                                color: active ? Qt.rgba(root.accent.r, root.accent.g, root.accent.b, 0.22)
                                     : nbHover.hovered ? root.surf(0.10) : root.surf(0.05)
                                Behavior on color { ColorAnimation { duration: 120 } }
                                HoverHandler { id: nbHover; cursorShape: Qt.PointingHandCursor }
                                TapHandler { onTapped: Quickshell.execDetached(["vendi-buds", "noise", mode]) }
                                Mono {
                                    anchors.centerIn: parent
                                    text: label
                                    font.pixelSize: 12
                                    font.bold: active
                                    color: active ? root.accent : root.fg
                                }
                            }
                            GridLayout {
                                Layout.fillWidth: true
                                rowSpacing: 6
                                columnSpacing: 6
                                visible: root.budsKind === "airpods"
                                columns: 2
                                NoiseBtn { mode: "off";          label: "Off" }
                                NoiseBtn { mode: "anc";          label: "ANC" }
                                NoiseBtn { mode: "transparency"; label: "Transparency" }
                                NoiseBtn { mode: "adaptive";     label: "Adaptive" }
                            }
                            RowLayout {
                                Layout.alignment: Qt.AlignHCenter
                                spacing: 10
                                component BudBat: Mono {
                                    property var comp
                                    property string label
                                    text: label + " " + (comp?.level !== undefined ? comp.level + "%" : "—")
                                    font.pixelSize: 13
                                    color: root.dim
                                }
                                BudBat { label: "L";    comp: root.budsBattery.left }
                                BudBat { label: "R";    comp: root.budsBattery.right }
                                BudBat { label: "Case"; comp: root.budsBattery.case }
                                Mono {
                                    Layout.preferredWidth: 90
                                    text: root.budsName
                                    font.pixelSize: 10
                                    color: root.dim
                                    elide: Text.ElideRight
                                }
                            }
                        }
                        Image {
                            Layout.alignment: Qt.AlignVCenter
                            source: Qt.resolvedUrl(root.budsKind === "airpods" ? "airpods.png" : "headphones-generic.svg")
                            fillMode: Image.PreserveAspectFit
                            // airpods.png is pre-cropped to its actual content
                            // bounds (was a 380x720 canvas with the pods only
                            // occupying a ~236px-tall strip in the middle —
                            // rendering that uncropped left huge empty space
                            // above/below and pushed everything below it down).
                            sourceSize.height: 74
                            smooth: true
                        }
                    }

                    Rectangle { Layout.fillWidth: true; height: 1; color: root.surf(0.08) }

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
                            property bool active: false
                            Layout.fillWidth: true
                            height: 38
                            radius: 12
                            color: active ? Qt.rgba(root.accent.r, root.accent.g, root.accent.b, 0.18)
                                 : qaHover.hovered ? root.surf(0.10) : root.surf(0.05)
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
                            glyph: root.netIcon === "󰤨" ? "󰤨" : "󰤭"; label: "Wi-Fi"
                            run: () => { control.ccPage = "wifi"; wifiScan.rescan(); }
                        }
                        QuickAction {
                            glyph: "󰂯"; label: "Bluetooth"
                            run: () => { control.ccPage = "bluetooth"; btScan.rescan(); }
                        }
                        QuickAction {
                            glyph: "󰕾"; label: "Audio"
                            run: () => control.ccPage = "audio"
                        }
                        QuickAction {
                            glyph: root.dnd ? "󰂛" : "󰂚"
                            label: "Notifs"
                            active: root.dnd
                            run: () => control.ccPage = "notifications"
                        }
                        QuickAction {
                            glyph: "󰹑"; label: "Shot"
                            run: () => {
                                panelWin.rightOpen = false;
                                Quickshell.execDetached(["sh", "-c",
                                    "sleep 0.3; grim -g \"$(slurp)\" - | wl-copy -t image/png"]);
                            }
                        }
                        QuickAction {
                            glyph: "󰐥"; label: "Power"
                            run: () => panelWin.togglePower()
                        }
                    }

                    Item { Layout.fillHeight: true }

                    // battery — pinned at the very bottom
                    RowLayout {
                        Layout.fillWidth: true
                        visible: root.batVisible
                        spacing: 10
                        Glyph { text: "󰁹"; color: root.fg; font.pixelSize: 14 }
                        Mono { text: "Battery"; color: root.fg }
                        Item { Layout.fillWidth: true }
                        Mono {
                            visible: root.batChargeShow
                            text: "Charging"
                            color: root.dim; font.pixelSize: 11
                        }
                        BatteryIndicator {
                            pct: root.batShow
                            charging: root.batChargeShow
                            h: 16
                            Layout.alignment: Qt.AlignVCenter
                        }
                    }
                }

                // ── Wi-Fi sub-page ───────────────────────────────────────────
                ColumnLayout {
                    anchors.fill: parent
                    anchors.margins: 20
                    spacing: 10
                    opacity: control.ccPage === "wifi" ? 1 : 0
                    visible: opacity > 0
                    transform: Translate {
                        x: control.ccPage === "wifi" ? 0 : 28
                        Behavior on x { NumberAnimation { duration: 200; easing.type: Easing.OutCubic } }
                    }
                    Behavior on opacity { NumberAnimation { duration: 160 } }

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 8
                        Mono {
                            text: "󰁍"; color: root.fg
                            MouseArea { anchors.fill: parent; anchors.margins: -8
                                cursorShape: Qt.PointingHandCursor; onClicked: control.ccPage = "main" }
                        }
                        Mono { text: "Wi-Fi"; font.bold: true; color: root.accent }
                        Item { Layout.fillWidth: true }
                        Glyph {
                            id: wifiRescan
                            visible: control.wifiRadio
                            text: "󰑐"   // refresh
                            color: wifiHov.hovered ? root.fg : root.dim
                            font.pixelSize: 15
                            Layout.alignment: Qt.AlignVCenter
                            HoverHandler { id: wifiHov; cursorShape: Qt.PointingHandCursor }
                            RotationAnimation on rotation {
                                running: wifiScan.scanning
                                loops: Animation.Infinite
                                from: 0; to: 360; duration: 900
                                onRunningChanged: if (!running) wifiRescan.rotation = 0
                            }
                            MouseArea { anchors.fill: parent; anchors.margins: -8
                                cursorShape: Qt.PointingHandCursor; onClicked: wifiScan.rescan() }
                        }
                        RadioToggle {
                            Layout.alignment: Qt.AlignVCenter
                            on: control.wifiRadio
                            onToggled: value => control.setWifiRadio(value)
                        }
                    }
                    Item {
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        visible: !control.wifiRadio
                        ColumnLayout {
                            anchors.centerIn: parent
                            spacing: 6
                            Glyph { text: "󰤭"; font.pixelSize: 26; color: root.dim
                                    Layout.alignment: Qt.AlignHCenter }
                            Mono { text: "Wi-Fi is off"; color: root.dim
                                   Layout.alignment: Qt.AlignHCenter }
                        }
                    }
                    ListView {
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        visible: control.wifiRadio
                        clip: true
                        spacing: 1
                        model: wifiModel
                        delegate: Rectangle {
                            required property var modelData
                            width: ListView.view ? ListView.view.width : 0
                            height: 34
                            radius: 8
                            color: wHov.hovered ? root.surf(0.08) : "transparent"
                            HoverHandler { id: wHov }
                            RowLayout {
                                anchors.fill: parent
                                anchors.leftMargin: 8; anchors.rightMargin: 8
                                spacing: 8
                                Glyph {
                                    text: modelData.signal >= 70 ? "󰤨" : modelData.signal >= 45 ? "󰤥"
                                        : modelData.signal >= 20 ? "󰤢" : "󰤟"
                                    color: modelData.active ? root.good : root.fg
                                }
                                Mono {
                                    Layout.fillWidth: true
                                    text: modelData.ssid
                                    color: modelData.active ? root.good : root.fg
                                    elide: Text.ElideRight
                                }
                                Glyph { visible: modelData.secure; text: "󰌾"; color: root.dim; font.pixelSize: 12 }
                                Glyph { visible: modelData.active; text: "󰄬"; color: root.good }
                            }
                            TapHandler {
                                onTapped: {
                                    if (modelData.active) return;
                                    if (modelData.secure) {
                                        wifiPw.ssid = modelData.ssid; wifiPw.visible = true;
                                        wifiPwField.text = ""; wifiPwField.forceActiveFocus();
                                    } else wifiScan.connectTo(modelData.ssid, "");
                                }
                            }
                        }
                    }
                    RowLayout {
                        id: wifiPw
                        property string ssid: ""
                        Layout.fillWidth: true
                        visible: false
                        spacing: 8
                        Rectangle {
                            Layout.fillWidth: true
                            height: 30; radius: 8
                            color: root.surf(0.08)
                            TextInput {
                                id: wifiPwField
                                anchors.fill: parent
                                anchors.leftMargin: 10; anchors.rightMargin: 10
                                verticalAlignment: TextInput.AlignVCenter
                                color: root.fg
                                font.family: root.mono; font.pixelSize: 12
                                // mask dots cram together — space them out
                                font.letterSpacing: text.length > 0 ? 2.5 : 0
                                echoMode: TextInput.Password
                                clip: true
                                cursorVisible: true
                                onAccepted: { wifiScan.connectTo(wifiPw.ssid, text); wifiPw.visible = false; }
                                Text {
                                    anchors.verticalCenter: parent.verticalCenter
                                    visible: wifiPwField.text.length === 0
                                    text: "password"
                                    color: root.dim
                                    font.family: root.mono; font.pixelSize: 12
                                }
                            }
                        }
                        Mono {
                            text: "join"; color: root.accent
                            MouseArea { anchors.fill: parent; anchors.margins: -8
                                cursorShape: Qt.PointingHandCursor
                                onClicked: { wifiScan.connectTo(wifiPw.ssid, wifiPwField.text); wifiPw.visible = false; } }
                        }
                    }
                }

                // ── Bluetooth sub-page ───────────────────────────────────────
                ColumnLayout {
                    anchors.fill: parent
                    anchors.margins: 20
                    spacing: 10
                    opacity: control.ccPage === "bluetooth" ? 1 : 0
                    visible: opacity > 0
                    transform: Translate {
                        x: control.ccPage === "bluetooth" ? 0 : 28
                        Behavior on x { NumberAnimation { duration: 200; easing.type: Easing.OutCubic } }
                    }
                    Behavior on opacity { NumberAnimation { duration: 160 } }

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 8
                        Mono {
                            text: "󰁍"; color: root.fg
                            MouseArea { anchors.fill: parent; anchors.margins: -8
                                cursorShape: Qt.PointingHandCursor; onClicked: control.ccPage = "main" }
                        }
                        Mono { text: "Bluetooth"; font.bold: true; color: root.accent }
                        Item { Layout.fillWidth: true }
                        Glyph {
                            id: btRescan
                            visible: control.btRadio
                            text: "󰑐"   // refresh
                            color: btHov.hovered ? root.fg : root.dim
                            font.pixelSize: 15
                            Layout.alignment: Qt.AlignVCenter
                            HoverHandler { id: btHov; cursorShape: Qt.PointingHandCursor }
                            RotationAnimation on rotation {
                                running: btScan.scanning
                                loops: Animation.Infinite
                                from: 0; to: 360; duration: 900
                                onRunningChanged: if (!running) btRescan.rotation = 0
                            }
                            MouseArea { anchors.fill: parent; anchors.margins: -8
                                cursorShape: Qt.PointingHandCursor; onClicked: btScan.rescan() }
                        }
                        RadioToggle {
                            Layout.alignment: Qt.AlignVCenter
                            on: control.btRadio
                            onToggled: value => control.setBtRadio(value)
                        }
                    }
                    Item {
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        visible: !control.btRadio
                        ColumnLayout {
                            anchors.centerIn: parent
                            spacing: 6
                            Glyph { text: "󰂲"; font.pixelSize: 26; color: root.dim
                                    Layout.alignment: Qt.AlignHCenter }
                            Mono { text: "Bluetooth is off"; color: root.dim
                                   Layout.alignment: Qt.AlignHCenter }
                        }
                    }
                    ListView {
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        visible: control.btRadio
                        clip: true
                        spacing: 1
                        model: btModel
                        delegate: Rectangle {
                            required property var modelData
                            width: ListView.view ? ListView.view.width : 0
                            height: 34
                            radius: 8
                            color: bHov.hovered ? root.surf(0.08) : "transparent"
                            HoverHandler { id: bHov }
                            RowLayout {
                                anchors.fill: parent
                                anchors.leftMargin: 8; anchors.rightMargin: 8
                                spacing: 8
                                Glyph { text: "󰂯"; color: modelData.connected ? root.good : root.fg }
                                Mono {
                                    Layout.fillWidth: true
                                    text: modelData.name
                                    color: modelData.connected ? root.good : root.fg
                                    elide: Text.ElideRight
                                }
                                Glyph { visible: modelData.connected; text: "󰄬"; color: root.good }
                            }
                            TapHandler { onTapped: btScan.toggle(modelData.mac, modelData.connected) }
                        }
                    }
                }

                // ── Notifications sub-page ───────────────────────────────────
                ColumnLayout {
                    anchors.fill: parent
                    anchors.margins: 20
                    spacing: 10
                    opacity: control.ccPage === "notifications" ? 1 : 0
                    visible: opacity > 0
                    transform: Translate {
                        x: control.ccPage === "notifications" ? 0 : 28
                        Behavior on x { NumberAnimation { duration: 200; easing.type: Easing.OutCubic } }
                    }
                    Behavior on opacity { NumberAnimation { duration: 160 } }

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 8
                        Mono {
                            text: "󰁍"; color: root.fg
                            MouseArea { anchors.fill: parent; anchors.margins: -8
                                cursorShape: Qt.PointingHandCursor; onClicked: control.ccPage = "main" }
                        }
                        Mono { text: "Notifications"; font.bold: true; color: root.accent }
                        Item { Layout.fillWidth: true }
                        Mono {
                            text: "clear"; color: root.dim; font.pixelSize: 11
                            visible: root.notifHistory.length > 0
                            MouseArea { anchors.fill: parent; anchors.margins: -8
                                cursorShape: Qt.PointingHandCursor; onClicked: root.notifHistory = [] }
                        }
                    }
                    // Do Not Disturb toggle
                    Rectangle {
                        Layout.fillWidth: true
                        height: 40; radius: 10
                        color: root.dnd ? Qt.rgba(root.accent.r, root.accent.g, root.accent.b, 0.18)
                             : root.surf(0.05)
                        Behavior on color { ColorAnimation { duration: 120 } }
                        RowLayout {
                            anchors.fill: parent
                            anchors.leftMargin: 12; anchors.rightMargin: 12
                            spacing: 8
                            Glyph { text: root.dnd ? "󰂛" : "󰂚"; color: root.dnd ? root.accent : root.fg }
                            Mono { text: "Do Not Disturb"; Layout.fillWidth: true; color: root.fg }
                            Rectangle {   // pill switch
                                width: 38; height: 20; radius: 10
                                color: root.dnd ? root.accent : root.surf(0.18)
                                Behavior on color { ColorAnimation { duration: 120 } }
                                Rectangle {
                                    width: 16; height: 16; radius: 8
                                    color: (root.dnd || !root.light) ? "white" : root.fg
                                    y: 2; x: root.dnd ? 20 : 2
                                    Behavior on x { NumberAnimation { duration: 140; easing.type: Easing.OutCubic } }
                                }
                            }
                        }
                        TapHandler { onTapped: root.dnd = !root.dnd }
                    }
                    ListView {
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        clip: true
                        spacing: 4
                        model: root.notifHistory
                        delegate: Rectangle {
                            required property var modelData
                            width: ListView.view ? ListView.view.width : 0
                            implicitHeight: nRow.implicitHeight + 14
                            radius: 8
                            color: root.surf(0.04)
                            ColumnLayout {
                                id: nRow
                                anchors.left: parent.left; anchors.right: parent.right
                                anchors.verticalCenter: parent.verticalCenter
                                anchors.leftMargin: 10; anchors.rightMargin: 10
                                spacing: 1
                                RowLayout {
                                    Layout.fillWidth: true
                                    Mono { text: modelData.app; color: root.accent; font.pixelSize: 10; font.bold: true }
                                    Item { Layout.fillWidth: true }
                                    Mono { text: modelData.when; color: root.dim; font.pixelSize: 10 }
                                }
                                Mono {
                                    Layout.fillWidth: true
                                    text: modelData.summary
                                    color: root.fg; font.pixelSize: 11
                                    elide: Text.ElideRight
                                }
                            }
                        }
                    }
                    Mono {
                        visible: root.notifHistory.length === 0
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        text: "No notifications"
                        color: root.dim
                        horizontalAlignment: Text.AlignHCenter
                        verticalAlignment: Text.AlignVCenter
                    }
                }

                // ── Audio sub-page (output + input device pickers) ───────────
                ColumnLayout {
                    anchors.fill: parent
                    anchors.margins: 20
                    spacing: 10
                    opacity: control.ccPage === "audio" ? 1 : 0
                    visible: opacity > 0
                    transform: Translate {
                        x: control.ccPage === "audio" ? 0 : 28
                        Behavior on x { NumberAnimation { duration: 200; easing.type: Easing.OutCubic } }
                    }
                    Behavior on opacity { NumberAnimation { duration: 160 } }

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 8
                        Mono {
                            text: "󰁍"; color: root.fg
                            MouseArea { anchors.fill: parent; anchors.margins: -8
                                cursorShape: Qt.PointingHandCursor; onClicked: control.ccPage = "main" }
                        }
                        Mono { text: "Audio"; font.bold: true; color: root.accent }
                    }

                    // device row used for both output and input lists.
                    component AudioRow: Rectangle {
                        id: arRoot
                        required property var node
                        property bool isInput: false
                        readonly property bool active: isInput
                            ? node === Pipewire.defaultAudioSource
                            : node === Pipewire.defaultAudioSink
                        readonly property string label: node
                            ? (node.description || node.nickname || node.name || "device") : "device"
                        Layout.fillWidth: true
                        height: 34
                        radius: 8
                        color: arHov.hovered ? root.surf(0.08) : "transparent"
                        HoverHandler { id: arHov }
                        RowLayout {
                            anchors.fill: parent
                            anchors.leftMargin: 8; anchors.rightMargin: 8
                            spacing: 8
                            Glyph {
                                text: arRoot.isInput ? "󰍬"
                                    : /headphone|airpod|bluetooth|buds|wh-|wf-/i.test(arRoot.label) ? "󰋋" : "󰓃"
                                color: arRoot.active ? root.good : root.fg
                            }
                            Mono {
                                Layout.fillWidth: true
                                text: arRoot.label
                                color: arRoot.active ? root.good : root.fg
                                elide: Text.ElideRight
                            }
                            Glyph { visible: arRoot.active; text: "󰄬"; color: root.good }
                        }
                        TapHandler {
                            onTapped: {
                                if (arRoot.isInput) Pipewire.preferredDefaultAudioSource = arRoot.node;
                                else Pipewire.preferredDefaultAudioSink = arRoot.node;
                            }
                        }
                    }

                    Mono { text: "OUTPUT"; color: root.dim; font.pixelSize: 10; font.bold: true }
                    ColumnLayout {
                        Layout.fillWidth: true; spacing: 2
                        Repeater {
                            model: root.audioSinks
                            delegate: AudioRow { required property var modelData; node: modelData }
                        }
                        Mono {
                            visible: root.audioSinks.length === 0
                            text: "no output devices"; color: root.dim; font.pixelSize: 11
                        }
                    }

                    Mono { text: "INPUT"; color: root.dim; font.pixelSize: 10; font.bold: true; Layout.topMargin: 6 }
                    ColumnLayout {
                        Layout.fillWidth: true; spacing: 2
                        Repeater {
                            model: root.audioSources
                            delegate: AudioRow { required property var modelData; node: modelData; isInput: true }
                        }
                        Mono {
                            visible: root.audioSources.length === 0
                            text: "no input devices"; color: root.dim; font.pixelSize: 11
                        }
                    }
                    Item { Layout.fillHeight: true }
                }

                // ── backing scanners (nmcli / bluetoothctl) ──────────────────
                ListModel { id: wifiModel }
                Process {
                    id: wifiScan
                    property bool scanning: false
                    property var seen: ({})
                    property var acc: []
                    function rescan() { if (scanning) return; scanning = true; running = true; }
                    function connectTo(ssid, pw) {
                        Quickshell.execDetached(pw.length > 0
                            ? ["nmcli","dev","wifi","connect",ssid,"password",pw]
                            : ["nmcli","dev","wifi","connect",ssid]);
                        wifiRefresh.restart();
                    }
                    // NM's rescan is async (~2s); sleep so the list call returns
                    // the fresh results instead of an empty cache. radio on first.
                    command: ["sh","-c",
                        "nmcli radio wifi on 2>/dev/null; nmcli dev wifi rescan 2>/dev/null; sleep 2; " +
                        "nmcli -t -f IN-USE,SIGNAL,SECURITY,SSID dev wifi list 2>/dev/null"]
                    // accumulate, then swap into the model on exit — no empty flicker
                    onStarted: { acc = []; seen = ({}); }
                    onExited: {
                        wifiModel.clear();
                        for (const n of acc) wifiModel.append(n);
                        scanning = false;
                    }
                    stdout: SplitParser {
                        onRead: line => {
                            if (!line) return;
                            const p = line.split(":");
                            if (p.length < 4) return;
                            const ssid = p.slice(3).join(":").replace(/\\:/g, ":");
                            if (!ssid || wifiScan.seen[ssid]) return;
                            wifiScan.seen[ssid] = true;
                            const sec = p[2];
                            wifiScan.acc.push({
                                ssid: ssid,
                                signal: parseInt(p[1]) || 0,
                                secure: sec.length > 0 && sec !== "--",
                                active: p[0] === "*",
                            });
                        }
                    }
                }
                Timer { id: wifiRefresh; interval: 2500; onTriggered: wifiScan.rescan() }
                // Auto-rescan every 10s while the Wi-Fi page is open and radio on.
                Timer {
                    interval: 10000; repeat: true
                    running: panelWin.rightOpen && control.ccPage === "wifi" && control.wifiRadio
                    triggeredOnStart: true
                    onTriggered: wifiScan.rescan()
                }

                ListModel { id: btModel }
                // Shared: list known devices with a connected marker, no scan.
                readonly property string btListCmd:
                    "conn=$(bluetoothctl devices Connected 2>/dev/null | awk '{print $2}'); " +
                    "bluetoothctl devices 2>/dev/null | while read -r _ mac name; do " +
                    "m=' '; grep -qxF \"$mac\" <<<\"$conn\" && m='*'; " +
                    "printf '%s\\t%s\\t%s\\n' \"$m\" \"$mac\" \"$name\"; done"
                // Accumulate "<mark>\t<mac>\t<name>" lines, skipping unnamed
                // devices (bluetoothctl shows the MAC as the name for those).
                property var btAcc: []
                function btAppend(line) {
                    if (!line) return;
                    const p = line.split("\t");
                    if (p.length < 3) return;
                    const name = p[2];
                    if (/^[0-9A-Fa-f:\-]{11,}$/.test(name)) return; // bare MAC, no real name
                    if (control.btAcc.some(d => d.mac === p[1])) return;
                    control.btAcc.push({ connected: p[0] === "*", mac: p[1], name: name });
                }
                function btSwap() {
                    btModel.clear();
                    for (const d of control.btAcc) btModel.append(d);
                }
                Process {
                    id: btScan
                    property bool scanning: false
                    // Scan (slow) only on open / the rescan button.
                    function rescan() { if (scanning) return; scanning = true; running = true; }
                    function toggle(mac, connected) {
                        Quickshell.execDetached(["bluetoothctl", connected ? "disconnect" : "connect", mac]);
                        btRefresh.restart(); // light relist to update the marker — no rescan
                    }
                    command: ["sh","-c","bluetoothctl power on >/dev/null 2>&1; bluetoothctl --timeout 5 scan on >/dev/null 2>&1; " + control.btListCmd]
                    onStarted: control.btAcc = []
                    onExited: { control.btSwap(); scanning = false; }
                    stdout: SplitParser { onRead: line => control.btAppend(line) }
                }
                // Lightweight relist (no scan) — used after connect/disconnect so
                // the list doesn't churn or empty out while you're tapping.
                Process {
                    id: btRelist
                    command: ["sh","-c", control.btListCmd]
                    onStarted: control.btAcc = []
                    onExited: control.btSwap()
                    stdout: SplitParser { onRead: line => control.btAppend(line) }
                }
                Timer { id: btRefresh; interval: 1500; onTriggered: btRelist.running = true }
                // Auto-rescan every 10s while the Bluetooth page is open and radio on.
                Timer {
                    interval: 10000; repeat: true
                    running: panelWin.rightOpen && control.ccPage === "bluetooth" && control.btRadio
                    triggeredOnStart: true
                    onTriggered: btScan.rescan()
                }
            }
            }
        }
    }

    // Overview chrome — one fullscreen overlay per screen, shown while the
    // compositor's exposé is open (bar.overviewActive).
    Variants {
        // the exposé opens on the focused monitor — its chrome goes there too
        model: root.screenList.filter(s => root.focusedOutput === "" ? s.name === root.primaryScreen
                                                                    : s.name === root.focusedOutput)
        Overview {
            required property var modelData
            screen: modelData
            bar: root
        }
    }
}
