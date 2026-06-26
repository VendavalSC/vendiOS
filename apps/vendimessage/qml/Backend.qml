// Backend — the live data source: a WebSocket client to vendi-chatd. Exposes a
// `conversations` model in the same shape the UI uses, populated from the
// daemon's rooms/timeline/message events. When the daemon isn't reachable,
// `connected` is false and Main falls back to mock data.

import QtQuick
import QtWebSockets

Item {
    id: be
    property bool connected: false      // socket open
    property bool authed: false         // signed in (daemon has a session)
    property var conversations: []
    property var requests: []           // pending chat requests (invites)
    property var searchResults: []      // user-directory search hits
    property string lastError: ""
    signal errored(string message)
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

    // a daemon ts is epoch-millis (from matrix) or a plain label (from mock)
    function _fmtTs(ts) {
        if (!ts) return "";
        if (/^\d+$/.test(String(ts))) return Qt.formatDateTime(new Date(parseInt(ts)), "h:mm AP");
        return ts;
    }
    // "@armando:vendi.chat" → "Armando"
    function _name(mxid) {
        var s = String(mxid || "");
        var m = /^@([^:]+):/.exec(s);
        var u = m ? m[1] : s.replace(/^@/, "");
        return u.length ? u.charAt(0).toUpperCase() + u.slice(1) : u;
    }
    // map a daemon message → the UI message shape
    function _map(d) {
        return {
            id: d.id || "", text: d.body || "", mine: d.mine === true, time: _fmtTs(d.ts),
            kind: d.kind || "text", source: d.media || "",
            replyTo: d.reply_to || "", replyName: "", replyText: "",
            sender: d.sender || "", senderColor: "",
            reactions: JSON.stringify(d.reactions || [])
        };
    }
    // fill in the quoted sender/text for any reply, resolved from the same thread
    function _resolveReplies(msgs) {
        var byId = {};
        for (var i = 0; i < msgs.length; i++) if (msgs[i].id) byId[msgs[i].id] = msgs[i];
        for (var j = 0; j < msgs.length; j++) {
            var t = msgs[j].replyTo ? byId[msgs[j].replyTo] : null;
            if (t) {
                msgs[j].replyName = t.mine ? "You" : _name(t.sender);
                msgs[j].replyText = t.kind === "image" ? "Photo" : t.text;
            }
        }
        return msgs;
    }
    function _find(id) {
        for (var i = 0; i < conversations.length; i++)
            if (conversations[i].id === id) return i;
        return -1;
    }

    function _onMessage(text) {
        var m; try { m = JSON.parse(text); } catch (e) { return; }
        if (m.type === "status") {
            be.authed = (m.state === "ready");
            if (be.authed) _send({ cmd: "list_rooms" });
            else { conversations = []; requests = []; }
        } else if (m.type === "error") {
            be.lastError = m.message || "Something went wrong";
            be.errored(be.lastError);
        } else if (m.type === "search_results") {
            be.searchResults = m.users || [];
        } else if (m.type === "rooms") {
            var convos = [], reqs = [];
            for (var i = 0; i < m.rooms.length; i++) {
                var r = m.rooms[i];
                var o = { id: r.id, name: r.name, color: r.color || "#7d8590",
                          preview: r.preview || "", time: "", unread: r.unread || 0,
                          group: false, members: [], typing: false, messages: [],
                          invite: r.invite === true, peer: r.peer || "" };
                if (o.invite) reqs.push(o); else convos.push(o);
            }
            conversations = convos; requests = reqs;
        } else if (m.type === "timeline") {
            var idx = _find(m.room); if (idx < 0) return;
            var cs = conversations.slice();
            var c = Object.assign({}, cs[idx]);
            c.messages = _resolveReplies((m.messages || []).map(_map));
            cs[idx] = c; conversations = cs;
        } else if (m.type === "message") {
            var j = _find(m.room); if (j < 0) return;
            var cs2 = conversations.slice();
            var c2 = Object.assign({}, cs2[j]);
            c2.messages = _resolveReplies(c2.messages.concat([_map(m.message)]));
            c2.preview = m.message.body || (m.message.kind === "image" ? "📷 Photo" : "");
            cs2[j] = c2; conversations = cs2;
        }
    }

    // public API
    function loadRoom(id) { _send({ cmd: "timeline", room: id }); }
    function send(id, text, replyTo) {
        _send({ cmd: "send", room: id, body: text, reply_to: replyTo || null });
    }
    function sendImage(id, path) { _send({ cmd: "send_image", room: id, path: path }); }
    function react(id, eventId, key) {
        if (eventId) _send({ cmd: "react", room: id, event_id: eventId, key: key });
    }
    function markRead(id) { _send({ cmd: "mark_read", room: id }); }

    // auth + people
    function register(user, password) { _send({ cmd: "register", user: user, password: password }); }
    function login(user, password) { _send({ cmd: "login", user: user, password: password }); }
    function logout() { _send({ cmd: "logout" }); }
    function startChat(user) { _send({ cmd: "start_chat", user: user }); }
    function acceptInvite(id) { _send({ cmd: "accept_invite", room: id }); }
    function rejectInvite(id) { _send({ cmd: "reject_invite", room: id }); }
    function searchUsers(query) {
        if (query && query.length) _send({ cmd: "search_users", query: query });
        else searchResults = [];
    }
    function block(user) { _send({ cmd: "block", user: user }); }
    function unblock(user) { _send({ cmd: "unblock", user: user }); }
}
