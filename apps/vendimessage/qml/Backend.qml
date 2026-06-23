// Backend — the live data source: a WebSocket client to vendi-chatd. Exposes a
// `conversations` model in the same shape the UI uses, populated from the
// daemon's rooms/timeline/message events. When the daemon isn't reachable,
// `connected` is false and Main falls back to mock data.

import QtQuick
import QtWebSockets

Item {
    id: be
    property bool connected: false
    property var conversations: []
    property string url: "ws://127.0.0.1:8765"

    WebSocket {
        id: sock
        url: be.url
        active: true
        onStatusChanged: {
            if (status === WebSocket.Open) {
                be.connected = true;
                _send({ cmd: "list_rooms" });
            } else if (status === WebSocket.Closed || status === WebSocket.Error) {
                be.connected = false;
                reconnect.restart();
            }
        }
        onTextMessageReceived: function (message) { be._onMessage(message); }
    }
    Timer { id: reconnect; interval: 1500; onTriggered: { sock.active = false; sock.active = true; } }

    function _send(obj) {
        if (sock.status === WebSocket.Open) sock.sendTextMessage(JSON.stringify(obj));
    }

    // map a daemon message → the UI message shape
    function _map(d) {
        return {
            text: d.body || "", mine: d.mine === true, time: d.ts || "",
            kind: d.kind || "text", source: d.media || "",
            replyName: "", replyText: "", sender: d.sender || "",
            senderColor: "", reactions: "[]"
        };
    }
    function _find(id) {
        for (var i = 0; i < conversations.length; i++)
            if (conversations[i].id === id) return i;
        return -1;
    }

    function _onMessage(text) {
        var m; try { m = JSON.parse(text); } catch (e) { return; }
        if (m.type === "rooms") {
            var out = [];
            for (var i = 0; i < m.rooms.length; i++) {
                var r = m.rooms[i];
                out.push({ id: r.id, name: r.name, color: r.color || "#7d8590",
                           preview: r.preview || "", time: "", unread: r.unread || 0,
                           group: false, members: [], typing: false, messages: [] });
            }
            conversations = out;
        } else if (m.type === "timeline") {
            var idx = _find(m.room); if (idx < 0) return;
            var cs = conversations.slice();
            var c = Object.assign({}, cs[idx]);
            c.messages = (m.messages || []).map(_map);
            cs[idx] = c; conversations = cs;
        } else if (m.type === "message") {
            var j = _find(m.room); if (j < 0) return;
            var cs2 = conversations.slice();
            var c2 = Object.assign({}, cs2[j]);
            c2.messages = c2.messages.concat([_map(m.message)]);
            c2.preview = m.message.body || (m.message.kind === "image" ? "📷 Photo" : "");
            cs2[j] = c2; conversations = cs2;
        }
    }

    // public API
    function loadRoom(id) { _send({ cmd: "timeline", room: id }); }
    function send(id, text) { _send({ cmd: "send", room: id, body: text }); }
    function sendImage(id, path) { _send({ cmd: "send_image", room: id, path: path }); }
}
