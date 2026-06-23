// AiContent — vendi AI, living inside the center notch.
//
// The notch itself becomes the panel (like Launcher fills it for spotlight): a
// little chat thread that streams the answer as it generates, with an input at
// the bottom. `wantHeight` reports the natural height so the notch springs open
// to fit. Conversation history is kept so follow-ups have context and you can
// see the thread.
//
// Streaming: vendi-ai prints answer chunks separated by 0x1e; we append each
// chunk live as it arrives.

import QtQuick
import Quickshell.Io

Item {
    id: ai

    property bool active: false
    property color accent:     "#cba6f7"
    property color panelColor: "#0b0b12"
    property color fg:         "#cdd6f4"
    property color dim:        "#717189"
    property string mono:      "JetBrainsMonoNL Nerd Font"
    signal requestClose()

    // notch height driver (capped); +input +paddings
    readonly property int maxThread: 440
    property real wantHeight: Math.min(maxThread, thread.contentHeight) + inputBox.height + 44

    // state
    property string phase: "ready"   // ready | thinking | responding
    property var history: []         // [{q, a, cards}]
    property string curQ: ""
    property string cur: ""
    property var cards: []           // cards for the in-progress turn
    property var pendingPerm: null   // {title, detail} awaiting Allow/Deny

    function decide(ok) {
        brain.write(ok ? "allow\n" : "deny\n");
        pendingPerm = null;
    }

    onActiveChanged: {
        if (active) input.forceActiveFocus();
        else { cur = ""; curQ = ""; phase = "ready"; }
    }

    function _withContext(q) {
        if (history.length === 0) return q;
        let ctx = "Earlier in this conversation:\n";
        for (const t of history.slice(-2)) ctx += "User: " + t.q + "\nYou: " + t.a + "\n";
        return ctx + "\nNow the user says: " + q;
    }
    function submit(t) {
        const q = t.trim();
        if (!q.length) return;
        if (q === "/clear" || q === "/c") {   // wipe the conversation
            history = []; cur = ""; curQ = ""; phase = "ready"; input.text = "";
            return;
        }
        if (phase === "thinking" || phase === "responding") return;
        curQ = q; cur = ""; cards = []; pendingPerm = null; phase = "thinking";
        input.text = "";
        brain.command = ["vendi-ai", _withContext(q)];
        brain.running = true;
    }

    // ── the chat thread (scrolls; auto-sticks to bottom) ─────────────────────
    Flickable {
        id: thread
        anchors { left: parent.left; right: parent.right; top: parent.top }
        anchors.leftMargin: 20; anchors.rightMargin: 20; anchors.topMargin: 16
        height: Math.min(ai.maxThread, contentHeight)
        contentHeight: threadCol.implicitHeight
        clip: true
        interactive: contentHeight > height
        onContentHeightChanged: contentY = Math.max(0, contentHeight - height)

        Column {
            id: threadCol
            width: thread.width
            spacing: 12

            // identity, only when empty
            Row {
                spacing: 7
                visible: ai.history.length === 0 && ai.phase === "ready"
                Text { text: "✦"; color: ai.accent; font.pixelSize: 13; anchors.verticalCenter: parent.verticalCenter }
                Text { text: "Ask vendi anything"; color: ai.dim; font.family: ai.mono; font.pixelSize: 13
                       anchors.verticalCenter: parent.verticalCenter }
            }

            // past turns
            Repeater {
                model: ai.history
                delegate: Column {
                    required property var modelData
                    width: threadCol.width
                    spacing: 4
                    Text {  // you
                        anchors.right: parent.right
                        text: modelData.q; color: ai.dim
                        font.family: ai.mono; font.pixelSize: 12
                        wrapMode: Text.WordWrap
                        horizontalAlignment: Text.AlignRight
                        width: Math.min(implicitWidth, parent.width)
                    }
                    Text {  // vendi
                        width: parent.width
                        visible: text.length > 0
                        text: modelData.a; color: ai.fg
                        font.family: ai.mono; font.pixelSize: 15; lineHeight: 1.4
                        wrapMode: Text.WordWrap
                    }
                    Repeater {  // result cards for this turn
                        model: modelData.cards || []
                        delegate: Column {
                            required property var modelData
                            width: threadCol.width
                            WeatherCard {
                                visible: parent.modelData.type === "weather"
                                width: parent.width; card: parent.modelData; mono: ai.mono
                                height: visible ? implicitHeight : 0
                            }
                            MatchCard {
                                visible: parent.modelData.type === "match"
                                width: parent.width; card: parent.modelData
                                fg: ai.fg; dim: ai.dim; accent: ai.accent; mono: ai.mono
                                height: visible ? implicitHeight : 0
                            }
                            InfoCard {
                                visible: parent.modelData.type !== "weather" && parent.modelData.type !== "match"
                                width: parent.width; card: parent.modelData
                                accent: ai.accent; fg: ai.fg; dim: ai.dim; mono: ai.mono
                                height: visible ? implicitHeight : 0
                            }
                        }
                    }
                }
            }

            // in-progress turn
            Column {
                width: threadCol.width
                spacing: 4
                visible: ai.phase !== "ready"
                Text {
                    anchors.right: parent.right
                    text: ai.curQ; color: ai.dim
                    font.family: ai.mono; font.pixelSize: 12
                    wrapMode: Text.WordWrap
                    horizontalAlignment: Text.AlignRight
                    width: Math.min(implicitWidth, parent.width)
                }
                // streaming answer
                Text {
                    id: streamText
                    width: parent.width
                    visible: ai.phase === "responding" && ai.cur.length > 0
                    text: ai.cur
                    color: ai.fg; font.family: ai.mono; font.pixelSize: 15; lineHeight: 1.4
                    wrapMode: Text.WordWrap
                }
                // live result cards for this turn
                Repeater {
                    model: ai.cards
                    delegate: Column {
                        required property var modelData
                        width: threadCol.width
                        WeatherCard {
                            visible: parent.modelData.type === "weather"
                            width: parent.width; card: parent.modelData; mono: ai.mono
                            height: visible ? implicitHeight : 0
                        }
                        MatchCard {
                            visible: parent.modelData.type === "match"
                            width: parent.width; card: parent.modelData
                            fg: ai.fg; dim: ai.dim; accent: ai.accent; mono: ai.mono
                            height: visible ? implicitHeight : 0
                        }
                        InfoCard {
                            visible: parent.modelData.type !== "weather" && parent.modelData.type !== "match"
                            width: parent.width; card: parent.modelData
                            accent: ai.accent; fg: ai.fg; dim: ai.dim; mono: ai.mono
                            height: visible ? implicitHeight : 0
                        }
                    }
                }

                // permission request (Tier-2 action) — Allow/Deny
                Rectangle {
                    visible: ai.pendingPerm !== null
                    width: threadCol.width
                    implicitHeight: permCol.implicitHeight + 24
                    radius: 14
                    color: Qt.rgba(ai.accent.r, ai.accent.g, ai.accent.b, 0.08)
                    border.width: 1
                    border.color: Qt.rgba(ai.accent.r, ai.accent.g, ai.accent.b, 0.45)
                    Column {
                        id: permCol
                        anchors { left: parent.left; right: parent.right; top: parent.top; margins: 12 }
                        spacing: 8
                        Text {
                            width: parent.width; wrapMode: Text.WordWrap
                            text: ai.pendingPerm ? ai.pendingPerm.title : ""
                            color: ai.fg; font.family: ai.mono; font.pixelSize: 13; font.weight: Font.DemiBold
                        }
                        Text {
                            width: parent.width; wrapMode: Text.WrapAnywhere
                            visible: ai.pendingPerm && ai.pendingPerm.detail && String(ai.pendingPerm.detail).length
                            text: ai.pendingPerm ? ai.pendingPerm.detail : ""
                            color: ai.dim; font.family: ai.mono; font.pixelSize: 12
                        }
                        Row {
                            spacing: 8
                            Rectangle {
                                width: 84; height: 30; radius: 9
                                color: Qt.rgba(1, 1, 1, denyHover.hovered ? 0.12 : 0.06)
                                Text { anchors.centerIn: parent; text: "Deny"; color: ai.dim
                                       font.family: ai.mono; font.pixelSize: 12 }
                                HoverHandler { id: denyHover }
                                TapHandler { onTapped: ai.decide(false) }
                            }
                            Rectangle {
                                width: 92; height: 30; radius: 9
                                color: Qt.rgba(ai.accent.r, ai.accent.g, ai.accent.b, allowHover.hovered ? 0.95 : 0.75)
                                Text { anchors.centerIn: parent; text: "Allow"; color: "#11111b"
                                       font.family: ai.mono; font.pixelSize: 12; font.weight: Font.DemiBold }
                                HoverHandler { id: allowHover }
                                TapHandler { onTapped: ai.decide(true) }
                            }
                        }
                    }
                }
                // thinking — a smooth bouncing wave
                Row {
                    spacing: 6; visible: ai.phase === "thinking"
                    topPadding: 2
                    Repeater {
                        model: 3
                        delegate: Rectangle {
                            required property int index
                            width: 7; height: 7; radius: 4; color: ai.accent
                            y: 0
                            opacity: 0.55 + 0.45 * (Math.abs(y) / 5)
                            SequentialAnimation on y {
                                loops: Animation.Infinite; running: ai.phase === "thinking"
                                PauseAnimation { duration: index * 150 }
                                NumberAnimation { to: -5; duration: 300; easing.type: Easing.OutSine }
                                NumberAnimation { to: 0;  duration: 300; easing.type: Easing.InSine }
                                PauseAnimation { duration: (2 - index) * 150 + 240 }
                            }
                        }
                    }
                }
            }
        }
    }

    // ── input ────────────────────────────────────────────────────────────────
    Rectangle {
        id: inputBox
        anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
        anchors.leftMargin: 18; anchors.rightMargin: 18; anchors.bottomMargin: 16
        height: 46; radius: 14
        color: Qt.rgba(1, 1, 1, 0.05)
        border.width: 1
        border.color: input.activeFocus ? Qt.rgba(ai.accent.r, ai.accent.g, ai.accent.b, 0.5)
                                        : Qt.rgba(1, 1, 1, 0.08)
        Behavior on border.color { ColorAnimation { duration: 150 } }

        Text {
            anchors.left: parent.left; anchors.leftMargin: 15
            anchors.verticalCenter: parent.verticalCenter
            visible: input.text.length === 0
            text: ai.phase === "thinking" ? "thinking…" : "Ask vendi…"
            color: ai.dim; font.family: ai.mono; font.pixelSize: 14
        }
        TextInput {
            id: input
            anchors.fill: parent
            anchors.leftMargin: 15; anchors.rightMargin: 15
            verticalAlignment: TextInput.AlignVCenter
            color: ai.fg; font.family: ai.mono; font.pixelSize: 14
            clip: true
            enabled: ai.phase !== "thinking"
            onAccepted: ai.submit(text)
            Keys.onEscapePressed: ai.requestClose()
        }
    }

    // ── brain (streaming) ────────────────────────────────────────────────────
    Process {
        id: brain
        stdinEnabled: true
        stdout: SplitParser {
            splitMarker: ""
            onRead: chunk => {
                if (ai.phase === "thinking") ai.phase = "responding";
                // [[CLEAR]] — the text streamed so far was pre-tool chatter; drop it
                // so only the real answer (streamed next) is shown.
                if (chunk.indexOf("[[CLEAR]]") !== -1) {
                    ai.cur = "";
                    chunk = chunk.split("[[CLEAR]]").join("");
                    if (chunk.length === 0) return;
                }
                // [[PERM]] — a Tier-2 action awaiting Allow/Deny
                var qi = chunk.indexOf("[[PERM]]");
                if (qi !== -1) {
                    if (qi > 0) ai.cur += chunk.substring(0, qi);
                    try { ai.pendingPerm = JSON.parse(chunk.slice(qi + 8)); } catch (e) {}
                    return;
                }
                // [[CARD]]…JSON can appear anywhere in the chunk; peel out every
                // card, keep the surrounding text.
                var ci = chunk.indexOf("[[CARD]]");
                while (ci !== -1) {
                    if (ci > 0) ai.cur += chunk.substring(0, ci);
                    var rest = chunk.substring(ci + 8);
                    try {
                        ai.cards = ai.cards.concat([JSON.parse(rest)]);
                        rest = "";   // the JSON consumed the remainder of this segment
                    } catch (e) {}
                    chunk = rest;
                    ci = chunk.indexOf("[[CARD]]");
                }
                ai.cur += chunk;
            }
        }
        onExited: (code, status) => {
            const a = ai.cur.trim().length ? ai.cur.trim() : (ai.cards.length ? "" : "…");
            const h = ai.history.slice();
            h.push({ q: ai.curQ, a: a, cards: ai.cards });
            ai.history = h.slice(-6);
            ai.cur = ""; ai.curQ = ""; ai.cards = []; ai.phase = "ready";
        }
    }
}
