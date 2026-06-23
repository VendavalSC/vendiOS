# vendiMessage — a beautiful messenger for vendiOS

Working name: **vendiMessage** (binary `vendi-chat`, daemon `vendi-chatd`).
Status: **design / not built**. Pick names before M1 if you want different ones.

A gorgeous, iMessage-inspired chat app native to vendiOS, plus an inline
quick-reply that drops out of the dynamic-island notch when a message arrives.

## Principles (inherited from the vendiOS ethos)

- **Free, open-source, no keys, no wallet risk.** Built on **Matrix** — an open,
  federated protocol. No API keys, no enterprise account, nothing metered. Users
  log into a free account on any homeserver. `matrix-rust-sdk` (Apache-2.0) is
  compiled in and vendorable into the offline mirror.
- **vendiOS hosts NOTHING.** We are a client. The network already exists.
- **Honest about privacy.** Content is end-to-end encrypted (Olm/Megolm — the
  homeserver only ever sees ciphertext). The homeserver does see **metadata**
  (who/when/which room). The app states this plainly; it does not pretend to be
  anonymous.
- **Opt-in, not in the base ISO** (like vendi AI) — `vendi chat install` pulls it.
- **Beautiful first.** The differentiator is the UI + the notch quick-reply, not
  the protocol. Heavily inspired by macOS iMessage (the look the user loves).

## Why Matrix (vs the alternatives we weighed)

- **Matrix (chosen):** biggest open network, mature Rust SDK that handles sync +
  E2E crypto + local store, provably legit (Matrix.org Foundation non-profit +
  Element enterprise funding; used by govts/militaries). Works offline (the
  server stores until you reconnect). Trade: metadata visible to the homeserver.
- SimpleX: best metadata privacy (no user IDs, dumb unlinkable relays) but smaller
  network + more integration work. Good future alternative backend.
- Tox: true serverless DHT P2P, no host to trust, but unreliable offline delivery
  + ugly IDs + niche. Rejected for a "works for normal people" messenger.
- Raw internet P2P: NAT traversal needs STUN/TURN relays and offline needs
  store-and-forward — i.e. it quietly reintroduces servers. Rejected.

## Architecture

Mirror the proven vendiOS pattern (a Rust worker + a QML surface, as with
`vendi-ai` ↔ the quickshell panel), but here the UI is a full standalone app.

```
┌─────────────────────────┐        ┌──────────────────────────────┐
│  vendi-chatd (Rust)      │        │  vendiMessage app (QML/Qt    │
│  matrix-rust-sdk         │  IPC   │  Quick) — the pretty client  │
│  • login / sync / E2E    │◀──────▶│  • sidebar + chat + bubbles  │
│  • SQLite store (sled)   │ local  │  • compose, media, reactions │
│  • rooms / timeline      │ socket │  • subscribes to the daemon  │
│  • send / receipts       │ (JSON) │                              │
└─────────────────────────┘        └──────────────────────────────┘
          ▲                                     
          │ same IPC                            
          ▼                                     
┌─────────────────────────┐  notch quick-reply lives in quickshell
│  quickshell notch        │  (reuse the AiContent/notification pattern):
│  notification + reply    │  message in → island expands → type → send
└─────────────────────────┘  → vendi-chatd, without opening the app.
```

- **Daemon** owns the Matrix session, crypto keys, and the local DB; it stays
  running (systemd user service) so messages arrive while the app is closed and
  the notch can show/reply to them.
- **IPC:** a local Unix socket speaking JSON lines (commands: `login`,
  `list_rooms`, `timeline`, `send`, `mark_read`, …; events: `message`, `typing`,
  `receipt`, `sync_state`). Same spirit as the vendi-ai stdin/stdout protocol,
  promoted to a socket because it's long-lived and multi-client (app + notch).
- **Rust↔QML:** evaluate `cxx-qt` for an in-process binding later; for the MVP the
  socket keeps the UI and the SDK cleanly decoupled and language-agnostic.

## UI (iMessage-inspired, vendiOS-clean)

