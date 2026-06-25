//! The chat backend behind the IPC layer. Three states:
//!   • NeedsAuth — no session yet; the app shows the login/create-account screen.
//!   • Mock      — in-memory rooms (default build, no homeserver).
//!   • Matrix    — real matrix-sdk client (compile with `--features matrix`).
//! The active state lives behind an `Arc<RwLock<…>>` so a runtime login/logout can
//! swap it while clients stay connected. All methods expose the same async surface
//! + a broadcast stream of pushed events, so the daemon and UI don't care which is
//! running.

use crate::protocol::{Message, Outgoing, Room};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

#[derive(Clone)]
pub struct Backend {
    events: broadcast::Sender<Outgoing>,
    inner: Arc<RwLock<Inner>>,
}

enum Inner {
    NeedsAuth,
    #[allow(dead_code)]
    Mock(MockState),
    #[cfg(feature = "matrix")]
    Matrix(crate::matrix::MatrixBackend),
}

struct MockState {
    rooms: Vec<Room>,
    msgs: HashMap<String, Vec<Message>>,
}

impl Backend {
    /// Build the backend. Matrix build with saved creds → logs in; matrix build
    /// without creds → NeedsAuth (login screen). Non-matrix build → mock.
    pub async fn new() -> anyhow::Result<Self> {
        let (events, _) = broadcast::channel(256);

        #[cfg(feature = "matrix")]
        let inner = match crate::matrix::MatrixBackend::from_config(events.clone()).await? {
            Some(b) => Inner::Matrix(b),
            None => {
                eprintln!("vendi-chatd: no saved session — waiting for sign-in");
                Inner::NeedsAuth
            }
        };
        #[cfg(not(feature = "matrix"))]
        let inner = Inner::Mock(MockState::seed());

        Ok(Self { events, inner: Arc::new(RwLock::new(inner)) })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Outgoing> {
        self.events.subscribe()
    }

    /// "needs_auth" while signed out, otherwise "ready".
    pub async fn status(&self) -> &'static str {
        match &*self.inner.read().await {
            Inner::NeedsAuth => "needs_auth",
            _ => "ready",
        }
    }

    // grab a clone of the live matrix backend (Client is cheap to clone), so we
    // never hold the lock across a network round-trip
    #[cfg(feature = "matrix")]
    async fn matrix(&self) -> Option<crate::matrix::MatrixBackend> {
        match &*self.inner.read().await {
            Inner::Matrix(m) => Some(m.clone()),
            _ => None,
        }
    }

    // ── auth ───────────────────────────────────────────────────────────────────
    #[cfg(feature = "matrix")]
    pub async fn login(&self, user: &str, password: &str, register: bool) -> anyhow::Result<()> {
        let b = crate::matrix::MatrixBackend::connect(self.events.clone(), user, password, register)
            .await?;
        *self.inner.write().await = Inner::Matrix(b);
        let rooms = self.list_rooms().await;
        let _ = self.events.send(Outgoing::Status { state: "ready".into() });
        let _ = self.events.send(Outgoing::Rooms { rooms });
        Ok(())
    }
    #[cfg(not(feature = "matrix"))]
    pub async fn login(&self, _u: &str, _p: &str, _r: bool) -> anyhow::Result<()> {
        anyhow::bail!("sign-in requires the matrix backend")
    }

    pub async fn logout(&self) -> anyhow::Result<()> {
        #[cfg(feature = "matrix")]
        crate::matrix::clear_config();
        *self.inner.write().await = Inner::NeedsAuth;
        let _ = self.events.send(Outgoing::Status { state: "needs_auth".into() });
        Ok(())
    }

    // ── rooms / messages ─────────────────────────────────────────────────────────
    pub async fn list_rooms(&self) -> Vec<Room> {
        #[cfg(feature = "matrix")]
        if let Some(m) = self.matrix().await {
            return m.list_rooms().await;
        }
        match &*self.inner.read().await {
            Inner::Mock(s) => s.rooms.clone(),
            _ => Vec::new(),
        }
    }

