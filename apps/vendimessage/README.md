# vendiMessage

An iMessage-inspired, vendiOS-native messenger. Walled + vendi-branded, built on
Matrix (closed homeserver, federation off) so vendiOS hosts nothing public-facing.
Full design: [`docs/vendi-messenger.md`](../../docs/vendi-messenger.md).

This directory is the **UI** (QML). The **daemon** lives at
[`src/vendi-chatd`](../../src/vendi-chatd) and owns the Matrix session + a local
Unix-socket JSON protocol that the UI and the notch quick-reply both speak.

## Run the UI (dev, mock data)

```sh
cd apps/vendimessage/qml
QT_FORCE_STDERR_LOGGING=1 qml6 Main.qml
```

`QT_FORCE_STDERR_LOGGING=1` is important: without it the `qml` runtime hides real
errors behind a useless "Did not load any objects, exiting."

> Qt gotcha: `font.pixelSize` must be an **int** (no `14.5`), and hex colours are
> `#AARRGGBB` (alpha first) — use `Qt.rgba(...)` for translucency.

## Files

- `Main.qml` — window; two-pane layout; theme (light/dark); mock backend.
- `Sidebar.qml` — header + search + conversation list.
- `ConversationRow.qml` — avatar, name, preview, time, unread dot.
- `ChatView.qml` — header + message list + composer.
- `MessageBubble.qml` — accent (sent, right) / gray (received, left) bubbles.
- `Composer.qml` — `+` attachments, text field, send.
- `Avatar.qml` — circular monogram.
- `mockdata.js` — sample conversations (shape matches the daemon's IPC).

## Wiring to the daemon (next)

`Main.qml`'s `backend` object is currently mock. The real version connects to the
daemon socket (`$XDG_RUNTIME_DIR/vendi-chat.sock`) and maps:
`list_rooms`→sidebar, `timeline`→thread, `send`→composer, pushed `message`
events→live append. In quickshell that's `Quickshell.Io.Socket`; a standalone Qt
host can use a small Rust/C++ bridge.
