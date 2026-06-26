# vendiMessage

A private, iMessage-inspired messenger for vendiOS. It's a **walled garden** built on
[Matrix](https://matrix.org): a closed homeserver with **federation off**, so only
vendiMessage users talk to each other and nothing is exposed to the wider Matrix
network. Full design notes: [`docs/vendi-messenger.md`](../../docs/vendi-messenger.md).

## Architecture

```
 ┌─────────────┐   WebSocket (127.0.0.1:8765)   ┌──────────────┐   matrix-sdk    ┌──────────────┐
 │ QML app     │ ─────── JSON protocol ───────► │ vendi-chatd  │ ──── HTTPS ───► │ homeserver   │
 │ (this dir)  │ ◄────── rooms/timeline ──────  │ (daemon)     │   (federation   │ continuwuity │
 └─────────────┘         message/status         └──────────────┘    off)         └──────────────┘
```

- **UI** — QML (this directory), run by `qml6`. A normal OS window; the compositor
  draws decorations.
- **Daemon** — [`src/vendi-chatd`](../../src/vendi-chatd) owns the Matrix session and
  exposes a line-delimited JSON protocol over a Unix socket **and** a WebSocket
  (QtWebSockets can't open Unix sockets). Build it with `--features matrix`.
- **Storage offload** — clients keep history + media; the homeserver keeps only a
  short retention window. Images are compressed (≤1600px, JPEG q72) before upload.

## Install

**On vendiOS:**

```sh
vendi install msg
```

Sets up the user service, a desktop entry, and a default homeserver, then launch
with `vendimessage` (or from the app menu).

**Anywhere (Arch / AUR):**

```sh
yay -S vendimessage-git        # PKGBUILD in pkg/vendimessage-git/
cp /usr/share/vendimessage/chat.conf.example ~/.config/vendi/chat.conf
systemctl --user enable --now vendi-chatd.service
vendimessage
```

First launch shows a **sign-in / create-account** screen (open sign-up).

## Features

- Sign in / **create account** against the homeserver (open registration).
- **Start a chat** by `@username`; incoming chats arrive as **requests** you
  Accept / Deny (Matrix invites).
- Text, **images** (drag-drop / picker, lightbox), jumbo emoji.
- **Replies** (nested quote) and emoji **reactions**.
- Typing indicators, group chats, per-message timestamps, scrollback.
- Bubbles follow the live **vendiOS accent** (`~/.config/vendi/theme-state`).
- **Log out** from the sidebar header (⎋).

## Develop (mock data, no homeserver)

```sh
cd apps/vendimessage/qml
QT_FORCE_STDERR_LOGGING=1 qml6 Main.qml
```

`QT_FORCE_STDERR_LOGGING=1` matters: without it the `qml` runtime hides real errors
behind "Did not load any objects, exiting." With no daemon reachable the app falls
back to mock conversations.

> Qt gotchas: `font.pixelSize` must be an **int** (no `14.5`); hex colours are
> `#AARRGGBB` (alpha first) — use `Qt.rgba(...)` for translucency; reading a local
> file via `XMLHttpRequest` needs `QML_XHR_ALLOW_FILE_READ=1` (the launcher sets it).

## Files

- `Main.qml` — window, theme (light/dark + live accent), backend wiring, overlays.
- `LoginPage.qml` — sign-in / create-account onboarding.
- `Sidebar.qml` — header (theme/logout/compose), search, requests, conversation list.
- `ChatView.qml` — header, message list, composer.
- `MessageBubble.qml` — bubbles, replies, reactions, images, jumbo emoji.
- `Composer.qml` — attachments, auto-growing input, reply banner, send.
- `Backend.qml` — the live WebSocket client to `vendi-chatd` (mock fallback).
- `Avatar.qml` / `RoundedImage.qml` / `TypingIndicator.qml` / `InfoPanel.qml` /
  `NewChatSheet.qml` / `ConversationRow.qml` — supporting components.
- `mockdata.js` — sample data (shape matches the daemon's protocol).
