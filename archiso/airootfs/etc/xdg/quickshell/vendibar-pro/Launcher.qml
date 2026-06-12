// vendi spotlight — the quickshell launcher (lives inside vendibar-pro, so
// it opens instantly and shares the theme). Opens as a bare search bar.
//
//   plain text   fuzzy app search · google search row at the bottom
//   2+2*8        inline calculator (Enter copies the result)
//   :fire        emoji search (Enter copies)
//   f notes      file search under ~ (fd, Enter opens)
//   w term       open-window switcher (Enter focuses, via vendi-ctl)
//   >cmd         run a shell command
//
// Actions mode (super+alt+space) replaces the GTK vendi-menu on the pro
// bar: the same nested system menu — capture, theme, wallpaper, settings,
// connect, install, power — rendered in the spotlight card. Type to filter,
// Backspace on empty input goes up a level.
//
// IPC: quickshell -c vendibar-pro ipc call launcher toggle | actions

import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import Quickshell.Widgets
import QtQuick
import QtQuick.Layouts

PanelWindow {
    id: win

    property bool open: false
    property string mode: "search"      // search | actions
    property var crumb: []              // actions drill-down stack
    // Theme handles, wired from shell.qml.
    property color accent: "#cba6f7"
    property color panel: "#0b0b12"
    property color fg: "#cdd6f4"
    property color dim: "#717189"
    property string mono: "JetBrainsMonoNL Nerd Font"

    function toggle() {
        if (open && mode === "actions") { mode = "search"; query.text = ""; return; }
        open = !open;
        if (open) {
            mode = "search";
            crumb = [];
            query.text = "";
            list.currentIndex = 0;
            winRefresh.running = true;
        }
    }
    function actions() {
        if (open && mode === "actions") { open = false; return; }
        open = true;
        mode = "actions";
        crumb = [];
        query.text = "";
        list.currentIndex = 0;
        wpRefresh.running = true;
    }

    visible: open
    color: "transparent"
    anchors { top: true; bottom: true; left: true; right: true }
    WlrLayershell.namespace: "vendi-spotlight"
    WlrLayershell.layer: WlrLayer.Overlay
    WlrLayershell.keyboardFocus: open ? WlrKeyboardFocus.Exclusive : WlrKeyboardFocus.None
    exclusionMode: ExclusionMode.Ignore

    // ── result providers ─────────────────────────────────────────────────────

    function sh(cmd) {
        return () => Quickshell.execDetached(["sh", "-c", cmd]);
    }
    readonly property string floatTerm: "alacritty --class vendi-float -e "

    // Open windows, refreshed each time the launcher opens.
    property var openWindows: []
    Process {
        id: winRefresh
        command: ["env", "VENDI_JSON=1", "vendi-ctl", "list-windows"]
        stdout: StdioCollector {
            onStreamFinished: {
                try { win.openWindows = JSON.parse(text).windows || []; }
                catch (e) { win.openWindows = []; }
            }
        }
    }

    // Wallpaper library, refreshed when actions mode opens.
    property var wallpaperFiles: []
    Process {
        id: wpRefresh
        command: ["sh", "-c", "ls -1 \"$HOME\"/Pictures/Wallpapers/*.png \"$HOME\"/Pictures/Wallpapers/*.jpg \"$HOME\"/Pictures/Wallpapers/*.jpeg \"$HOME\"/Pictures/Wallpapers/*.webp 2>/dev/null"]
        stdout: StdioCollector {
            onStreamFinished: win.wallpaperFiles = text.trim() ? text.trim().split("\n") : []
        }
    }

    // File search (`f ` prefix): debounced fd under ~.
    property var fileResults: []
    Timer {
        id: fileDebounce
        interval: 220
        onTriggered: {
            const needle = query.text.slice(1).trim().replace(/['"\\]/g, "");
            if (!needle) { win.fileResults = []; return; }
            fileSearch.command = ["sh", "-c",
                "fd --max-results 10 -i '" + needle + "' \"$HOME\" 2>/dev/null"];
            fileSearch.running = true;
        }
    }
    Process {
        id: fileSearch
        stdout: StdioCollector {
            onStreamFinished: win.fileResults = text.trim() ? text.trim().split("\n") : []
        }
    }

    // Small curated emoji set — enough for chat duty.
    readonly property var emoji: [
        ["😀","grin happy smile"],["😂","joy laugh tears"],["🤣","rofl laugh"],["😊","blush smile"],
        ["😍","heart eyes love"],["😘","kiss"],["😎","cool sunglasses"],["🤔","think hmm"],
        ["😴","sleep tired"],["😭","cry sob"],["😡","angry mad"],["🥺","pleading puppy"],
        ["😅","sweat laugh"],["🙃","upside down"],["😉","wink"],["🤯","mind blown"],
        ["🥳","party celebrate"],["😇","angel halo"],["💀","skull dead"],["🤡","clown"],
        ["👍","thumbs up yes"],["👎","thumbs down no"],["👏","clap applause"],["🙏","pray please thanks"],
        ["🤝","handshake deal"],["💪","muscle strong"],["👌","ok perfect"],["✌️","peace victory"],
        ["🖕","middle finger"],["👋","wave hello bye"],["🤌","pinched fingers chef"],["✊","fist"],
        ["❤️","heart love red"],["🧡","heart orange"],["💛","heart yellow"],["💚","heart green"],
        ["💙","heart blue"],["💜","heart purple"],["🖤","heart black"],["💔","broken heart"],
        ["✨","sparkles magic"],["🔥","fire lit hot"],["⭐","star"],["⚡","zap lightning bolt"],
        ["🎯","bullseye target"],["🎉","tada party confetti"],["🎊","confetti ball"],["💯","100 hundred percent"],
        ["🚀","rocket launch ship"],["🌙","moon night"],["☀️","sun day"],["🌈","rainbow"],
        ["🍕","pizza food"],["🍺","beer cheers"],["☕","coffee"],["🍰","cake dessert"],
        ["🐱","cat kitty"],["🐶","dog puppy"],["🦊","fox"],["🐢","turtle"],
        ["💻","laptop computer"],["⌨️","keyboard"],["🖥️","desktop pc"],["📱","phone"],
        ["🎵","music note"],["🎧","headphones"],["🎮","game controller"],["📷","camera photo"],
        ["💡","idea bulb"],["🔒","lock secure"],["🔑","key password"],["⚙️","gear settings"],
        ["📝","memo note write"],["📦","package box"],["🐛","bug insect"],["🧠","brain smart"],
        ["💸","money fly"],["💎","gem diamond"],["⏰","alarm clock time"],["🗑️","trash delete"]
    ]

    // Calculator: digits/operators only, evaluated in a throwaway Function.
    function calc(expr) {
        if (!/^[0-9+\-*/().%^ ,e]+$/.test(expr) || !/[0-9]/.test(expr)) return null;
        try {
            const r = new Function("return (" + expr.replace(/\^/g, "**").replace(/,/g, ".") + ")")();
            if (typeof r !== "number" || !isFinite(r)) return null;
            return String(Math.round(r * 1e9) / 1e9);
        } catch (e) { return null; }
    }

    function fuzzy(hay, needle) {
        hay = hay.toLowerCase(); needle = needle.toLowerCase();
        if (hay.startsWith(needle)) return 0;
        const i = hay.indexOf(needle);
        if (i >= 0) return 1 + i / 100;
        let j = 0;
        for (const c of hay) if (c === needle[j]) j++;
        return j === needle.length ? 3 : -1;
    }

    // The nested system menu — the quickshell twin of `vendi-menu actions`.
    function actionsTree() {
        const pick = "grim -g \"$(slurp -p)\" -t ppm - | python3 -c 'import sys;d=sys.stdin.buffer.read();print(\"#%02x%02x%02x\"%(d[-3],d[-2],d[-1]))' | wl-copy";
        return [
            { glyph: "\u{f0100}", title: "Capture", children: [
                { glyph: "\u{f0c4e}", title: "Region to clipboard", act: sh("grim -g \"$(slurp)\" - | wl-copy") },
                { glyph: "\u{f1077}", title: "Region to file", act: sh("mkdir -p ~/Pictures && grim -g \"$(slurp)\" ~/Pictures/screenshot-$(date +%s).png") },
                { glyph: "\u{f0e51}", title: "Screen to file", act: sh("mkdir -p ~/Pictures && grim ~/Pictures/screenshot-$(date +%s).png") },
                { glyph: "\u{f020a}", title: "Color picker", hint: "hex → clipboard", act: sh(pick) },
            ]},
            { glyph: "\u{f03d8}", title: "Theme", children:
                ["mocha", "latte", "gruvbox", "mono", "think", "dynamic"].map(t => ({
                    glyph: "\u{f03d8}", title: t.charAt(0).toUpperCase() + t.slice(1),
                    hint: t === "dynamic" ? "from wallpaper" : "",
                    act: sh("vendi theme " + t),
                }))
            },
            { glyph: "\u{f0e09}", title: "Wallpaper", children: [
                { glyph: "\u{f0598}", title: "Shuffle", act: sh("vendi-ctl wallpaper random") },
                { glyph: "\u{f06e8}", title: "Default gradient", act: sh("vendi-ctl wallpaper default") },
            ].concat(win.wallpaperFiles.map(p => ({
                glyph: "\u{f0e09}",
                title: p.split("/").pop().replace(/\.[^.]+$/, ""),
                act: sh("vendi-ctl wallpaper '" + p + "'"),
            })))},
            { glyph: "\u{f0493}", title: "Settings", children: [
                { glyph: "\u{f035b}", title: "Bar: minimal", act: sh("vendi bar classic") },
                { glyph: "\u{f035c}", title: "Bar: pro", act: sh("vendi bar pro") },
                { glyph: "\u{f0493}", title: "WM config", act: sh(win.floatTerm + "sh -c 'mkdir -p ~/.config/vendi && ${EDITOR:-vim} ~/.config/vendi/vendiwm.kdl'") },
                { glyph: "\u{f0450}", title: "Reload session", act: sh("pkill -x vendiwm") },
            ]},
            { glyph: "\u{f05a9}", title: "Connect", children: [
                { glyph: "\u{f05a9}", title: "Wi-Fi", act: sh(win.floatTerm + "vendi wifi") },
                { glyph: "\u{f00af}", title: "Bluetooth", act: sh(win.floatTerm + "vendi bt") },
                { glyph: "\u{f057e}", title: "Audio output", act: sh(win.floatTerm + "vendi audio") },
                { glyph: "\u{f0210}", title: "Power profile", act: sh(win.floatTerm + "vendi power") },
            ]},
            { glyph: "\u{f0419}", title: "Install", children: [
                { glyph: "\u{f0419}", title: "Install package", act: sh(win.floatTerm + "sh -c 'pacman -Slq | fzf --multi --prompt=\"install> \" --preview \"pacman -Si {}\" | xargs -ro sudo pacman -S; printf \"\\n  done — any key closes \"; read -rsn1'") },
                { glyph: "\u{f0376}", title: "Remove package", act: sh(win.floatTerm + "sh -c 'pacman -Qq | fzf --multi --prompt=\"remove> \" --preview \"pacman -Qi {}\" | xargs -ro sudo pacman -Rns; printf \"\\n  done — any key closes \"; read -rsn1'") },
                { glyph: "\u{f06b0}", title: "Update system", act: sh(win.floatTerm + "sh -c 'sudo vendi update; printf \"\\n  done — any key closes \"; read -rsn1'") },
            ]},
            { glyph: "\u{f0425}", title: "Power", children: [
                { glyph: "\u{f033e}", title: "Lock", act: sh("vendi-ctl lock") },
                { glyph: "\u{f04b2}", title: "Suspend", act: sh("systemctl suspend") },
                { glyph: "\u{f0709}", title: "Restart", act: sh("systemctl reboot") },
                { glyph: "\u{f0425}", title: "Shut down", act: sh("systemctl poweroff") },
            ]},
        ];
    }

    // The unified result list: [{glyph|icon, title, hint, act, stay}]
    property var results: {
        const q = query.text;

        if (mode === "actions") {
            const page = crumb.length ? crumb[crumb.length - 1].children : actionsTree();
            return page
                .filter(n => !q || fuzzy(n.title, q) >= 0)
                .map(n => n.children
                    ? { glyph: n.glyph, title: n.title, hint: "›", stay: true,
                        act: () => { win.crumb = win.crumb.concat([n]); query.text = ""; list.currentIndex = 0; } }
                    : n);
        }

        const out = [];
        if (q.startsWith(">")) {
            const cmd = q.slice(1).trim();
            if (cmd) out.push({ glyph: "\u{eb32}", title: cmd, hint: "run command",
                                act: sh(cmd) });
            return out;
        }
        if (q.startsWith(":")) {
            const needle = q.slice(1).trim().toLowerCase();
            for (const [e, words] of win.emoji) {
                if (!needle || words.includes(needle) || words.split(" ").some(w => w.startsWith(needle))) {
                    out.push({ glyph: e, title: words.split(" ").slice(0, 3).join(" "), hint: "copy emoji",
                               act: sh("wl-copy '" + e + "'") });
                    if (out.length >= 24) break;
                }
            }
            return out;
        }
        if (q.startsWith("f ")) {
            for (const p of win.fileResults) {
                out.push({ glyph: "\u{f0214}", title: p.replace(/^\/home\/[^/]+/, "~"), hint: "open",
                           act: sh("xdg-open '" + p.replace(/'/g, "'\\''") + "'") });
            }
            return out;
        }
        if (q.startsWith("w ") || q === "w") {
            const needle = q.slice(1).trim();
            for (const w of win.openWindows) {
                if (!needle || fuzzy(w.title, needle) >= 0) {
                    out.push({ glyph: "\u{f05af}", title: w.title || "(untitled)",
                               hint: "workspace " + w.workspace,
                               act: () => Quickshell.execDetached(["vendi-ctl", "focus", String(w.id)]) });
                }
            }
            return out;
        }
        // Spotlight starts as a bare bar — nothing until you type.
        if (!q) return out;

        const c = calc(q);
        if (c !== null) {
            out.push({ glyph: "\u{f0349}", title: c, hint: "copy result",
                       act: sh("wl-copy '" + c + "'") });
        }
        const apps = DesktopEntries.applications.values
            .filter(a => !a.noDisplay)
            .map(a => ({ a: a, s: fuzzy(a.name, q) }))
            .filter(x => x.s >= 0)
            .sort((x, y) => x.s - y.s || x.a.name.localeCompare(y.a.name))
            .slice(0, 7);
        for (const { a } of apps) {
            out.push({ icon: a.icon, title: a.name, hint: a.genericName || "",
                       act: () => a.execute() });
        }
        // Always offer the web as the last resort.
        out.push({ glyph: "\u{f0349}", title: "Search Google for \u{201c}" + q + "\u{201d}", hint: "web",
                   act: sh("xdg-open 'https://www.google.com/search?q=" + encodeURIComponent(q) + "'") });
        return out;
    }

    onResultsChanged: if (list.currentIndex >= results.length) list.currentIndex = 0

    function activate() {
        const r = results[list.currentIndex];
        if (!r) return;
        r.act();
        if (!r.stay) win.open = false;
    }

    // ── UI ───────────────────────────────────────────────────────────────────

    MouseArea {
        anchors.fill: parent
        onClicked: win.open = false
    }

    Rectangle {
        id: card
        anchors.horizontalCenter: parent.horizontalCenter
        y: parent.height * 0.22
        width: 560
        height: queryRow.height + (list.count > 0 ? list.height + 16 : 0) + 12
        radius: 18
        color: win.panel
        border.width: 1
        border.color: Qt.rgba(win.accent.r, win.accent.g, win.accent.b, 0.25)

        opacity: win.open ? 1 : 0
        scale: win.open ? 1 : 0.96
        Behavior on opacity { NumberAnimation { duration: 140; easing.type: Easing.OutCubic } }
        Behavior on scale { NumberAnimation { duration: 180; easing.type: Easing.OutBack; easing.overshoot: 1.2 } }
        Behavior on height { NumberAnimation { duration: 150; easing.type: Easing.OutCubic } }

        MouseArea { anchors.fill: parent }  // swallow clicks inside the card

        RowLayout {
            id: queryRow
            width: parent.width
            height: 54
            spacing: 10
            Text {
                Layout.leftMargin: 18
                text: win.mode === "actions"
                    ? (win.crumb.length ? "\u{f0141}" : "\u{f0493}")
                    : "\u{f0349}"
                font.family: win.mono
                font.pixelSize: 17
                color: win.accent
                TapHandler {
                    enabled: win.mode === "actions" && win.crumb.length > 0
                    onTapped: win.crumb = win.crumb.slice(0, -1)
                }
            }
            TextInput {
                id: query
                Layout.fillWidth: true
                Layout.rightMargin: 18
                focus: true
                color: win.fg
                font.family: win.mono
                font.pixelSize: 16
                clip: true
                onTextChanged: {
                    list.currentIndex = 0;
                    if (text.startsWith("f ")) fileDebounce.restart();
                }
                Keys.onPressed: event => {
                    if (event.key === Qt.Key_Escape) {
                        win.open = false;
                        event.accepted = true;
                    } else if ((event.key === Qt.Key_Backspace || event.key === Qt.Key_Left)
                               && text === "" && win.mode === "actions" && win.crumb.length > 0) {
                        win.crumb = win.crumb.slice(0, -1);
                        list.currentIndex = 0;
                        event.accepted = true;
                    }
                }
                Keys.onReturnPressed: win.activate()
                Keys.onEnterPressed: win.activate()
                Keys.onDownPressed: list.currentIndex = Math.min(list.currentIndex + 1, list.count - 1)
                Keys.onUpPressed: list.currentIndex = Math.max(list.currentIndex - 1, 0)
                Keys.onTabPressed: list.currentIndex = (list.currentIndex + 1) % Math.max(list.count, 1)
                Text {
                    visible: query.text === ""
                    text: win.mode === "actions"
                        ? (win.crumb.map(c => c.title).join(" › ") || "vendiOS")
                        : "search · 2+2 · :emoji · f files · w windows · >run"
                    color: win.dim
                    font.family: win.mono
                    font.pixelSize: 14
                    anchors.verticalCenter: parent.verticalCenter
                }
            }
        }

        Rectangle {
            anchors.top: queryRow.bottom
            width: parent.width - 28
            anchors.horizontalCenter: parent.horizontalCenter
            height: 1
            color: Qt.rgba(1, 1, 1, 0.07)
            visible: list.count > 0
        }

        ListView {
            id: list
            anchors.top: queryRow.bottom
            anchors.topMargin: 8
            anchors.horizontalCenter: parent.horizontalCenter
            width: parent.width - 16
            height: Math.min(contentHeight, 440)
            interactive: contentHeight > 440
            clip: true
            model: win.results
            delegate: Rectangle {
                required property var modelData
                required property int index
                width: list.width
                height: 44
                radius: 10
                color: list.currentIndex === index
                    ? Qt.rgba(win.accent.r, win.accent.g, win.accent.b, 0.16)
                    : "transparent"
                RowLayout {
                    anchors.fill: parent
                    anchors.leftMargin: 12
                    anchors.rightMargin: 12
                    spacing: 12
                    IconImage {
                        visible: !!modelData.icon
                        source: modelData.icon ? Quickshell.iconPath(modelData.icon, "application-x-executable") : ""
                        implicitSize: 24
                    }
                    Text {
                        visible: !modelData.icon
                        text: modelData.glyph || ""
                        font.family: win.mono
                        font.pixelSize: 18
                        color: win.accent
                        Layout.preferredWidth: 24
                        horizontalAlignment: Text.AlignHCenter
                    }
                    Text {
                        Layout.fillWidth: true
                        text: modelData.title
                        elide: Text.ElideRight
                        color: list.currentIndex === index ? win.accent : win.fg
                        font.family: win.mono
                        font.pixelSize: 14
                    }
                    Text {
                        text: modelData.hint || ""
                        color: win.dim
                        font.family: win.mono
                        font.pixelSize: 11
                    }
                }
                MouseArea {
                    anchors.fill: parent
                    hoverEnabled: true
                    onEntered: list.currentIndex = index
                    onClicked: win.activate()
                }
            }
        }
    }
}
