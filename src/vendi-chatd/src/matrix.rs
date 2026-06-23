//! Real backend on matrix-sdk (compile with `--features matrix`).
//!
//! Credentials come from ~/.config/vendi/chat.conf (simple key=value):
//!     homeserver = https://chat.vendios.example
//!     user       = alonso
//!     password   = ...
//! When the file is absent, `from_config` returns None and the daemon falls
//! back to the mock backend.
//!
//! Status: first cut — login, background sync, live incoming messages, send and
//! room list are wired. Scrollback (timeline history) is a TODO: it wants the
//! matrix-sdk-ui Timeline; for now it returns the empty set and live messages
//! stream in via the sync handler.

use crate::protocol::{Message, Outgoing, Room};
use crate::backend::media_cache_dir;
use matrix_sdk::{
    attachment::AttachmentConfig,
    config::SyncSettings,
    media::{MediaFormat, MediaRequest},
    ruma::events::room::message::{MessageType, RoomMessageEventContent, SyncRoomMessageEvent},
    ruma::RoomId,
    Client, Room as SdkRoom,
};
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct MatrixBackend {
    client: Client,
    #[allow(dead_code)]
    events: broadcast::Sender<Outgoing>,
}

struct Config {
    homeserver: String,
    user: String,
    password: String,
}

impl MatrixBackend {
    pub async fn from_config(events: broadcast::Sender<Outgoing>) -> anyhow::Result<Option<Self>> {
        let Some(cfg) = load_config() else { return Ok(None) };

        let client = Client::builder().homeserver_url(&cfg.homeserver).build().await?;
        client
            .matrix_auth()
            .login_username(&cfg.user, &cfg.password)
            .initial_device_display_name("vendiMessage")
            .await?;

        let me = client.user_id().map(|u| u.to_owned());

        // live incoming messages → broadcast. Images are DOWNLOADED to the local
        // media cache here, so the homeserver only has to keep its copy briefly
        // (a short media-retention window) — storage lives on the clients.
        let tx = events.clone();
        client.add_event_handler(move |ev: SyncRoomMessageEvent, room: SdkRoom, client: Client| {
            let tx = tx.clone();
            let me = me.clone();
            async move {
                let Some(orig) = ev.as_original() else { return };
                let id = orig.event_id.to_string();
                let sender = orig.sender.to_string();
                let mine = me.as_deref() == Some(orig.sender.as_ref());
                let msg = match &orig.content.msgtype {
                    MessageType::Text(t) => {
                        Some(Message::text(id, sender, t.body.clone(), mine, String::new()))
                    }
                    MessageType::Image(img) => {
                        let req = MediaRequest { source: img.source.clone(), format: MediaFormat::File };
                        match client.media().get_media_content(&req, true).await {
                            Ok(bytes) => {
                                let dest = media_cache_dir().join(format!("{id}.bin"));
                                let _ = std::fs::write(&dest, bytes);
                                Some(Message::image(id, sender, mine, String::new(),
                                                    dest.to_string_lossy().to_string()))
                            }
                            Err(_) => None,
                        }
                    }
                    _ => None,
                };
                if let Some(m) = msg {
                    let _ = tx.send(Outgoing::Message { room: room.room_id().to_string(), message: m });
                }
            }
        });

        // initial + continuous sync
        client.sync_once(SyncSettings::default()).await?;
        let bg = client.clone();
        let st = events.clone();
        tokio::spawn(async move {
            let _ = st.send(Outgoing::Status { state: "ready".into() });
            let _ = bg.sync(SyncSettings::default()).await;
        });

        Ok(Some(Self { client, events }))
    }

    pub async fn list_rooms(&self) -> Vec<Room> {
        let mut out = Vec::new();
        for r in self.client.rooms() {
            let name = r
                .display_name()
                .await
                .map(|d| d.to_string())
                .unwrap_or_else(|_| r.room_id().to_string());
            let unread = r.unread_notification_counts().notification_count as u32;
            out.push(Room {
                id: r.room_id().to_string(),
                name,
                preview: String::new(),
                unread,
                color: color_for(r.room_id().as_str()),
            });
        }
        out
    }

    pub async fn timeline(&self, _room: &str, _limit: Option<u32>) -> Vec<Message> {
        // TODO: scrollback via matrix-sdk-ui Timeline. Live messages arrive via
        // the sync event handler in the meantime.
        Vec::new()
    }

    pub async fn send(&self, room: &str, body: &str) -> anyhow::Result<()> {
        let rid = RoomId::parse(room)?;
        let Some(room) = self.client.get_room(&rid) else {
            anyhow::bail!("unknown room {room}");
        };
        room.send(RoomMessageEventContent::text_plain(body)).await?;
        Ok(())
    }

    pub async fn send_image(&self, room: &str, path: &str) -> anyhow::Result<()> {
        let rid = RoomId::parse(room)?;
        let Some(room) = self.client.get_room(&rid) else {
            anyhow::bail!("unknown room {room}");
        };
        let data = std::fs::read(path)?;
        let p = std::path::Path::new(path);
        let filename = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "image".into());
        let mime = mime_for(p);
        room.send_attachment(&filename, &mime, data, AttachmentConfig::new()).await?;
        Ok(())
    }

    pub async fn mark_read(&self, _room: &str) -> anyhow::Result<()> {
        // TODO: send a read receipt for the latest event
        Ok(())
    }
}

fn load_config() -> Option<Config> {
    let path = dirs::config_dir()?.join("vendi/chat.conf");
    let text = std::fs::read_to_string(path).ok()?;
    let mut homeserver = String::new();
    let mut user = String::new();
    let mut password = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let (k, v) = (k.trim(), v.trim());
            match k {
                "homeserver" => homeserver = v.to_string(),
                "user" => user = v.to_string(),
                "password" => password = v.to_string(),
                _ => {}
            }
        }
    }
    if homeserver.is_empty() || user.is_empty() {
        return None;
    }
    Some(Config { homeserver, user, password })
}

fn mime_for(p: &std::path::Path) -> mime::Mime {
    match p.extension().and_then(|e| e.to_str()).map(|s| s.to_ascii_lowercase()).as_deref() {
        Some("png") => mime::IMAGE_PNG,
        Some("gif") => mime::IMAGE_GIF,
        Some("webp") => "image/webp".parse().unwrap(),
        Some("bmp") => "image/bmp".parse().unwrap(),
        _ => mime::IMAGE_JPEG,
    }
}

/// Deterministic avatar colour from the room id (so it's stable across runs).
fn color_for(seed: &str) -> String {
    let h: u32 = seed.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    const PALETTE: [&str; 6] = ["#5b7cfa", "#f0883e", "#bc6bd9", "#56b36a", "#d9534f", "#e0b341"];
    PALETTE[(h as usize) % PALETTE.len()].to_string()
}
