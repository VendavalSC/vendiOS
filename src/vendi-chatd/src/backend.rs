//! The chat backend behind the IPC layer. Two implementations:
//!   • Mock  — in-memory rooms; runs with no homeserver (default build).
//!   • Matrix — real matrix-sdk client (compile with `--features matrix`).
//! Both expose the same async surface + a broadcast stream of pushed events, so
//! the daemon and the UI don't care which is running.

use crate::protocol::{Message, Outgoing, Room};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct Backend {
    events: broadcast::Sender<Outgoing>,
    inner: Inner,
}

#[derive(Clone)]
enum Inner {
    Mock(Arc<Mutex<MockState>>),
    #[cfg(feature = "matrix")]
    Matrix(crate::matrix::MatrixBackend),
}

struct MockState {
    rooms: Vec<Room>,
    msgs: HashMap<String, Vec<Message>>,
}

impl Backend {
    /// Build the backend. Uses Matrix when compiled with the feature AND a
    /// config/credentials exist; otherwise the mock.
    pub async fn new() -> anyhow::Result<Self> {
        let (events, _) = broadcast::channel(256);

        #[cfg(feature = "matrix")]
        {
            if let Some(b) = crate::matrix::MatrixBackend::from_config(events.clone()).await? {
                return Ok(Self { events, inner: Inner::Matrix(b) });
            }
            eprintln!("vendi-chatd: no chat config found — running the mock backend");
        }

        Ok(Self { events, inner: Inner::Mock(Arc::new(Mutex::new(MockState::seed()))) })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Outgoing> {
        self.events.subscribe()
    }

    pub async fn list_rooms(&self) -> Vec<Room> {
        match &self.inner {
            Inner::Mock(s) => s.lock().unwrap().rooms.clone(),
            #[cfg(feature = "matrix")]
            Inner::Matrix(m) => m.list_rooms().await,
        }
    }

    pub async fn timeline(&self, room: &str, _limit: Option<u32>) -> Vec<Message> {
        match &self.inner {
            Inner::Mock(s) => s.lock().unwrap().msgs.get(room).cloned().unwrap_or_default(),
            #[cfg(feature = "matrix")]
            Inner::Matrix(m) => m.timeline(room, _limit).await,
        }
    }

    pub async fn send(&self, room: &str, body: &str) -> anyhow::Result<()> {
        match &self.inner {
            Inner::Mock(s) => {
                let msg = Message::text(
                    format!("m{}", now_secs()), "me".into(), body.to_string(), true, "now".into(),
                );
                {
                    let mut st = s.lock().unwrap();
                    st.msgs.entry(room.to_string()).or_default().push(msg.clone());
                    if let Some(r) = st.rooms.iter_mut().find(|r| r.id == room) {
                        r.preview = body.to_string();
                    }
                }
                let _ = self.events.send(Outgoing::Message { room: room.to_string(), message: msg });
                Ok(())
            }
            #[cfg(feature = "matrix")]
            Inner::Matrix(m) => m.send(room, body).await,
        }
    }

    /// Send an image. The client-cache model: we copy/keep the file in the local
    /// media cache and reference that path, so the server only needs to hold the
    /// upload transiently (a short media-retention window) — clients keep it.
    pub async fn send_image(&self, room: &str, path: &str) -> anyhow::Result<()> {
        let src = path.strip_prefix("file://").unwrap_or(path);
        match &self.inner {
            Inner::Mock(s) => {
                let cached = cache_copy(src)?;
                let msg = Message::image(
                    format!("m{}", now_secs()), "me".into(), true, "now".into(), cached,
                );
                {
                    let mut st = s.lock().unwrap();
                    st.msgs.entry(room.to_string()).or_default().push(msg.clone());
                    if let Some(r) = st.rooms.iter_mut().find(|r| r.id == room) {
                        r.preview = "📷 Photo".into();
                    }
                }
                let _ = self.events.send(Outgoing::Message { room: room.to_string(), message: msg });
                Ok(())
            }
            #[cfg(feature = "matrix")]
            Inner::Matrix(m) => m.send_image(room, src).await,
        }
    }

    pub async fn mark_read(&self, room: &str) -> anyhow::Result<()> {
        match &self.inner {
            Inner::Mock(s) => {
                if let Some(r) = s.lock().unwrap().rooms.iter_mut().find(|r| r.id == room) {
                    r.unread = 0;
                }
                Ok(())
            }
            #[cfg(feature = "matrix")]
            Inner::Matrix(m) => m.mark_read(room).await,
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl MockState {
    fn seed() -> Self {
        let rooms = vec![
            Room { id: "!armando".into(), name: "Armando Cajide".into(),
                   preview: "That's awesome!".into(), unread: 0, color: "#5b7cfa".into() },
            Room { id: "!ariel".into(), name: "Ariel".into(),
                   preview: "see you tomorrow!".into(), unread: 1, color: "#f0883e".into() },
            Room { id: "!zoe".into(), name: "Zoe".into(),
                   preview: "haha that's so true".into(), unread: 0, color: "#bc6bd9".into() },
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
fn cache_copy(src: &str) -> anyhow::Result<String> {
    let p = std::path::Path::new(src);
    let name = p.file_name().map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| format!("img{}", now_secs()));
    let dest = media_cache_dir().join(format!("{}-{}", now_secs(), name));
    std::fs::copy(p, &dest)?;
    Ok(dest.to_string_lossy().to_string())
}