    pub async fn timeline(&self, room: &str, _limit: Option<u32>) -> Vec<Message> {
        #[cfg(feature = "matrix")]
        if let Some(m) = self.matrix().await {
            return m.timeline(room, _limit).await;
        }
        match &*self.inner.read().await {
            Inner::Mock(s) => s.msgs.get(room).cloned().unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    pub async fn send(&self, room: &str, body: &str, reply_to: Option<&str>) -> anyhow::Result<()> {
        #[cfg(feature = "matrix")]
        if let Some(m) = self.matrix().await {
            return m.send(room, body, reply_to).await;
        }
        let mut g = self.inner.write().await;
        if let Inner::Mock(s) = &mut *g {
            let mut msg = Message::text(
                format!("m{}", now_secs()), "me".into(), body.to_string(), true, "now".into(),
            );
            if let Some(r) = reply_to {
                msg.reply_to = r.to_string();
            }
            s.msgs.entry(room.to_string()).or_default().push(msg.clone());
            if let Some(r) = s.rooms.iter_mut().find(|r| r.id == room) {
                r.preview = body.to_string();
            }
            drop(g);
            let _ = self.events.send(Outgoing::Message { room: room.to_string(), message: msg });
        }
        Ok(())
    }

    pub async fn react(&self, room: &str, event_id: &str, key: &str) -> anyhow::Result<()> {
        #[cfg(feature = "matrix")]
        if let Some(m) = self.matrix().await {
            return m.react(room, event_id, key).await;
        }
        let mut g = self.inner.write().await;
        if let Inner::Mock(s) = &mut *g {
            if let Some(msgs) = s.msgs.get_mut(room) {
                if let Some(m) = msgs.iter_mut().find(|m| m.id == event_id) {
                    m.reactions.push(key.to_string());
                }
            }
        }
        Ok(())
    }

    /// Send an image. The client-cache model: copy/keep the file locally and
    /// reference that path, so the server only holds the upload transiently.
    pub async fn send_image(&self, room: &str, path: &str) -> anyhow::Result<()> {
        let src = path.strip_prefix("file://").unwrap_or(path);
        #[cfg(feature = "matrix")]
        if let Some(m) = self.matrix().await {
            return m.send_image(room, src).await;
        }
        let cached = cache_copy(src)?;
        let mut g = self.inner.write().await;
        if let Inner::Mock(s) = &mut *g {
            let msg = Message::image(
                format!("m{}", now_secs()), "me".into(), true, "now".into(), cached,
            );
            s.msgs.entry(room.to_string()).or_default().push(msg.clone());
            if let Some(r) = s.rooms.iter_mut().find(|r| r.id == room) {
                r.preview = "📷 Photo".into();
            }
            drop(g);
            let _ = self.events.send(Outgoing::Message { room: room.to_string(), message: msg });
        }
        Ok(())
    }

    // ── invites / new chats ──────────────────────────────────────────────────────
    pub async fn start_chat(&self, user: &str) -> anyhow::Result<()> {
        #[cfg(feature = "matrix")]
        if let Some(m) = self.matrix().await {
            m.start_chat(user).await?;
            let rooms = self.list_rooms().await;
            let _ = self.events.send(Outgoing::Rooms { rooms });
            return Ok(());
        }
        let _ = user;
        Ok(())
    }

    pub async fn accept_invite(&self, room: &str) -> anyhow::Result<()> {
        #[cfg(feature = "matrix")]
        if let Some(m) = self.matrix().await {
            m.accept_invite(room).await?;
            let rooms = self.list_rooms().await;
            let _ = self.events.send(Outgoing::Rooms { rooms });
            return Ok(());
        }
        let _ = room;
        Ok(())
    }

    pub async fn reject_invite(&self, room: &str) -> anyhow::Result<()> {
        #[cfg(feature = "matrix")]
        if let Some(m) = self.matrix().await {
            m.reject_invite(room).await?;
            let rooms = self.list_rooms().await;
            let _ = self.events.send(Outgoing::Rooms { rooms });
            return Ok(());
        }
        let _ = room;
        Ok(())
    }

    pub async fn mark_read(&self, room: &str) -> anyhow::Result<()> {
        #[cfg(feature = "matrix")]
        if let Some(m) = self.matrix().await {
            return m.mark_read(room).await;
        }
        let mut g = self.inner.write().await;
        if let Inner::Mock(s) = &mut *g {
            if let Some(r) = s.rooms.iter_mut().find(|r| r.id == room) {
                r.unread = 0;
            }
        }
        Ok(())
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl MockState {
    #[allow(dead_code)]
    fn seed() -> Self {
        let rooms = vec![
            Room { id: "!armando".into(), name: "Armando Cajide".into(),
                   preview: "That's awesome!".into(), unread: 0, color: "#5b7cfa".into(), invite: false },
            Room { id: "!ariel".into(), name: "Ariel".into(),
                   preview: "see you tomorrow!".into(), unread: 1, color: "#f0883e".into(), invite: false },
            Room { id: "!zoe".into(), name: "Zoe".into(),
                   preview: "haha that's so true".into(), unread: 0, color: "#bc6bd9".into(), invite: false },
        ];
        let mut msgs = HashMap::new();
        msgs.insert("!armando".into(), vec![
            mk("a1", "Armando", "Hey!", false),
            mk("a2", "Armando", "I got a new 🐶", false),
            mk("a3", "me", "Hey Armando!", true),
            mk("a4", "me", "It was great catching up with you the other day.", true),
        ]);
        msgs.insert("!ariel".into(), vec![
            mk("b1", "Ariel", "are we still on for tomorrow?", false),
            mk("b2", "me", "yep! 10am works", true),
            mk("b3", "Ariel", "see you tomorrow!", false),
        ]);
        Self { rooms, msgs }
    }
}

#[allow(dead_code)]
fn mk(id: &str, sender: &str, body: &str, mine: bool) -> Message {
    Message::text(id.into(), sender.into(), body.into(), mine, "9:00 AM".into())
}

/// The on-client media cache — downloaded/sent images live here so the homeserver
/// can purge its copy (storage offloaded to clients).
pub fn media_cache_dir() -> std::path::PathBuf {
    let base = dirs::cache_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    let dir = base.join("vendi-chat/media");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Copy a picked file into the media cache; returns the cached path.
#[allow(dead_code)]
fn cache_copy(src: &str) -> anyhow::Result<String> {
    let p = std::path::Path::new(src);
    let name = p.file_name().map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| format!("img{}", now_secs()));
    let dest = media_cache_dir().join(format!("{}-{}", now_secs(), name));
    std::fs::copy(p, &dest)?;
    Ok(dest.to_string_lossy().to_string())
}