Two-pane, exactly like the reference:
- **Sidebar:** search field; conversation rows = circular avatar + display name +
  last-message preview + timestamp; unread dot; selected row highlighted (accent).
- **Chat pane:** header (name, avatar, call/info buttons); message list with
  **rounded bubbles** — accent/blue for *sent* (right), neutral/gray for
  *received* (left), subtle tails, day separators, "Delivered/Read" under the last
  sent bubble; tapback **reactions** on long-press/hover; inline images.
- **Composer:** `+` attachments menu (Photos, stickers, files), text field,
  emoji. Send on Enter.
- Theme-driven colours (sent bubble = theme accent), respects light/dark and the
  vendiOS minimal aesthetic. Spring/scale animations on send (per the animations
  feedback — make it feel alive).

## MVP milestones

- **M1 — daemon core (CLI-testable, no UI):** `vendi-chatd` logs into a homeserver
  (user/pass), syncs, lists rooms, prints a room timeline, sends a text message.
  E2E working for encrypted rooms. Local store persists. Prove on the box.
- **M2 — the pretty client:** QML app, sidebar + chat pane + bubbles + live
  send/receive over the IPC socket. The iMessage look.
- **M3 — notch quick-reply + notifications:** message arrives → island expands
  with sender + preview + inline reply (reuse AiContent pattern); desktop
  notification; mark-read.
- **M4 — richness:** media send/view, reactions, read receipts, typing
  indicators, E2E device-verification UX, presence.

## Storage strategy — everything lives on the clients (the 128 GB Pi)

The Pi is just transport. **Both message history AND media live on the clients;**
the homeserver only retains a short rolling window so devices can sync, then it
purges. This is a great fit for Matrix because the client SDK already persists a
full local store.

**Message history (text):**
- `matrix-rust-sdk` keeps a local SQLite store (state + timeline) on each client,
  so `vendi-chatd` already has the full history locally and survives server purges.
- Turn on server-side **message retention** so old events drop off the Pi:
  ```toml
  # conduwuit.toml — purge events/media after a rolling window
  # (closed server, all users local; tune down to save more space)
  # message + media retention (exact keys vary by conduwuit version — verify on setup)
  cleanup_second_interval = 86400
  # rely on per-room m.room.retention (the client can set a default) +
  # conduwuit's media/transaction pruning.
  ```
- The client store is the source of truth for scrollback; the server is a relay
  that keeps just enough to deliver. Trade-off: a brand-new device only sees
  history from the retention window forward (older history exists only on the
  devices that already have it). Acceptable for a tiny-storage, privacy-leaning
  setup; a future "history export/import between your own devices" can bridge it.

**Media (images):**

- **Clients download & keep.** `vendi-chatd` downloads every incoming image into a
  local cache (`~/.cache/vendi-chat/media/`) and the UI shows it from there. Each
  client thus holds its own permanent copy.
- **Server keeps media only briefly.** Configure conduwuit media retention so the
  homeserver purges media after a short window (long enough for all recipients to
  sync + download). Rough conduwuit config:
  ```toml
  # conduwuit.toml
  max_request_size = 20_000_000          # cap uploads (~20 MB)
  media_startup_check = true
  # prune cached/remote media older than N days (closed server = all local users)
  media_retention_days = 14              # tune down to 7 / 3 to save more space
  ```
  (Exact keys depend on the conduwuit version — verify when we set up the Pi.)
- **Cap upload size** client-side too, and downscale very large images before
  upload (future) so neither the Pi nor client caches bloat.
- Net effect: the Pi only ever holds the last ~N days of media; history lives on
  the devices that received it. A re-download after purge isn't possible, which is
  the accepted trade for running on tiny storage.

## Risks / open questions

- E2E **key/device verification UX** is Matrix's roughest edge — design it simply
  (emoji-SAS verify, cross-signing) so normal users aren't lost.
- First-run **login** flow: pick-a-homeserver vs default to matrix.org; register
  in-app vs link out. Keep it one screen.
- Push/notifications without a Google FCM gateway → rely on the always-running
  daemon's sync loop (fine on a desktop OS).
- Naming + whether the notch reply is part of vendibar or a small companion.
