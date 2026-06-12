// vendi spotlight — the quickshell launcher (lives inside vendibar-pro, so
// it opens instantly and shares the theme).
//
//   plain text   fuzzy app search (desktop entries)
//   2+2*8        inline calculator (Enter copies the result)
//   :fire        emoji search (Enter copies)
//   w term       open-window switcher (Enter focuses, via vendi-ctl)
//   >cmd         run a shell command
//
// Toggled over IPC: quickshell -c vendibar-pro ipc call launcher toggle
// (bound to super+space through vendi-launcher). Esc closes, arrows move,
// Enter activates.

import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import Quickshell.Widgets
import QtQuick
import QtQuick.Layouts

PanelWindow {
    id: win

    property bool open: false
    // Theme handles, wired from shell.qml.
    property color accent: "#cba6f7"
    property color panel: "#0b0b12"
    property color fg: "#cdd6f4"
    property color dim: "#717189"
    property string mono: "JetBrainsMonoNL Nerd Font"

    function toggle() {
        open = !open;
        if (open) {
            query.text = "";
            list.currentIndex = 0;
            winRefresh.running = true;
        }
    }

    visible: open
    color: "transparent"
    anchors { top: true; bottom: true; left: true; right: true }
    WlrLayershell.namespace: "vendi-spotlight"
    WlrLayershell.layer: WlrLayer.Overlay
    WlrLayershell.keyboardFocus: open ? WlrKeyboardFocus.Exclusive : WlrKeyboardFocus.None
    exclusionMode: ExclusionMode.Ignore

    // ── result providers ─────────────────────────────────────────────────────

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
        // subsequence match
        let j = 0;
        for (const c of hay) if (c === needle[j]) j++;
        return j === needle.length ? 3 : -1;
    }

    // The unified result list: [{glyph|icon, title, hint, action}]
    property var results: {
        const q = query.text;
        const out = [];
        if (q.startsWith(">")) {
            const cmd = q.slice(1).trim();
            if (cmd) out.push({ glyph: "\u{eb32}", title: cmd, hint: "run command",
                                act: () => Quickshell.execDetached(["sh", "-c", cmd]) });
            return out;
        }
        if (q.startsWith(":")) {
            const needle = q.slice(1).trim().toLowerCase();
            for (const [e, words] of win.emoji) {
                if (!needle || words.includes(needle) || words.split(" ").some(w => w.startsWith(needle))) {
                    out.push({ glyph: e, title: words.split(" ").slice(0, 3).join(" "), hint: "copy emoji",
                               act: () => Quickshell.execDetached(["sh", "-c", "wl-copy '" + e + "'"]) });
                    if (out.length >= 24) break;
                }
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
        // calculator answer rides on top when the query computes
        const c = calc(q);
        if (c !== null) {
            out.push({ glyph: "\u{f0349}", title: c, hint: "copy result",
                       act: () => Quickshell.execDetached(["sh", "-c", "wl-copy '" + c + "'"]) });
        }
        // apps
        const apps = DesktopEntries.applications.values
            .filter(a => !a.noDisplay)
            .map(a => ({ a: a, s: q ? fuzzy(a.name, q) : 0 }))
            .filter(x => x.s >= 0)
            .sort((x, y) => x.s - y.s || x.a.name.localeCompare(y.a.name))
            .slice(0, 9);
        for (const { a } of apps) {
            out.push({ icon: a.icon, title: a.name, hint: a.genericName || "",
                       act: () => a.execute() });
        }
        return out;
    }

    function activate() {
        const r = results[list.currentIndex];
        if (!r) return;
        r.act();
        win.open = false;
    }

    // ── UI ───────────────────────────────────────────────────────────────────

    // Click-away closes.
    MouseArea {
        anchors.fill: parent
        onClicked: win.open = false
    }

    Rectangle {
        id: card
        anchors.horizontalCenter: parent.horizontalCenter
        y: parent.height * 0.24
        width: 560
        height: queryRow.height + (list.count > 0 ? list.contentHeight + 14 : 0) + 14
        radius: 18
        color: win.panel
        border.width: 1
        border.color: Qt.rgba(win.accent.r, win.accent.g, win.accent.b, 0.25)

        // pop in
        opacity: win.open ? 1 : 0
        scale: win.open ? 1 : 0.96
        Behavior on opacity { NumberAnimation { duration: 140; easing.type: Easing.OutCubic } }
        Behavior on scale { NumberAnimation { duration: 180; easing.type: Easing.OutBack; easing.overshoot: 1.2 } }
        Behavior on height { NumberAnimation { duration: 140; easing.type: Easing.OutCubic } }

        MouseArea { anchors.fill: parent }  // swallow clicks inside the card

        RowLayout {
            id: queryRow
            width: parent.width
            height: 54
            spacing: 10
            Text {
                Layout.leftMargin: 18
                text: "\u{f0349}"
                font.family: win.mono
                font.pixelSize: 17
                color: win.accent
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
                onTextChanged: list.currentIndex = 0
                Keys.onEscapePressed: win.open = false
                Keys.onReturnPressed: win.activate()
                Keys.onEnterPressed: win.activate()
                Keys.onDownPressed: list.currentIndex = Math.min(list.currentIndex + 1, list.count - 1)
                Keys.onUpPressed: list.currentIndex = Math.max(list.currentIndex - 1, 0)
                Keys.onTabPressed: list.currentIndex = (list.currentIndex + 1) % Math.max(list.count, 1)
                Text {
                    visible: query.text === ""
                    text: "search · 2+2 · :emoji · w windows · >run"
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
            height: contentHeight
            interactive: false
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
