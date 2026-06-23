// ChatView — the right pane: header, the message list (with a typing-indicator
// footer), and the composer. Switching conversations does a subtle whole-pane
// fade (not per-bubble pops); a newly sent/received message fades in on its own.

import QtQuick
import QtQuick.Controls.Basic

Item {
    id: cv
    property var theme
    property var convo            // { id, name, color, messages, typing }
    signal send(string text, string replyName, string replyText)
    signal sendImage(string path)
    signal openImage(string source)
    signal openInfo()
    signal reacted(int index, string reactionsJson)

    function addReaction(idx, emoji) {
        var cur = [];
        try { cur = JSON.parse(thread.get(idx).reactions || "[]"); } catch (e) {}
        cur.push(emoji);
        var js = JSON.stringify(cur);
        thread.setProperty(idx, "reactions", js);
        cv.reacted(idx, js);
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
    function startReply(name, text) { composer.setReply(name, text); }

    ListModel { id: thread }

    // ── header ─────────────────────────────────────────────────────────────────
    Item {
        id: header
        anchors { left: parent.left; right: parent.right; top: parent.top }
        height: 58
        Row {
            anchors.centerIn: parent
            spacing: 9
            Avatar {
                name: cv.convo ? cv.convo.name : ""
                tint: cv.convo ? cv.convo.color : theme.textSecondary
                size: 30
                ui: theme.ui
                group: cv.convo ? cv.convo.group === true : false
                members: cv.convo && cv.convo.members ? cv.convo.members : []
                anchors.verticalCenter: parent.verticalCenter
            }
            Column {
                anchors.verticalCenter: parent.verticalCenter
                spacing: 0
                Text {
                    text: cv.convo ? cv.convo.name : ""
                    color: theme.textPrimary
                    font.pixelSize: 15; font.weight: Font.DemiBold; font.family: theme.ui
                    horizontalAlignment: Text.AlignHCenter; anchors.horizontalCenter: parent.horizontalCenter
                }
                Text {
                    visible: cv.convo && cv.convo.group === true
                    text: cv.convo && cv.convo.members
                          ? ("You, " + cv.convo.members.map(function (m) { return m.name; }).join(", "))
                          : ""
                    color: theme.textSecondary
                    font.pixelSize: 11; font.family: theme.ui
                    elide: Text.ElideRight; horizontalAlignment: Text.AlignHCenter
                    anchors.horizontalCenter: parent.horizontalCenter
                    width: Math.min(implicitWidth, 280)
                }
            }
        }
        Rectangle {
            id: infoBtn
            width: 32; height: 32; radius: 16
            color: infoHover.hovered ? theme.hoverBg : "transparent"
            anchors.right: parent.right; anchors.rightMargin: 14
            anchors.verticalCenter: parent.verticalCenter
            scale: infoTap.pressed ? 0.9 : 1.0
            Behavior on scale { NumberAnimation { duration: 110; easing.type: Easing.OutQuad } }
            Rectangle {
                anchors.centerIn: parent
                width: 19; height: 19; radius: 9.5
                color: "transparent"; border.width: 1.6; border.color: theme.accent
                Text { anchors.centerIn: parent; text: "i"; color: theme.accent
                       font.pixelSize: 12; font.italic: true; font.family: theme.ui }
            }
            HoverHandler { id: infoHover }
            TapHandler { id: infoTap; onTapped: cv.openInfo() }
        }
        Rectangle {
            anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
            height: 1; color: theme.divider
        }
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
            onImageClicked: function (s) { cv.openImage(s); }
            onReplyRequested: cv.startReply(model.mine ? "You" : (model.sender ? model.sender : (cv.convo ? cv.convo.name : "")),
                                           model.kind === "image" ? "Photo" : model.text)
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
        onSend: function (t) { cv.send(t, composer.replyName, composer.replyText); composer.clearReply(); }
        onAttach: function (p) { cv.sendImage(p); }
    }
}
