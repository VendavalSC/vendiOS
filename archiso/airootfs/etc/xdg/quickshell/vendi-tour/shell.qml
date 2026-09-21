// vendi-tour — first-login quickstart + hands-on tour.
//
// One floating card over the desktop. Explaining steps show the keys; doing
// steps ("Try it") melt the card into a small pill at the bottom of the
// screen so the desktop is yours, and vendiwm's IPC event stream tells us
// the moment you've actually done the thing (opened a terminal, switched a
// workspace, …) — then the pill checks itself off and swells back into the
// next card. Chords are read live from vendiwm (list-binds), so the tour
// always teaches YOUR keys, overrides included.
//
// Runs on first login (vendi-session, marker ~/.config/vendi/welcomed) and
// any time via `vendi welcome`.   Run: quickshell -n -c vendi-tour
//
// Motion is vsync-driven (Qt's default driver) — it relies on vendiwm pacing
// frame callbacks to the refresh (e326fe2). Without that, callbacks flooded
// in at ~300/s and Qt's per-frame animation stepping ran everything fast.

import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import QtQuick
import QtQuick.Effects

ShellRoot {
    id: root

    // ── theme (follows `vendi theme` live, like the bar) ─────────────────────
    property color accent: "#cba6f7"
    property string themeName: ""
    property string themeAtStart: ""
    readonly property color panel: Qt.rgba(0.05 + accent.r * 0.06,
                                           0.05 + accent.g * 0.06,
                                           0.07 + accent.b * 0.06, 0.97)
    readonly property color fg:   "#cdd6f4"
    readonly property color dim:  "#8a8aa3"
    readonly property color faint: Qt.rgba(1, 1, 1, 0.06)
    readonly property color good: "#a6e3a1"
    readonly property string mono: "JetBrainsMonoNL Nerd Font"
    // Text on the accent (primary buttons): dark unless the accent is dark.
    readonly property color onAccent: (accent.r * 0.3 + accent.g * 0.59 + accent.b * 0.11) > 0.55
        ? "#11111b" : "#f5f5fa"

    readonly property string home: Quickshell.env("HOME")
    readonly property string rt: Quickshell.env("XDG_RUNTIME_DIR") || "/tmp"
    readonly property string ipcPath: rt + "/" + (Quickshell.env("WAYLAND_DISPLAY") || "wayland-1") + ".ipc.sock"

    FileView {
        path: root.home + "/.config/vendi/theme-state"
        watchChanges: true
        onLoaded: {
            const a = /ACCENT_HEX=([0-9a-fA-F]{6})/.exec(text());
            if (a) root.accent = "#" + a[1];
            const t = /THEME=([a-z]+)/.exec(text());
            if (t) {
                root.themeName = t[1];
                if (root.themeAtStart === "") root.themeAtStart = t[1];
            }
        }
        onFileChanged: reload()
    }

    // ── your keys, straight from the compositor ──────────────────────────────
    property var binds: []
    Socket {
        id: bindsSock
        path: root.ipcPath
        connected: true
        onConnectionStateChanged: if (connected) { write('{"cmd":"list-binds"}\n'); flush(); }
        parser: SplitParser {
            onRead: line => {
                try { root.binds = JSON.parse(line).binds || []; } catch (e) {}
                bindsSock.connected = false;
            }
        }
    }
    // First chord whose action satisfies `pred`, else the stock default.
    function chord(pred, fallback) {
        for (const b of root.binds) if (pred(b.action)) return b.chord;
        return fallback;
    }
    readonly property string kTerm:     chord(a => /kitty|alacritty|foot/.test(a) && a.startsWith("spawn"), "super+return")
    readonly property string kLauncher: chord(a => a === "spawn vendi-launcher", "super+space")
    readonly property string kAi:       chord(a => a === "spawn vendi-launcher ai", "super+a")
    readonly property string kDash:     chord(a => a === "spawn vendi-launcher dash", "super+d")
    readonly property string kOverview: chord(a => a === "overview", "super+o")
    readonly property string kWs2:      chord(a => a === "workspace 2", "super+2")
    readonly property string kWs1:      chord(a => a === "workspace 1", "super+1")
    readonly property string kClose:    chord(a => a === "close", "super+q")
    readonly property string kKeys:     chord(a => a === "spawn vendi-menu keys", "super+k")
    readonly property string kLock:     chord(a => a === "spawn vendi-ctl lock", "super+escape")
    readonly property string kFloat:    chord(a => a === "toggle-floating", "super+shift+space")
    readonly property string kShot:     chord(a => a === "spawn vendi shot area --copy", "super+shift+s")
    readonly property string kFocusL:   chord(a => a === "focus-left", "super+left")
    readonly property string kFocusR:   chord(a => a === "focus-right", "super+right")
    readonly property string kMoveL:    chord(a => a === "move-left", "super+shift+left")
    readonly property string kResizeL:  chord(a => a === "resize-left", "super+ctrl+left")

    // ── what you're doing, live ──────────────────────────────────────────────
    property int opened: 0          // windows opened since this step began
    property int closed: 0
    property int lastWinFocus: 0    // last real (non-zero) focused window
    property int focusAtStart: 0
    property int activeWs: 1
    property int wsAtStart: 1
    Socket {
        id: events
        path: root.ipcPath
        connected: true
        onConnectionStateChanged: if (connected) {
            write('{"cmd":"subscribe","events":["window","workspace","overview"]}\n');
            flush();
        }
        parser: SplitParser { onRead: line => root.onEvent(line) }
    }
    function onEvent(line) {
        let e; try { e = JSON.parse(line); } catch (x) { return; }
        switch (e.event) {
        case "window-opened":     root.opened++; break;
        case "window-closed":     root.closed++; break;
        case "window-focused":    if (e.id) root.lastWinFocus = e.id; break;
        case "workspaces-changed": root.activeWs = e.active; break;
        }
        root.check(e);
    }

    // ── the tour ─────────────────────────────────────────────────────────────
    // kind: hero | do (waits for `wait`) | read | themes | tools | done
    readonly property var steps: [
        { kind: "hero" },
        { kind: "do", wait: "open", count: 1, title: "Open a terminal",
          body: "Everything starts here. vendiOS ships kitty, themed to match the rest of the system.",
          keys: [root.kTerm], ask: "to open a terminal" },
        { kind: "do", wait: "open", count: 1, title: "Now open another",
          body: "Watch vendiWM split the space. Windows tile themselves: no dragging, no overlap, no gaps to babysit.",
          keys: [root.kTerm], ask: "to open a second one" },
        { kind: "do", wait: "focus", title: "Move between windows",
          body: "Super plus the arrow keys moves focus to the window in that direction. The accent border follows you.",
          keys: [root.kFocusL, root.kFocusR], ask: "to hop to the other window" },
        { kind: "read", title: "Shape the layout",
          body: "Add shift to the arrows to swap windows, ctrl to resize them. Or just hold super and drag a window: the layout flows around it like liquid. "
              + "Need a window to float? " + root.pretty(root.kFloat) + ".",
          keys: [root.kMoveL, root.kResizeL] },
        { kind: "do", wait: "workspace", title: "Workspaces",
          body: "super plus a number jumps to that workspace; add shift to send the focused window there. This card follows you.",
          keys: [root.kWs2], ask: "to go to workspace 2" },
        { kind: "do", wait: "overview", title: "See everything",
          body: "Overview zooms out to every window on every workspace. Click one to jump to it; Esc to back out.",
          keys: [root.kOverview], ask: "to open the overview" },
        { kind: "read", title: "Find anything",
          body: "The launcher searches apps, files and settings, and does quick math. The local AI answers questions about your system, "
              + "entirely offline, and the dashboard holds your calendar, media and quick toggles.",
          keys: [root.kLauncher, root.kAi, root.kDash] },
        { kind: "read", title: "The island",
          body: "The notch at the top of the screen is alive. Music, calls, recording, battery and notifications grow out of it. "
              + "Click it any time; hover the corners for wifi, bluetooth and power.",
          keys: [] },
        { kind: "themes", title: "Make it yours",
          body: "Pick a theme. Everything recolors live, from the bar and borders to the terminal, editor and GTK apps. "
              + "'dynamic' pulls the colors out of your wallpaper." },
        { kind: "do", wait: "close", title: "Close up",
          body: "Close the focused window. Your terminals are back on workspace 1 (" + root.pretty(root.kWs1) + ") if you're still on 2.",
          keys: [root.kClose], ask: "to close a window" },
        { kind: "tools", title: "The vendi command",
          body: "One command runs the system. Copy any of these into a terminal:" },
        { kind: "done" },
    ]
    property int step: 0
    readonly property var cur: steps[step]
    property bool collapsed: false   // melted into the pill, waiting on you
    property bool celebrating: false // the pill's check-off beat

    function enter(i) {
        root.step = Math.max(0, Math.min(steps.length - 1, i));
        root.opened = 0;
        root.closed = 0;
        root.focusAtStart = root.lastWinFocus;
        root.wsAtStart = root.activeWs;
    }
    function next() { root.collapsed = false; root.enter(root.step + 1); }
    function back() { root.collapsed = false; root.enter(root.step - 1); }
    function tryIt() {
        root.focusAtStart = root.lastWinFocus;
        root.wsAtStart = root.activeWs;
        root.collapsed = true;
    }

    function check(e) {
        const s = root.cur;
        if (s.kind !== "do" || root.celebrating) return;
        let done = false;
        switch (s.wait) {
        case "open":      done = root.opened >= (s.count || 1); break;
        case "close":     done = root.closed >= 1; break;
        case "focus":     done = e.event === "window-focused" && e.id !== 0 && e.id !== root.focusAtStart; break;
        case "workspace": done = e.event === "workspaces-changed" && e.active !== root.wsAtStart; break;
        case "overview":  done = e.event === "overview" && e.active === true; break;
        }
        if (!done) return;
        if (root.collapsed) {
            root.celebrating = true;
            celebrate.restart();
        } else {
            root.next();
        }
    }
    Timer {
        id: celebrate
        interval: 1100
        onTriggered: { root.celebrating = false; root.next(); }
    }

    function finish() {
        Quickshell.execDetached(["sh", "-c", "mkdir -p \"$HOME/.config/vendi\" && touch \"$HOME/.config/vendi/welcomed\""]);
        Qt.quit();
    }

    // "super+shift+left" → ["super", "shift", "←"]
    function keycaps(ch) {
        const names = { "return": "return", "space": "space", "escape": "esc", "left": "←", "right": "→",
                        "up": "↑", "down": "↓", "grave": "`", "period": ".", "comma": ",", "minus": "-",
                        "print": "print", "tab": "tab" };
        return ch.split("+").map(k => names[k] !== undefined ? names[k] : k);
    }
    function pretty(ch) { return root.keycaps(ch).join(" "); }

    readonly property var themes: [
        { n: "dynamic", c: "" }, { n: "mocha", c: "#cba6f7" }, { n: "latte", c: "#8839ef" },
        { n: "gruvbox", c: "#fe8019" }, { n: "nord", c: "#88c0d0" }, { n: "tokyonight", c: "#7aa2f7" },
        { n: "everforest", c: "#a7c080" }, { n: "mono", c: "#ffffff" }, { n: "think", c: "#e2231a" },
    ]
    readonly property var tools: [
        { c: "vendi update",     d: "update everything (a btrfs snapshot is taken first)" },
        { c: "vendi rollback",   d: "boot back into yesterday if an update goes wrong" },
        { c: "vendi dev",        d: "the dev toolchain + vendiVim, themed to match" },
        { c: "vendi game setup", d: "steam, proton, gamemode, mangohud in one go" },
        { c: "vendi theme",      d: "list and switch themes" },
        { c: "vendi report",     d: "bundle logs for a bug report" },
    ]
    property bool hasWallpapers: false
    Process {
        command: ["sh", "-c", "ls \"$HOME/Pictures/Wallpapers\" 2>/dev/null | grep -qiE '\\.(png|jpe?g|webp)$' && echo yes"]
        running: true
        stdout: SplitParser { onRead: l => root.hasWallpapers = l.trim() === "yes" }
    }
    property string copied: ""
    Timer { id: copiedTimer; interval: 1400; onTriggered: root.copied = "" }

    // ── the window ───────────────────────────────────────────────────────────
    PanelWindow {
        id: win
        screen: Quickshell.screens[0]
        anchors { top: true; bottom: true; left: true; right: true }
        color: "transparent"
        WlrLayershell.namespace: "vendi-tour"
        WlrLayershell.layer: WlrLayer.Overlay
        // The card takes the keyboard (Enter / arrows / Esc); the pill gives
        // it back so the desktop is fully yours. Compositor binds work either
        // way — vendiwm resolves them before focus routing.
        WlrLayershell.keyboardFocus: root.collapsed ? WlrKeyboardFocus.None : WlrKeyboardFocus.OnDemand
        exclusiveZone: 0

        // Only the card/pill takes input; everything around it clicks through.
        mask: Region { x: shape.x; y: shape.y; width: shape.width; height: shape.height }

        // A soft dim behind the card; gone while you're trying things out.
        Rectangle {
            anchors.fill: parent
            color: "#000000"
            opacity: 0.32 * Math.max(0, 1 - win.m)
        }

        // ── the shape: card ⇄ pill, one surface morphing ───────────────────
        readonly property real cardW: 700
        readonly property real cardH: 470
        readonly property real pillW: Math.min(width - 48, pillRow.implicitWidth + 44)
        readonly property real pillH: 62

        // The card⇄pill morph is ONE value on ONE spring: 0 = card, 1 = pill.
        // Position, size, corners, the dim and both contents' fades are all
        // derived from it, so it reads as a single fluid motion — separate
        // Behaviors per property each settled on their own clock and the
        // morph came apart into steps (drop, then reshape, then fade).
        property real m: root.collapsed ? 1 : 0
        Behavior on m { SpringAnimation { spring: 4.2; damping: 0.52; mass: 1.0; epsilon: 0.0005 } }
        function lerp(a, b) { return a + (b - a) * m; }
        // 0→1 as m runs lo→hi (clamped), eased — for the content crossfades.
        function ramp(lo, hi) {
            const t = Math.max(0, Math.min(1, (m - lo) / (hi - lo)));
            return t * t * (3 - 2 * t);
        }
        // The pill's width changes with its text (e.g. "Nice."); glide it.
        property real pillWs: pillW
        Behavior on pillWs { NumberAnimation { duration: 320; easing.type: Easing.OutCubic } }

        // The shadow is its own SDF item riding the shape's geometry. It used
        // to be a layer.effect (MultiEffect) on the shape itself — which made
        // Qt reallocate an offscreen texture and re-blur it on every frame of
        // the morph, since the size changes each frame: the morph stuttered.
        RectangularShadow {
            x: shape.x; y: shape.y; width: shape.width; height: shape.height
            radius: shape.radius
            scale: shape.scale
            opacity: shape.opacity * 0.9
            offset: Qt.vector2d(0, 10)
            blur: 36
            color: Qt.rgba(0, 0, 0, 0.55)
        }

        Rectangle {
            id: shape
            x: (win.width - width) / 2
            width: win.lerp(win.cardW, win.pillWs)
            height: Math.max(24, win.lerp(win.cardH, win.pillH))
            y: win.lerp((win.height - win.cardH) / 2, win.height - win.pillH - 36)
            // 28px corners on the card become a full capsule on the pill.
            radius: Math.min(height / 2, win.lerp(28, win.pillH / 2))
            color: root.panel
            border.width: 1
            border.color: root.celebrating ? Qt.rgba(root.good.r, root.good.g, root.good.b, 0.6) : root.faint
            Behavior on border.color { ColorAnimation { duration: 200 } }

            // Pop-in on launch.
            scale: 0.92
            opacity: 0
            Component.onCompleted: { scale = 1; opacity = 1; }
            Behavior on scale   { NumberAnimation { duration: 420; easing.type: Easing.OutBack; easing.overshoot: 1.4 } }
            Behavior on opacity { NumberAnimation { duration: 260 } }

            // Keyboard: Enter = primary, ←/→ = back/next, Esc = pill→card, card→quit.
            focus: true
            Keys.onPressed: ev => {
                if (ev.key === Qt.Key_Escape) {
                    if (root.collapsed) root.collapsed = false; else root.finish();
                } else if (ev.key === Qt.Key_Return || ev.key === Qt.Key_Enter) {
                    primary.activate();
                } else if (ev.key === Qt.Key_Right && root.cur.kind !== "do") {
                    if (root.step < root.steps.length - 1) root.next();
                } else if (ev.key === Qt.Key_Left) {
                    if (root.step > 0) root.back();
                } else return;
                ev.accepted = true;
            }

            // ═══ card ═══════════════════════════════════════════════════════
            Item {
                id: card
                anchors.fill: parent
                // Rides the morph: fades over its first third, sinking and
                // shrinking a little toward the pill instead of popping out.
                opacity: 1 - win.ramp(0.0, 0.35)
                visible: opacity > 0.01
                scale: 1 - 0.06 * win.ramp(0.0, 0.5)
                transformOrigin: Item.Bottom

                // Page content — re-keyed per step so each page fades/slides in.
                Item {
                    id: page
                    anchors { left: parent.left; right: parent.right; top: parent.top; bottom: footer.top }
                    anchors.margins: 40
                    anchors.bottomMargin: 16

                    property int shown: root.step
                    opacity: 1
                    Connections {
                        target: root
                        function onStepChanged() { pageIn.restart(); }
                    }
                    SequentialAnimation {
                        id: pageIn
                        ParallelAnimation {
                            NumberAnimation { target: page; property: "opacity"; from: 0; to: 1; duration: 280; easing.type: Easing.OutCubic }
                            NumberAnimation { target: pageShift; property: "x"; from: 18; to: 0; duration: 360; easing.type: Easing.OutCubic }
                        }
                    }
                    transform: Translate { id: pageShift; x: 0 }

                    // ── hero ────────────────────────────────────────────────
                    Item {
                        anchors.fill: parent
                        visible: root.cur.kind === "hero"
                        Blob {
                            id: heroBlob
                            anchors.horizontalCenter: parent.horizontalCenter
                            y: 6
                            size: 118
                            tint: root.accent
                        }
                        Column {
                            anchors { horizontalCenter: parent.horizontalCenter; top: heroBlob.bottom; topMargin: 30 }
                            spacing: 12
                            Text {
                                anchors.horizontalCenter: parent.horizontalCenter
                                text: "Welcome to vendiOS"
                                color: root.fg; font.family: root.mono; font.pixelSize: 30; font.weight: Font.DemiBold
                            }
                            Text {
                                anchors.horizontalCenter: parent.horizontalCenter
                                width: 520; horizontalAlignment: Text.AlignHCenter; wrapMode: Text.WordWrap
                                text: "A two-minute, hands-on tour: you press real keys, it notices. "
                                    + "Leave anytime — vendi welcome brings it back."
                                color: root.dim; font.family: root.mono; font.pixelSize: 14; lineHeight: 1.3
                            }
                        }
                    }

                    // ── explain / do ────────────────────────────────────────
                    Column {
                        anchors { left: parent.left; right: parent.right; top: parent.top }
                        spacing: 16
                        visible: ["do", "read", "themes", "tools"].indexOf(root.cur.kind) >= 0

                        Text {
                            text: ({ "do": "TRY IT", "read": "GOOD TO KNOW", "themes": "MAKE IT YOURS", "tools": "TOOLBOX" })[root.cur.kind] || ""
                            color: root.accent; font.family: root.mono; font.pixelSize: 11; font.letterSpacing: 2.5
                            font.weight: Font.DemiBold
                        }
                        Text {
                            text: root.cur.title || ""
                            color: root.fg; font.family: root.mono; font.pixelSize: 27; font.weight: Font.DemiBold
                        }
                        Text {
                            width: parent.width; wrapMode: Text.WordWrap
                            text: root.cur.body || ""
                            color: root.dim; font.family: root.mono; font.pixelSize: 14; lineHeight: 1.35
                        }
                        Item { width: 1; height: 6 }
                        // key chords
                        Flow {
                            width: parent.width
                            spacing: 18
                            visible: (root.cur.keys || []).length > 0
                            Repeater {
                                model: root.cur.keys || []
                                Chord { keys: root.keycaps(modelData); big: true }
                            }
                        }

                        // themes
                        Flow {
                            width: parent.width
                            spacing: 10
                            visible: root.cur.kind === "themes"
                            Repeater {
                                model: root.themes
                                Chip {
                                    label: modelData.n
                                    swatch: modelData.c === "" ? root.accent : modelData.c
                                    rainbow: modelData.c === ""
                                    active: root.themeName === modelData.n
                                    onClicked: Quickshell.execDetached(["vendi", "theme", modelData.n])
                                }
                            }
                        }
                        Flow {
                            width: parent.width
                            spacing: 10
                            visible: root.cur.kind === "themes"
                            Chip { label: "light / dark"; glyph: "◐"; onClicked: Quickshell.execDetached(["vendi", "appearance", "toggle"]) }
                            Chip {
                                label: "next wallpaper"; glyph: "▣"; visible: root.hasWallpapers
                                onClicked: Quickshell.execDetached(["vendi-ctl", "wallpaper", "next"])
                            }
                            Chip { label: "night light"; glyph: "☾"; onClicked: Quickshell.execDetached(["vendi", "night", "toggle"]) }
                            Chip {
                                label: "undo: " + root.themeAtStart; glyph: "↺"
                                visible: root.themeAtStart !== "" && root.themeName !== root.themeAtStart
                                onClicked: Quickshell.execDetached(["vendi", "theme", root.themeAtStart])
                            }
                        }

                        // tools
                        Column {
                            width: parent.width
                            spacing: 6
                            visible: root.cur.kind === "tools"
                            Repeater {
                                model: root.tools
                                Rectangle {
                                    width: parent.width; height: 36; radius: 10
                                    color: toolHover.hovered ? Qt.rgba(1, 1, 1, 0.07) : Qt.rgba(1, 1, 1, 0.03)
                                    Behavior on color { ColorAnimation { duration: 140 } }
                                    Text {
                                        id: toolCmd
                                        anchors { left: parent.left; leftMargin: 14; verticalCenter: parent.verticalCenter }
                                        width: 170
                                        text: modelData.c
                                        color: root.accent; font.family: root.mono; font.pixelSize: 13
                                    }
                                    Text {
                                        anchors { left: toolCmd.right; right: copyTag.left; rightMargin: 10; verticalCenter: parent.verticalCenter }
                                        text: modelData.d; elide: Text.ElideRight
                                        color: root.dim; font.family: root.mono; font.pixelSize: 12
                                    }
                                    Text {
                                        id: copyTag
                                        anchors { right: parent.right; rightMargin: 14; verticalCenter: parent.verticalCenter }
                                        text: root.copied === modelData.c ? "copied ✓" : "copy"
                                        color: root.copied === modelData.c ? root.good : (toolHover.hovered ? root.fg : root.dim)
                                        font.family: root.mono; font.pixelSize: 12
                                    }
                                    HoverHandler { id: toolHover; cursorShape: Qt.PointingHandCursor }
                                    TapHandler {
                                        onTapped: {
                                            Quickshell.execDetached(["wl-copy", modelData.c]);
                                            root.copied = modelData.c;
                                            copiedTimer.restart();
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // ── done ────────────────────────────────────────────────
                    Item {
                        anchors.fill: parent
                        visible: root.cur.kind === "done"
                        Blob {
                            id: doneBlob
                            anchors.horizontalCenter: parent.horizontalCenter
                            y: 0
                            size: 78
                            tint: root.good
                        }
                        Column {
                            anchors { horizontalCenter: parent.horizontalCenter; top: doneBlob.bottom; topMargin: 22 }
                            spacing: 18
                            Text {
                                anchors.horizontalCenter: parent.horizontalCenter
                                text: "You're set."
                                color: root.fg; font.family: root.mono; font.pixelSize: 28; font.weight: Font.DemiBold
                            }
                            Column {
                                anchors.horizontalCenter: parent.horizontalCenter
                                spacing: 10
                                Repeater {
                                    model: [
                                        { k: root.kKeys, d: "every shortcut, searchable" },
                                        { k: root.kLauncher, d: "launch anything" },
                                        { k: root.kLock, d: "lock the screen" },
                                    ]
                                    Row {
                                        spacing: 16
                                        Item {
                                            width: 210; height: 30
                                            Chord { anchors.right: parent.right; anchors.verticalCenter: parent.verticalCenter; keys: root.keycaps(modelData.k) }
                                        }
                                        Text {
                                            anchors.verticalCenter: parent.verticalCenter
                                            text: modelData.d; color: root.dim; font.family: root.mono; font.pixelSize: 14
                                        }
                                    }
                                }
                            }
                            Text {
                                anchors.horizontalCenter: parent.horizontalCenter
                                text: "vendi welcome  brings this tour back"
                                color: Qt.rgba(root.dim.r, root.dim.g, root.dim.b, 0.7); font.family: root.mono; font.pixelSize: 12
                            }
                        }
                    }
                }

                // ── footer: progress + buttons ──────────────────────────────
                Item {
                    id: footer
                    anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
                    anchors.margins: 28
                    height: 42

                    Row {
                        anchors { left: parent.left; verticalCenter: parent.verticalCenter }
                        spacing: 6
                        Repeater {
                            model: root.steps.length
                            Rectangle {
                                anchors.verticalCenter: parent.verticalCenter
                                width: index === root.step ? 18 : 6; height: 6; radius: 3
                                color: index === root.step ? root.accent
                                     : index < root.step ? Qt.rgba(root.accent.r, root.accent.g, root.accent.b, 0.45)
                                     : Qt.rgba(1, 1, 1, 0.14)
                                Behavior on width { NumberAnimation { duration: 260; easing.type: Easing.OutCubic } }
                                Behavior on color { ColorAnimation { duration: 200 } }
                            }
                        }
                    }

                    Row {
                        anchors { right: parent.right; verticalCenter: parent.verticalCenter }
                        spacing: 10
                        Button {
                            label: root.cur.kind === "hero" ? "Skip tour" : root.cur.kind === "do" ? "Skip step" : "Back"
                            visible: root.cur.kind !== "done"
                            onClicked: {
                                if (root.cur.kind === "hero") root.finish();
                                else if (root.cur.kind === "do") root.next();
                                else root.back();
                            }
                        }
                        Button {
                            id: primary
                            filled: true
                            label: root.cur.kind === "hero" ? "Start the tour"
                                 : root.cur.kind === "do" ? "Try it"
                                 : root.cur.kind === "done" ? "Finish"
                                 : "Next"
                            function activate() {
                                if (root.collapsed) return;
                                if (root.cur.kind === "do") root.tryIt();
                                else if (root.cur.kind === "done") root.finish();
                                else root.next();
                            }
                            onClicked: activate()
                        }
                    }
                }
            }

            // ═══ pill ═══════════════════════════════════════════════════════
            Item {
                anchors.fill: parent
                opacity: win.ramp(0.6, 0.95)
                visible: opacity > 0.01

                Row {
                    id: pillRow
                    anchors.centerIn: parent
                    spacing: 14

                    // Pulsing dot while waiting; a check once you've done it.
                    Item {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 22; height: 22
                        Rectangle {
                            anchors.centerIn: parent
                            width: 22; height: 22; radius: 11
                            color: "transparent"
                            border.width: 2
                            border.color: root.accent
                            visible: !root.celebrating
                            SequentialAnimation on scale {
                                running: root.collapsed && !root.celebrating
                                loops: Animation.Infinite
                                NumberAnimation { from: 0.55; to: 1.15; duration: 900; easing.type: Easing.OutCubic }
                                NumberAnimation { to: 0.55; duration: 500; easing.type: Easing.InCubic }
                            }
                            opacity: 2 - scale * 1.5
                        }
                        Rectangle {
                            anchors.centerIn: parent
                            width: 10; height: 10; radius: 5
                            color: root.accent
                            visible: !root.celebrating
                        }
                        Rectangle {
                            anchors.centerIn: parent
                            width: 22; height: 22; radius: 11
                            color: root.good
                            visible: root.celebrating
                            scale: root.celebrating ? 1 : 0.3
                            Behavior on scale { NumberAnimation { duration: 380; easing.type: Easing.OutBack; easing.overshoot: 2.2 } }
                            Text {
                                anchors.centerIn: parent
                                text: "✓"; color: "#11111b"; font.pixelSize: 13; font.weight: Font.Bold
                            }
                        }
                    }

                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        visible: !root.celebrating
                        text: "Press"
                        color: root.fg; font.family: root.mono; font.pixelSize: 14
                    }
                    Row {
                        anchors.verticalCenter: parent.verticalCenter
                        visible: !root.celebrating
                        spacing: 10
                        Repeater {
                            model: root.cur.kind === "do" ? root.cur.keys : []
                            Chord { keys: root.keycaps(modelData) }
                        }
                    }
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        text: root.celebrating ? "Nice." : (root.cur.ask || "")
                        color: root.celebrating ? root.good : root.fg
                        font.family: root.mono; font.pixelSize: 14
                    }
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        visible: !root.celebrating
                        text: "back"
                        color: backHover.hovered ? root.fg : root.dim
                        font.family: root.mono; font.pixelSize: 12
                        leftPadding: 6
                        HoverHandler { id: backHover; cursorShape: Qt.PointingHandCursor }
                        TapHandler { onTapped: root.collapsed = false }
                    }
                }
            }
        }
    }

    // ── pieces ───────────────────────────────────────────────────────────────

    // A chord as keycaps joined by thin "+" marks.
    component Chord: Row {
        id: chordRow
        property var keys: []
        property bool big: false
        spacing: 6
        Repeater {
            model: chordRow.keys
            Row {
                spacing: 6
                Text {
                    anchors.verticalCenter: parent.verticalCenter
                    visible: index > 0
                    text: "+"; color: Qt.rgba(1, 1, 1, 0.3)
                    font.family: root.mono; font.pixelSize: chordRow.big ? 13 : 11
                }
                Rectangle {
                    anchors.verticalCenter: parent.verticalCenter
                    height: chordRow.big ? 34 : 26
                    width: Math.max(height, capText.implicitWidth + (chordRow.big ? 22 : 16))
                    radius: chordRow.big ? 9 : 7
                    color: Qt.rgba(1, 1, 1, 0.07)
                    border.width: 1
                    border.color: Qt.rgba(1, 1, 1, 0.10)
                    // the keycap's lip
                    Rectangle {
                        anchors { left: parent.left; right: parent.right; bottom: parent.bottom; margins: 1 }
                        height: 3; radius: parent.radius
                        color: Qt.rgba(0, 0, 0, 0.35)
                    }
                    Text {
                        id: capText
                        anchors.centerIn: parent
                        anchors.verticalCenterOffset: -1
                        text: modelData
                        color: root.fg; font.family: root.mono
                        font.pixelSize: chordRow.big ? 14 : 12
                    }
                }
            }
        }
    }

    component Button: Rectangle {
        id: btn
        property string label: ""
        property bool filled: false
        signal clicked()
        height: 40
        width: btnText.implicitWidth + 40
        radius: height / 2
        color: filled ? (btnHover.hovered ? Qt.lighter(root.accent, 1.08) : root.accent)
                      : (btnHover.hovered ? Qt.rgba(1, 1, 1, 0.10) : Qt.rgba(1, 1, 1, 0.05))
        Behavior on color { ColorAnimation { duration: 140 } }
        Behavior on width { NumberAnimation { duration: 200; easing.type: Easing.OutCubic } }
        scale: btnTap.pressed ? 0.96 : 1
        Behavior on scale { NumberAnimation { duration: 120 } }
        Text {
            id: btnText
            anchors.centerIn: parent
            text: btn.label
            color: btn.filled ? root.onAccent : root.fg
            font.family: root.mono; font.pixelSize: 14
            font.weight: btn.filled ? Font.DemiBold : Font.Normal
        }
        HoverHandler { id: btnHover; cursorShape: Qt.PointingHandCursor }
        TapHandler { id: btnTap; onTapped: btn.clicked() }
    }

    component Chip: Rectangle {
        id: chip
        property string label: ""
        property color swatch: "transparent"
        property bool rainbow: false
        property string glyph: ""
        property bool active: false
        signal clicked()
        height: 36
        width: chipRow.implicitWidth + 28
        radius: 18
        color: active ? Qt.rgba(root.accent.r, root.accent.g, root.accent.b, 0.18)
             : chipHover.hovered ? Qt.rgba(1, 1, 1, 0.09) : Qt.rgba(1, 1, 1, 0.04)
        border.width: 1
        border.color: active ? Qt.rgba(root.accent.r, root.accent.g, root.accent.b, 0.7) : Qt.rgba(1, 1, 1, 0.07)
        Behavior on color { ColorAnimation { duration: 160 } }
        Behavior on border.color { ColorAnimation { duration: 160 } }
        scale: chipTap.pressed ? 0.95 : 1
        Behavior on scale { NumberAnimation { duration: 120 } }
        Row {
            id: chipRow
            anchors.centerIn: parent
            spacing: 9
            Rectangle {
                anchors.verticalCenter: parent.verticalCenter
                visible: chip.glyph === ""
                width: 14; height: 14; radius: 7
                color: chip.swatch
                gradient: chip.rainbow ? rainbowGrad : null
                Gradient {
                    id: rainbowGrad
                    orientation: Gradient.Horizontal
                    GradientStop { position: 0.0; color: "#f38ba8" }
                    GradientStop { position: 0.5; color: "#a6e3a1" }
                    GradientStop { position: 1.0; color: "#89b4fa" }
                }
            }
            Text {
                anchors.verticalCenter: parent.verticalCenter
                visible: chip.glyph !== ""
                text: chip.glyph; color: root.accent
                font.family: root.mono; font.pixelSize: 14
            }
            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: chip.label
                color: chip.active ? root.fg : Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.85)
                font.family: root.mono; font.pixelSize: 13
            }
        }
        HoverHandler { id: chipHover; cursorShape: Qt.PointingHandCursor }
        TapHandler { id: chipTap; onTapped: chip.clicked() }
    }

    // vendiOS's living blob (the lock screen's, in colour): a body plus four
    // same-colour satellites breathing on two periods, slowly orbiting.
    component Blob: Item {
        id: blob
        property real size: 110
        property color tint: root.accent
        width: size; height: size
        Rectangle {
            anchors.centerIn: parent
            width: blob.size * 1.5; height: width; radius: width / 2
            color: blob.tint; opacity: 0.10
        }
        Item {
            id: blobSats
            anchors.fill: parent
            property real wob: 0
            property real wob2: 0
            RotationAnimation on rotation { from: 0; to: 360; duration: 9000; loops: Animation.Infinite }
            SequentialAnimation on wob {
                loops: Animation.Infinite
                NumberAnimation { to: 1; duration: 1500; easing.type: Easing.InOutSine }
                NumberAnimation { to: 0; duration: 1500; easing.type: Easing.InOutSine }
            }
            SequentialAnimation on wob2 {
                loops: Animation.Infinite
                NumberAnimation { to: 1; duration: 2300; easing.type: Easing.InOutSine }
                NumberAnimation { to: 0; duration: 2300; easing.type: Easing.InOutSine }
            }
            Rectangle {
                anchors.centerIn: parent
                width: blob.size * 0.84; height: width; radius: width / 2
                color: blob.tint
            }
            Repeater {
                model: [
                    { ang: 0.0, rad: 0.42, off: 0.105, w: 1 },
                    { ang: 2.1, rad: 0.37, off: 0.140, w: -1 },
                    { ang: 4.2, rad: 0.40, off: 0.120, w: 1 },
                    { ang: 5.4, rad: 0.34, off: 0.150, w: -1 },
                ]
                Rectangle {
                    property real bulge: modelData.off + (modelData.w > 0 ? blobSats.wob : blobSats.wob2) * 0.04
                    property real breathe: modelData.rad + (modelData.w > 0 ? blobSats.wob2 : blobSats.wob) * 0.025
                    width: blob.size * breathe * 2; height: width; radius: width / 2
                    color: blob.tint
                    x: blob.size / 2 + Math.cos(modelData.ang) * blob.size * bulge - width / 2
                    y: blob.size / 2 + Math.sin(modelData.ang) * blob.size * bulge - height / 2
                }
            }
        }
    }
}
