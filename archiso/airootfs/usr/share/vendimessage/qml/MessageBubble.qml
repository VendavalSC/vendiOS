// MessageBubble — one message: a text bubble (accent gradient when sent, gray
// when received), a jumbo-emoji message (no bubble), or an image. Supports a
// nested reply quote, group sender label/avatar, a per-run timestamp, and emoji
// reactions. Hovering reveals reply + react actions.

import QtQuick

Item {
    id: b
    property var theme
    property string msgId: ""
    property string text: ""
    property string time: ""
    property bool mine: false
    property bool showTail: true
    property bool delivered: false
    property string kind: "text"
    property string source: ""
    property bool groupStart: false
    property string replyName: ""
    property string replyText: ""
    property string senderName: ""
    property color senderColor: "#888"
    property bool showSender: false
    property bool showAvatar: false
    property int gutter: 0
    property string reactions: "[]"   // JSON array of emoji
    signal imageClicked(string source)
    signal replyRequested()
    signal react(string emoji)

    readonly property real maxW: width * 0.68
    readonly property int topGap: groupStart ? 10 : 0
    implicitHeight: col.height + topGap

    // softened sender name colour (the raw member colours were too loud)
    readonly property color softSender: Qt.rgba(senderColor.r * 0.62 + 0.16,
                                                senderColor.g * 0.62 + 0.16,
                                                senderColor.b * 0.62 + 0.16, 1)

    // a text message of only 1–3 emoji renders big with no bubble
    readonly property int emojiCount: kind === "text" ? emojiOnly(text) : 0
    readonly property bool jumbo: emojiCount >= 1 && emojiCount <= 3
    function emojiOnly(s) {
        var t = String(s).replace(/\s+/g, "");
        if (!t.length) return 0;
        var any = /[\u{1F1E6}-\u{1F1FF}\u{1F300}-\u{1FAFF}\u{2190}-\u{21FF}\u{2300}-\u{27BF}\u{2B00}-\u{2BFF}\u{2600}-\u{26FF}\u{FE00}-\u{FE0F}\u{200D}\u{20E3}\u{2139}\u{2122}]/gu;
        if (t.replace(any, "").length > 0) return 0;
        var base = t.match(/[\u{1F300}-\u{1FAFF}\u{2600}-\u{27BF}\u{2B00}-\u{2BFF}\u{2190}-\u{21FF}\u{1F1E6}-\u{1F1FF}]/gu);
        return base ? base.length : 0;
    }

    readonly property var rx: { try { return JSON.parse(b.reactions || "[]"); } catch (e) { return []; } }
    function groupedReactions() {
        var m = {}, order = [];
        for (var i = 0; i < rx.length; i++) {
            var e = rx[i];
            if (!(e in m)) { m[e] = 0; order.push(e); }
            m[e]++;
        }
        return order.map(function (e) { return { emoji: e, count: m[e] }; });
    }

    property bool pickerOpen: false

    HoverHandler { id: hover }

    // group: author avatar in the left gutter
    Avatar {
        visible: b.showAvatar
        name: b.senderName; tint: b.senderColor; size: 24; ui: b.theme.ui
        anchors.left: parent.left; anchors.bottom: bubbleRow.bottom
    }

    Column {
        id: col
        y: b.topGap
        spacing: 2
        anchors.right: b.mine ? parent.right : undefined
        anchors.left: b.mine ? undefined : parent.left
        anchors.leftMargin: b.mine ? 0 : b.gutter

        // group sender name (softened)
        Text {
            visible: b.showSender
            text: b.senderName
            color: b.softSender
            font.pixelSize: 12; font.weight: Font.DemiBold; font.family: b.theme.ui
            leftPadding: 4
        }

        // the message body (jumbo / bubble / image) lives in a row so reactions
        // can tuck under its trailing corner
        Item {
            id: bubbleRow
            // size explicitly to the active body (childrenRect + the bottom-anchored
            // reaction row would form a binding loop)
            width: b.jumbo ? jumboT.width : (b.kind === "image" ? pic.width : bubble.width)
            height: b.jumbo ? jumboT.height : (b.kind === "image" ? pic.height : bubble.height)
            anchors.right: b.mine ? parent.right : undefined
            anchors.left: b.mine ? undefined : parent.left

            // ── jumbo emoji ──
            Text {
                id: jumboT
                visible: b.jumbo
                text: b.text
                font.pixelSize: b.emojiCount === 1 ? 46 : (b.emojiCount === 2 ? 38 : 32)
                font.family: b.theme.ui
            }

            // ── text bubble (with optional nested reply quote) ──
            Rectangle {
                id: bubble
                visible: b.kind !== "image" && !b.jumbo
                readonly property bool hasReply: b.replyText.length > 0
                radius: 18
                gradient: Gradient {
                    GradientStop { position: 0.0; color: b.mine ? b.theme.accent : b.theme.bubbleIn }
                    GradientStop { position: 1.0; color: b.mine ? b.theme.accent2 : b.theme.bubbleIn }
                }
                implicitWidth: Math.max(label.implicitWidth, hasReply ? 172 : 0) + 26
                implicitHeight: inner.implicitHeight + 16
                width: Math.min(implicitWidth, b.maxW)

                Column {
                    id: inner
                    x: 13; y: 8
                    width: bubble.width - 26
                    spacing: 6
                    Rectangle {
                        visible: bubble.hasReply
                        width: parent.width; height: qcol.implicitHeight + 12; radius: 10
                        color: b.mine ? Qt.rgba(1, 1, 1, 0.16) : b.theme.hoverBg
                        Rectangle { width: 3; height: parent.height - 12; radius: 1.5; x: 8
                                    anchors.verticalCenter: parent.verticalCenter
                                    color: b.mine ? Qt.rgba(1, 1, 1, 0.85) : b.theme.accent }
                        Column {
                            id: qcol; x: 19; width: parent.width - 30
                            anchors.verticalCenter: parent.verticalCenter; spacing: 1
                            Text { text: b.replyName; font.pixelSize: 12; font.weight: Font.DemiBold
                                   font.family: b.theme.ui; width: parent.width; elide: Text.ElideRight
                                   color: b.mine ? Qt.rgba(1, 1, 1, 0.95) : b.theme.accent }
                            Text { text: b.replyText; font.pixelSize: 12; font.family: b.theme.ui
                                   width: parent.width; elide: Text.ElideRight
                                   color: b.mine ? Qt.rgba(1, 1, 1, 0.75) : b.theme.textSecondary }
                        }
                    }
                    Text {
                        id: label
                        width: parent.width; text: b.text
                        color: b.mine ? b.theme.bubbleOutText : b.theme.bubbleInText
                        wrapMode: Text.Wrap; font.pixelSize: 15; font.family: b.theme.ui
                    }
                }
            }

            // ── image ──
            RoundedImage {
                id: pic
                visible: b.kind === "image"
                source: b.kind === "image" ? b.source : ""
                radius: 18
                width: Math.min(248, b.maxW)
                height: Math.max(120, Math.min(320, width * ratio))
                scale: picTap.pressed ? 0.97 : 1.0
                Behavior on scale { NumberAnimation { duration: 110; easing.type: Easing.OutQuad } }
                TapHandler { id: picTap; onTapped: b.imageClicked(b.source) }
            }

            // ── reaction badges, tucked under the trailing corner ──
            Row {
                id: rxRow
                visible: b.rx.length > 0
                spacing: 3
                anchors.right: b.mine ? parent.right : undefined
                anchors.left: b.mine ? undefined : parent.left
                anchors.rightMargin: b.mine ? 8 : 0
                anchors.leftMargin: b.mine ? 0 : 8
                anchors.top: parent.bottom
                anchors.topMargin: -10
                Repeater {
                    model: b.groupedReactions()
                    Rectangle {
                        height: 21; width: rxText.width + (modelData.count > 1 ? cnt.width + 10 : 12); radius: 11
                        color: b.theme.bubbleIn
                        border.width: 1.5; border.color: b.theme.windowBg
                        Row {
                            anchors.centerIn: parent; spacing: 2
                            Text { id: rxText; text: modelData.emoji; font.pixelSize: 12 }
                            Text { id: cnt; visible: modelData.count > 1; text: modelData.count
                                   color: b.theme.textSecondary; font.pixelSize: 11; font.family: b.theme.ui
                                   anchors.verticalCenter: parent.verticalCenter }
                        }
                        scale: 0
                        Component.onCompleted: scale = 1
                        Behavior on scale { NumberAnimation { duration: 220; easing.type: Easing.OutBack } }
                    }
                }
            }
        }

        // reserve room for the reaction badge that overhangs the bubble's bottom
        // so it doesn't crowd the next (tightly-spaced) message
        Item { width: 1; height: (b.rx.length > 0 && !b.showTail) ? 14 : 0 }

        // per-run timestamp / delivered
        Text {
            visible: b.showTail
            text: (b.delivered && b.mine) ? ("Delivered · " + b.time) : b.time
            color: b.theme.textSecondary
            font.pixelSize: 10; font.family: b.theme.ui
            topPadding: b.rx.length > 0 ? 13 : 1
            leftPadding: 4; rightPadding: 4
            anchors.right: b.mine ? parent.right : undefined
            anchors.left: b.mine ? undefined : parent.left
        }
    }

    // ── hover actions (react + reply) ──
    Row {
        id: actions
        visible: hover.hovered || b.pickerOpen
        spacing: 4
        anchors.verticalCenter: bubbleRow.verticalCenter
        anchors.left: b.mine ? undefined : col.right
        anchors.leftMargin: b.mine ? 0 : 6
        anchors.right: b.mine ? col.left : undefined
        anchors.rightMargin: b.mine ? 6 : 0

        ActionButton { icon: "react"; theme: b.theme; onClicked: b.pickerOpen = !b.pickerOpen }
        ActionButton { icon: "reply"; theme: b.theme; onClicked: b.replyRequested() }
    }

    // ── reaction picker popup ──
    Rectangle {
        id: picker
        visible: b.pickerOpen
        z: 50
        width: pr.width + 16; height: 38; radius: 19
        color: b.theme.windowBg
        border.width: 1; border.color: b.theme.divider
        anchors.bottom: bubbleRow.top; anchors.bottomMargin: 6
        anchors.left: b.mine ? undefined : col.left
        anchors.right: b.mine ? col.right : undefined
        scale: b.pickerOpen ? 1 : 0.7
        opacity: b.pickerOpen ? 1 : 0
        transformOrigin: Item.Bottom
        Behavior on scale { NumberAnimation { duration: 170; easing.type: Easing.OutBack } }
        Behavior on opacity { NumberAnimation { duration: 130 } }
        Row {
            id: pr
            anchors.centerIn: parent; spacing: 4
            Repeater {
                model: ["❤️", "👍", "😂", "😮", "😢", "🙏"]
                Rectangle {
                    width: 28; height: 28; radius: 14
                    color: emoHover.hovered ? b.theme.hoverBg : "transparent"
                    Text { anchors.centerIn: parent; text: modelData; font.pixelSize: 17 }
                    scale: emoHover.hovered ? 1.25 : 1.0
                    Behavior on scale { NumberAnimation { duration: 110; easing.type: Easing.OutBack } }
                    HoverHandler { id: emoHover }
                    TapHandler { onTapped: { b.react(modelData); b.pickerOpen = false; } }
                }
            }
        }
    }

    // small circular hover-action button with a drawn icon
    component ActionButton: Rectangle {
        property var theme
        property string icon: "reply"
        signal clicked()
        width: 26; height: 26; radius: 13
        color: abHover.hovered ? theme.hoverBg : "transparent"
        scale: abTap.pressed ? 0.85 : 1.0
        Behavior on scale { NumberAnimation { duration: 100; easing.type: Easing.OutQuad } }
        Canvas {
            anchors.centerIn: parent; width: 16; height: 15
            onPaint: {
                var ctx = getContext("2d");
                ctx.clearRect(0, 0, width, height);
                ctx.strokeStyle = theme.textSecondary; ctx.fillStyle = theme.textSecondary;
                ctx.lineWidth = 1.5; ctx.lineCap = "round"; ctx.lineJoin = "round";
                if (icon === "reply") {
                    ctx.beginPath(); ctx.moveTo(5, 2); ctx.lineTo(1, 6); ctx.lineTo(5, 10); ctx.stroke();
                    ctx.beginPath(); ctx.moveTo(1.5, 6); ctx.lineTo(9, 6);
                    ctx.quadraticCurveTo(14, 6, 14, 10.5); ctx.lineTo(14, 13); ctx.stroke();
                } else { // react: a little smiley
                    ctx.beginPath(); ctx.arc(8, 7.5, 6, 0, Math.PI * 2); ctx.stroke();
                    ctx.beginPath(); ctx.arc(5.7, 6, 0.9, 0, Math.PI * 2); ctx.fill();
                    ctx.beginPath(); ctx.arc(10.3, 6, 0.9, 0, Math.PI * 2); ctx.fill();
                    ctx.beginPath(); ctx.arc(8, 8, 3, 0.15 * Math.PI, 0.85 * Math.PI); ctx.stroke();
                }
            }
            Connections { target: theme; function onTextSecondaryChanged() { parent.requestPaint() } }
        }
        HoverHandler { id: abHover }
        TapHandler { id: abTap; onTapped: parent.clicked() }
    }
}
