// ChatView — the right pane: header, the message list (with a typing-indicator
// footer), and the composer. Switching conversations does a subtle whole-pane
// fade (not per-bubble pops); a newly sent/received message fades in on its own.

import QtQuick
import QtQuick.Controls.Basic

Item {
    id: cv
    property var theme
    property var convo            // { id, name, color, messages, typing }
    signal send(string text, string replyName, string replyText, string replyId)
    signal sendImage(string path)
    signal openImage(string source)
    signal openInfo()
    signal startCall(bool video)
    signal reacted(int index, string reactionsJson)
    signal reactSent(string msgId, string emoji)

    function addReaction(idx, emoji) {
        var cur = [];
        try { cur = JSON.parse(thread.get(idx).reactions || "[]"); } catch (e) {}
        cur.push(emoji);
        var js = JSON.stringify(cur);
        thread.setProperty(idx, "reactions", js);
        cv.reacted(idx, js);
        var mid = thread.get(idx).id;
        if (mid) cv.reactSent(mid, emoji);
    }

    property string _loadedId: ""
    onConvoChanged: syncModel()
    Component.onCompleted: syncModel()

    function syncModel() {
        if (!convo) { thread.clear(); _loadedId = ""; return; }
        var msgs = convo.messages;
        if (convo.id !== _loadedId) {            // switched conversation → rebuild + fade
            thread.clear();
            for (var i = 0; i < msgs.length; i++) thread.append(msgs[i]);
            _loadedId = convo.id;
            switchFade.restart();
        } else {                                  // same convo → append only new tail
            for (var j = thread.count; j < msgs.length; j++) thread.append(msgs[j]);
        }
        Qt.callLater(msgs_view.positionViewAtEnd);
    }
    function startReply(name, text, id) { composer.setReply(name, text, id); }

    ListModel { id: thread }

    // ── header (iMessage: centered avatar + name › , call buttons top-right) ─────
    Item {
        id: header
        anchors { left: parent.left; right: parent.right; top: parent.top }
        height: 64

        Column {
            anchors.centerIn: parent
            spacing: 3
            Avatar {
                anchors.horizontalCenter: parent.horizontalCenter
                name: cv.convo ? cv.convo.name : ""
                tint: cv.convo ? cv.convo.color : theme.textSecondary
                size: 30; ui: theme.ui
                group: cv.convo ? cv.convo.group === true : false
                members: cv.convo && cv.convo.members ? cv.convo.members : []
            }
            Row {
                anchors.horizontalCenter: parent.horizontalCenter
                spacing: 3
                Text {
                    text: cv.convo ? cv.convo.name : ""
                    color: theme.textPrimary
                    font.pixelSize: 13; font.weight: Font.DemiBold; font.family: theme.ui
                    anchors.verticalCenter: parent.verticalCenter
                }
                Text {
                    text: "›"; color: theme.textSecondary
                    font.pixelSize: 14; font.family: theme.ui
                    anchors.verticalCenter: parent.verticalCenter
                }
            }
        }
        TapHandler { onTapped: cv.openInfo() }   // tap the name area → details

        // call buttons (FaceTime audio + video)
        Row {
            anchors.right: parent.right; anchors.rightMargin: 14
            anchors.verticalCenter: parent.verticalCenter
            spacing: 2
            CallButton { kind: "audio"; theme: cv.theme; onClicked: cv.startCall(false) }
            CallButton { kind: "video"; theme: cv.theme; onClicked: cv.startCall(true) }
        }

        Rectangle {
            anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
            height: 1; color: theme.divider
        }
    }

    // drawn phone / video-camera button
    component CallButton: Rectangle {
        property var theme
        property string kind: "video"
        signal clicked()
        width: 34; height: 34; radius: 17
        color: cbHover.hovered ? theme.hoverBg : "transparent"
        scale: cbTap.pressed ? 0.88 : 1.0
        Behavior on scale { NumberAnimation { duration: 110; easing.type: Easing.OutQuad } }
        Canvas {
            id: callIcon
            anchors.centerIn: parent; width: 20; height: 18
            onPaint: {
                var ctx = getContext("2d"); ctx.clearRect(0, 0, width, height);
                ctx.strokeStyle = theme.accent; ctx.fillStyle = theme.accent;
                ctx.lineWidth = 1.7; ctx.lineCap = "round"; ctx.lineJoin = "round";
                if (kind === "video") {
                    // camera body
                    ctx.beginPath(); ctx.moveTo(2,5); ctx.lineTo(12,5);
                    ctx.quadraticCurveTo(14,5,14,7); ctx.lineTo(14,11);
                    ctx.quadraticCurveTo(14,13,12,13); ctx.lineTo(2,13);
                    ctx.quadraticCurveTo(0,13,0,11); ctx.lineTo(0,7);
                    ctx.quadraticCurveTo(0,5,2,5); ctx.stroke();
                    // lens triangle
                    ctx.beginPath(); ctx.moveTo(15,7); ctx.lineTo(20,4); ctx.lineTo(20,14); ctx.lineTo(15,11); ctx.closePath(); ctx.stroke();
                } else {
                    // phone handset
                    ctx.beginPath();
                    ctx.moveTo(2,3);
                    ctx.quadraticCurveTo(2,1,4,2);
                    ctx.lineTo(7,4);
                    ctx.quadraticCurveTo(8,5,7,6);
                    ctx.lineTo(6,7);
                    ctx.quadraticCurveTo(8,11,12,13);
                    ctx.lineTo(13,12);
                    ctx.quadraticCurveTo(14,11,15,12);
                    ctx.lineTo(17,15);
                    ctx.quadraticCurveTo(18,17,16,17);
                    ctx.quadraticCurveTo(7,17,2,3);
                    ctx.stroke();
                }
            }
            Connections { target: theme; function onAccentChanged() { callIcon.requestPaint() } }
        }
        HoverHandler { id: cbHover }
        TapHandler { id: cbTap; onTapped: parent.clicked() }
    }

    // ── messages ────────────────────────────────────────────────────────────────
    ListView {
        id: msgs_view
        anchors { left: parent.left; right: parent.right; top: header.bottom; bottom: composer.top }
        anchors.leftMargin: 18; anchors.rightMargin: 18; anchors.topMargin: 12; anchors.bottomMargin: 6
        clip: true
        spacing: 2
        model: thread
        cacheBuffer: 4000
        // auto-scroll: jump to the newest on new messages, and follow content
        // growth (e.g. the typing bubble) only when already at the bottom.
        property bool atBottom: true
        onContentYChanged: atBottom = atYEnd || contentHeight <= height
        onContentHeightChanged: if (atBottom) Qt.callLater(positionViewAtEnd)
        onCountChanged: Qt.callLater(positionViewAtEnd)

        // subtle fade of the whole pane when switching conversations
        NumberAnimation { id: switchFade; target: msgs_view; property: "opacity"; from: 0; to: 1
                          duration: 220; easing.type: Easing.OutQuad }

        delegate: MessageBubble {
            width: msgs_view.width
            theme: cv.theme
            text: model.text
            time: model.time
            mine: model.mine === true
            kind: model.kind ? model.kind : "text"
            source: model.source ? model.source : ""
            replyName: model.replyName ? model.replyName : ""
            replyText: model.replyText ? model.replyText : ""
            reactions: model.reactions ? model.reactions : "[]"

            // sender-run logic (in groups, a run also breaks when the author changes)
            property bool isGroup: cv.convo && cv.convo.group === true
            property var prevMsg: index > 0 ? thread.get(index - 1) : null
            property var nextMsg: index < thread.count - 1 ? thread.get(index + 1) : null
            function breaks(a, b) {
                if (!a || !b) return true;
                if (a.mine !== b.mine) return true;
                return isGroup && !b.mine && a.sender !== b.sender;
            }
            senderName: model.sender ? model.sender : ""
            senderColor: model.senderColor && String(model.senderColor).length ? model.senderColor : cv.theme.textSecondary
            gutter: isGroup ? 32 : 0
            groupStart: index > 0 && breaks(prevMsg, model)
            showTail: breaks(nextMsg, model)
            showSender: isGroup && !model.mine && (index === 0 || breaks(prevMsg, model))
            showAvatar: isGroup && !model.mine && breaks(nextMsg, model)
            delivered: model.mine === true && index === (thread.count - 1)
            msgId: model.id ? model.id : ""
            onImageClicked: function (s) { cv.openImage(s); }
            onReplyRequested: cv.startReply(model.mine ? "You" : (cv.convo ? cv.convo.name : (model.sender ? model.sender : "")),
                                           model.kind === "image" ? "Photo" : model.text, model.id ? model.id : "")
            onReact: function (e) { cv.addReaction(index, e); }
        }

        // a newly added message fades in (no pop)
        add: Transition { NumberAnimation { property: "opacity"; from: 0; to: 1; duration: 150 } }
        displaced: Transition { NumberAnimation { properties: "y"; duration: 160; easing.type: Easing.OutCubic } }

        footer: TypingIndicator {
            width: msgs_view.width
            theme: cv.theme
            active: cv.convo && cv.convo.typing === true
        }
    }

    Composer {
        id: composer
        anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
        theme: cv.theme
        onSend: function (t) { cv.send(t, composer.replyName, composer.replyText, composer.replyId); composer.clearReply(); }
        onAttach: function (p) { cv.sendImage(p); }
    }
}
