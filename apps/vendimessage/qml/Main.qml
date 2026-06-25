// vendiMessage — main window. A normal OS window (the compositor handles
// decorations, corners, shadow, and tiling/floating — we draw none of that): the
// content fills the whole window. Two-pane, iMessage-*inspired* (clean, not a
// copy). Runs standalone for UI dev with mock data (`qml6 Main.qml`); the real
// vendi-chatd backend drops in behind the `backend` object later.

import QtQuick
import QtQuick.Controls.Basic
import QtCore
import "mockdata.js" as Mock

ApplicationWindow {
    id: win
    width: 900; height: 620
    minimumWidth: 560; minimumHeight: 400
    visible: true
    title: "vendiMessage"
    color: theme.windowBg

    property bool dark: true
    property string ui: "Adwaita Sans"
    // bubbles follow the live vendiOS accent (~/.config/vendi/theme-state)
    property string accentHex: "#7b6cff"
    function readAccent() {
        try {
            var cfg = "" + StandardPaths.writableLocation(StandardPaths.GenericConfigLocation);
            if (!cfg.length) return;
            var xhr = new XMLHttpRequest();
            xhr.open("GET", cfg + "/vendi/theme-state", false);
            xhr.send();
            if (xhr.status === 200 || xhr.status === 0) {
                var m = /ACCENT_HEX=([0-9a-fA-F]{6})/.exec(xhr.responseText);
                if (m) accentHex = "#" + m[1];
            }
        } catch (e) {}
    }
    Component.onCompleted: readAccent()
    Timer { interval: 3000; running: true; repeat: true; onTriggered: win.readAccent() }
    property string lightbox: ""        // source of the image shown full-screen, "" = hidden
    property bool infoOpen: false       // contact/group info page
    property bool composeOpen: false    // new-message sheet
    readonly property int sidebarW: 268

    function createConversation(name) {
        if (!String(name).trim().length) return;
        if (backend.connected) { backend.startChat(String(name).trim()); composeOpen = false; return; }
        var palette = ["#5b7cfa", "#f0883e", "#bc6bd9", "#56b36a", "#d9534f", "#e0b341", "#34c759"];
        var c = {
            id: "!new" + Date.now(), name: String(name).trim(),
            color: palette[Math.floor(Math.random() * palette.length)],
            preview: "", time: nowTime(), unread: false, messages: []
        };
        _setConvos([c].concat(conversations));
        currentIndex = 0;
        composeOpen = false;
    }
    function toggleMute() {
        var convos = conversations.slice();
        var c = Object.assign({}, convos[currentIndex]);
        c.muted = !c.muted;
        convos[currentIndex] = c;
        _setConvos(convos);
    }

    function isImageUrl(u) {
        return /\.(png|jpe?g|gif|webp|bmp)$/i.test(String(u));
    }

    // ── theme ────────────────────────────────────────────────────────────────
    QtObject {
        id: theme
        readonly property string ui: win.ui
        // bubbles follow the live vendiOS accent; accent2 is a derived deeper shade
        readonly property color accent:        win.accentHex
        readonly property color accent2:       Qt.darker(win.accentHex, 1.4)
        readonly property color bubbleOutText: "#ffffff"
        property color windowBg:      win.dark ? "#161618" : "#ffffff"
        property color sidebarBg:     win.dark ? "#1d1d20" : "#f7f7f9"
        property color divider:       win.dark ? "#2b2b2f" : "#e9e9ec"
        property color textPrimary:   win.dark ? "#f4f4f7" : "#16161a"
        property color textSecondary: win.dark ? "#8c8c94" : "#86868b"
        property color bubbleIn:      win.dark ? "#2c2c30" : "#ececef"
        property color bubbleInText:  win.dark ? "#f4f4f7" : "#16161a"
        property color inputBg:       win.dark ? "#252529" : "#eeeef1"
        property color hoverBg:       win.dark ? Qt.rgba(1,1,1,0.06) : Qt.rgba(0,0,0,0.05)

        // animate the whole palette when the theme toggles
        Behavior on windowBg      { ColorAnimation { duration: 260; easing.type: Easing.InOutQuad } }
        Behavior on sidebarBg     { ColorAnimation { duration: 260; easing.type: Easing.InOutQuad } }
        Behavior on divider       { ColorAnimation { duration: 260; easing.type: Easing.InOutQuad } }
        Behavior on textPrimary   { ColorAnimation { duration: 260; easing.type: Easing.InOutQuad } }
        Behavior on textSecondary { ColorAnimation { duration: 260; easing.type: Easing.InOutQuad } }
        Behavior on bubbleIn      { ColorAnimation { duration: 260; easing.type: Easing.InOutQuad } }
        Behavior on inputBg       { ColorAnimation { duration: 260; easing.type: Easing.InOutQuad } }
    }

    // ── backend: live vendi-chatd over WebSocket, mock fallback when offline ───
    Backend { id: backend }
    property var mockConvos: Mock.conversations()
    readonly property var conversations: backend.connected ? backend.conversations : mockConvos
    property int currentIndex: 0
    readonly property var currentConvo: (conversations && currentIndex < conversations.length)
                                        ? conversations[currentIndex] : undefined
    onCurrentConvoChanged: {
        if (backend.connected && currentConvo && currentConvo.messages.length === 0)
            backend.loadRoom(currentConvo.id);
        // opening a chat clears its unread (server receipt + local), killing the
        // stale notification dot
        if (currentConvo && currentConvo.unread) {
            if (backend.connected) backend.markRead(currentConvo.id);
            _clearUnread(currentIndex);
        }
    }
    function _setConvos(arr) { if (backend.connected) backend.conversations = arr; else mockConvos = arr; }
    function _clearUnread(idx) {
        var arr = conversations.slice();
        if (!arr[idx] || !arr[idx].unread) return;
        var c = Object.assign({}, arr[idx]); c.unread = 0; arr[idx] = c;
        _setConvos(arr);
    }

    property int _typingFor: 0          // convo index the simulated reply is for

    function nowTime() {
        return Qt.formatTime(new Date(), "h:mm AP");
    }
    function sendMessage(text, replyName, replyText, replyId) {
        if (backend.connected) { if (currentConvo) backend.send(currentConvo.id, text, replyId); return; }
        appendMessage(currentIndex, {
            id: "", text: text, mine: true, time: nowTime(), kind: "text", source: "",
            replyTo: replyId || "", replyName: replyName || "", replyText: replyText || "",
            sender: "", senderColor: "", reactions: "[]"
        }, text);
        simulateReply();
    }
    function sendImageMessage(path) {
        if (backend.connected) { if (currentConvo) backend.sendImage(currentConvo.id, path); return; }
        appendMessage(currentIndex, {
            text: "", mine: true, time: nowTime(), kind: "image", source: path,
            replyName: "", replyText: "", sender: "", senderColor: "", reactions: "[]"
        }, "📷 Photo");
        simulateReply();
    }
    function appendMessage(idx, msg, preview) {
        var convos = conversations.slice();
        var c = Object.assign({}, convos[idx]);
        c.messages = c.messages.concat([msg]);
        c.preview = preview;
        c.typing = false;
        convos[idx] = c;
        _setConvos(convos);              // re-triggers currentConvo → live update
    }
    function setTyping(idx, on) {
        var convos = conversations.slice();
        var c = Object.assign({}, convos[idx]);
        c.typing = on;
        convos[idx] = c;
        _setConvos(convos);
    }

    // MOCK only: pretend the contact starts typing, then replies. Goes away once
    // the real backend drives `typing` and incoming messages.
    function simulateReply() {
        _typingFor = currentIndex;
        typingTimer.restart();
    }
    Timer {
        id: typingTimer; interval: 900
        onTriggered: { win.setTyping(win._typingFor, true); replyTimer.restart(); }
    }
    Timer {
        id: replyTimer; interval: 2200
        onTriggered: {
            var replies = ["sounds good!", "haha 😄", "for sure 👍", "ok!", "nice", "totally agree"];
            var convo = win.conversations[win._typingFor];
            var sender = "", scolor = "";
            if (convo && convo.group && convo.members && convo.members.length) {
                var m = convo.members[Math.floor(Math.random() * convo.members.length)];
                sender = m.name; scolor = m.color;
            }
            win.appendMessage(win._typingFor, {
                text: replies[Math.floor(Math.random() * replies.length)],
                mine: false, time: win.nowTime(), kind: "text", source: "",
                replyName: "", replyText: "", sender: sender, senderColor: scolor, reactions: "[]"
            }, "");
        }
    }

    // ── layout: content fills the whole window ─────────────────────────────────
    Row {
        anchors.fill: parent
        Sidebar {
            id: sidebar
            width: win.sidebarW; height: parent.height
            theme: theme
            dark: win.dark
            model: win.conversations
            requests: backend.connected ? backend.requests : []
            currentIndex: win.currentIndex
            onSelected: function (id) {
                for (var i = 0; i < win.conversations.length; i++)
                    if (win.conversations[i].id === id) { win.currentIndex = i; break; }
            }
            onToggleTheme: win.dark = !win.dark
            onNewChat: win.composeOpen = true
            onAcceptRequest: function (id) { if (backend.connected) backend.acceptInvite(id); }
            onRejectRequest: function (id) { if (backend.connected) backend.rejectInvite(id); }
        }
        ChatView {
            width: parent.width - win.sidebarW; height: parent.height
            theme: theme
            convo: win.currentConvo
            onSend: function (t, rn, rt, rid) { win.sendMessage(t, rn, rt, rid); }
            onSendImage: function (p) { win.sendImageMessage(p); }
            onOpenImage: function (s) { win.lightbox = s; }
            onOpenInfo: win.infoOpen = true
            onReacted: function (i, js) {
                // persist into the source message so it survives a convo switch
                if (win.currentConvo && win.currentConvo.messages[i])
                    win.currentConvo.messages[i].reactions = js;
            }
            onReactSent: function (mid, emoji) {
                // send the reaction to the homeserver (display already updated locally)
                if (backend.connected && win.currentConvo) backend.react(win.currentConvo.id, mid, emoji);
            }
        }
    }

    // ── contact / group info page ──────────────────────────────────────────────
    InfoPanel {
        theme: theme
        convo: win.currentConvo
        open: win.infoOpen
        onClosed: win.infoOpen = false
        onAct: function (which) {
            if (which === "message") win.infoOpen = false;
            else if (which === "search") { win.infoOpen = false; sidebar.focusSearch(); }
            else if (which === "mute") win.toggleMute();
        }
    }

    // ── compose (pencil) → new conversation ────────────────────────────────────
    NewChatSheet {
        theme: theme
        open: win.composeOpen
        connected: backend.connected
        onClosed: win.composeOpen = false
        onCreate: function (name) { win.createConversation(name); }
    }

    // ── onboarding: login / create account (shown until the daemon has a session)
    LoginPage {
        id: loginPage
        anchors.fill: parent
        z: 100
        visible: backend.connected && !backend.authed
        theme: theme
        onSubmit: function (user, password, isRegister) {
            if (isRegister) backend.register(user, password);
            else backend.login(user, password);
        }
    }
    Connections {
        target: backend
        function onErrored(message) { loginPage.errorText = message; }
        function onAuthedChanged() { if (backend.authed) { loginPage.busy = false; loginPage.errorText = ""; } }
    }

    // ── drag & drop images anywhere in the window ──────────────────────────────
    DropArea {
        anchors.fill: parent
        onDropped: function (drop) {
            if (!drop.hasUrls) return;
            for (var i = 0; i < drop.urls.length; i++)
                if (win.isImageUrl(drop.urls[i])) win.sendImageMessage(String(drop.urls[i]));
            drop.accept();
        }
        Rectangle {   // drop highlight
            anchors.fill: parent
            visible: parent.containsDrag
            color: Qt.rgba(theme.accent.r, theme.accent.g, theme.accent.b, 0.12)
            border.width: 3; border.color: theme.accent
            Text {
                anchors.centerIn: parent; text: "Drop a photo to send"
                color: theme.textPrimary; font.pixelSize: 18; font.family: theme.ui; font.weight: Font.DemiBold
            }
        }
    }

    // ── image lightbox (tap an image to preview) ───────────────────────────────
    Rectangle {
        id: lb
        anchors.fill: parent
        color: Qt.rgba(0, 0, 0, 0.88)
        visible: opacity > 0
        opacity: win.lightbox.length ? 1 : 0
        Behavior on opacity { NumberAnimation { duration: 180; easing.type: Easing.OutQuad } }

        Image {
            anchors.centerIn: parent
            source: win.lightbox
            fillMode: Image.PreserveAspectFit
            width: parent.width * 0.92; height: parent.height * 0.92
            smooth: true; mipmap: true
            scale: win.lightbox.length ? 1.0 : 0.92
            Behavior on scale { NumberAnimation { duration: 200; easing.type: Easing.OutBack } }
        }
        TapHandler { onTapped: win.lightbox = "" }
        Shortcut { sequence: "Escape"; enabled: win.lightbox.length > 0; onActivated: win.lightbox = "" }
    }
}
