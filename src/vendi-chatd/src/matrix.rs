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
    room::MessagesOptions,
    ruma::events::{
        reaction::ReactionEventContent,
        relation::{Annotation, InReplyTo},
        room::message::{MessageType, Relation, RoomMessageEventContent, SyncRoomMessageEvent},
        AnyMessageLikeEvent, AnyTimelineEvent, MessageLikeEvent,
    },
    ruma::{EventId, RoomId},
    Client, Room as SdkRoom,
};
use std::collections::HashMap;
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
                let ts = orig.origin_server_ts.0.to_string();
                let reply_to = match &orig.content.relates_to {
                    Some(Relation::Reply { in_reply_to }) => in_reply_to.event_id.to_string(),
                    _ => String::new(),
                };
                let msg = match &orig.content.msgtype {
                    MessageType::Text(t) => {
                        Some(Message::text(id, sender, t.body.clone(), mine, ts))
                    }
                    MessageType::Image(img) => {
                        let req = MediaRequest { source: img.source.clone(), format: MediaFormat::File };
                        match client.media().get_media_content(&req, true).await {
                            Ok(bytes) => {
                                let dest = media_cache_dir().join(format!("{id}.bin"));
                                let _ = std::fs::write(&dest, bytes);
                                Some(Message::image(id, sender, mine, ts,
                                                    dest.to_string_lossy().to_string()))
                            }
                            Err(_) => None,
                        }
                    }
                    _ => None,
                };
                if let Some(mut m) = msg {
                    if !reply_to.is_empty() {
                        m.reply_to = reply_to;
                        if m.kind == "text" {
                            m.body = strip_reply_fallback(&m.body);
                        }
                    }
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

    /// Scrollback: fetch recent history via the /messages endpoint (backward
    /// pagination). Images are downloaded into the local media cache, so the
    /// server can purge its copies — storage lives on the clients.
    pub async fn timeline(&self, room: &str, limit: Option<u32>) -> Vec<Message> {
        let Ok(rid) = RoomId::parse(room) else { return Vec::new() };
        let Some(room) = self.client.get_room(&rid) else { return Vec::new() };

        let mut opts = MessagesOptions::backward();
        opts.limit = limit.unwrap_or(50).into();
        let resp = match room.messages(opts).await {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };

        // first pass: collect emoji reactions, keyed by their target event id
        let mut reactions: HashMap<String, Vec<String>> = HashMap::new();
        for ev in &resp.chunk {
            if let Ok(AnyTimelineEvent::MessageLike(AnyMessageLikeEvent::Reaction(
                MessageLikeEvent::Original(r),
            ))) = ev.event.deserialize()
            {
                let target = r.content.relates_to.event_id.to_string();
                reactions.entry(target).or_default().push(r.content.relates_to.key);
            }
        }

        let me = self.client.user_id().map(|u| u.to_owned());
        let mut out = Vec::new();
        // backward pagination yields newest-first; reverse for chronological order
        for ev in resp.chunk.into_iter().rev() {
            let Ok(any) = ev.event.deserialize() else { continue };
            let AnyTimelineEvent::MessageLike(AnyMessageLikeEvent::RoomMessage(
                MessageLikeEvent::Original(orig),
            )) = any
            else {
                continue;
            };
            let id = orig.event_id.to_string();
            let sender = orig.sender.to_string();
            let mine = me.as_deref() == Some(orig.sender.as_ref());
            let ts = orig.origin_server_ts.0.to_string();
            let reply_to = match &orig.content.relates_to {
                Some(Relation::Reply { in_reply_to }) => in_reply_to.event_id.to_string(),
                _ => String::new(),
            };
            let mut msg = match orig.content.msgtype {
                MessageType::Text(t) => {
                    let body = if reply_to.is_empty() {
                        t.body
                    } else {
                        strip_reply_fallback(&t.body)
                    };
                    Message::text(id.clone(), sender, body, mine, ts)
                }
                MessageType::Image(img) => {
                    let req = MediaRequest { source: img.source.clone(), format: MediaFormat::File };
                    match self.client.media().get_media_content(&req, true).await {
                        Ok(bytes) => {
                            let dest = media_cache_dir().join(format!("{id}.bin"));
                            let _ = std::fs::write(&dest, &bytes);
                            Message::image(
                                id.clone(),
                                sender,
                                mine,
                                ts,
                                dest.to_string_lossy().to_string(),
                            )
                        }
                        Err(_) => continue,
                    }
                }
                _ => continue,
            };
            msg.reply_to = reply_to;
            if let Some(rx) = reactions.remove(&id) {
                msg.reactions = rx;
            }
            out.push(msg);
        }
        out
    }

    pub async fn send(&self, room: &str, body: &str, reply_to: Option<&str>) -> anyhow::Result<()> {
        let rid = RoomId::parse(room)?;
        let Some(room) = self.client.get_room(&rid) else {
            anyhow::bail!("unknown room {room}");
        };
        let mut content = RoomMessageEventContent::text_plain(body);
        if let Some(r) = reply_to {
            if let Ok(eid) = EventId::parse(r) {
                content.relates_to = Some(Relation::Reply { in_reply_to: InReplyTo::new(eid) });
            }
        }
        room.send(content).await?;
        Ok(())
    }

    pub async fn react(&self, room: &str, event_id: &str, key: &str) -> anyhow::Result<()> {
        let rid = RoomId::parse(room)?;
        let Some(room) = self.client.get_room(&rid) else {
            anyhow::bail!("unknown room {room}");
        };
        let eid = EventId::parse(event_id)?;
        room.send(ReactionEventContent::new(Annotation::new(eid, key.to_string()))).await?;
        Ok(())
    }

    pub async fn send_image(&self, room: &str, path: &str) -> anyhow::Result<()> {
        let rid = RoomId::parse(room)?;
        let Some(room) = self.client.get_room(&rid) else {
            anyhow::bail!("unknown room {room}");
        };
        // Compress hard before upload — the homeserver (Pi) keeps a short-lived
        // copy; clients keep the original. Downscale to <=1600px, JPEG q72.
        let (data, mime, filename) = compress_image(path);
        room.send_attachment(&filename, &mime, data, AttachmentConfig::new()).await?;
        Ok(())
    }

    pub async fn mark_read(&self, room: &str) -> anyhow::Result<()> {
        use matrix_sdk::ruma::api::client::receipt::create_receipt::v3::ReceiptType;
        use matrix_sdk::ruma::events::receipt::ReceiptThread;
        let Ok(rid) = RoomId::parse(room) else { return Ok(()) };
        let Some(room) = self.client.get_room(&rid) else { return Ok(()) };
        let mut opts = MessagesOptions::backward();
        opts.limit = 1u32.into();
        if let Ok(resp) = room.messages(opts).await {
            if let Some(ev) = resp.chunk.first() {
                if let Ok(any) = ev.event.deserialize() {
                    let eid = any.event_id().to_owned();
                    let _ = room
                        .send_single_receipt(ReceiptType::Read, ReceiptThread::Unthreaded, eid)
                        .await;
                }
            }
        }
        Ok(())
    }
}

/// Decode, downscale (longest side <=1600), and re-encode as JPEG q72. Falls back
/// to the raw bytes if the file can't be decoded. Returns (bytes, mime, filename).
fn compress_image(path: &str) -> (Vec<u8>, mime::Mime, String) {
    let p = std::path::Path::new(path);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "image".into());
    match image::open(path) {
        Ok(img) => {
            let img = if img.width() > 1600 || img.height() > 1600 {
                img.resize(1600, 1600, image::imageops::FilterType::Lanczos3)
            } else {
                img
            };
            let rgb = img.to_rgb8();
            let mut buf = std::io::Cursor::new(Vec::new());
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 72);
            if enc.encode_image(&rgb).is_ok() {
                return (buf.into_inner(), mime::IMAGE_JPEG, format!("{stem}.jpg"));
            }
            (std::fs::read(path).unwrap_or_default(), mime_for(p), filename_of(p))
        }
        Err(_) => (std::fs::read(path).unwrap_or_default(), mime_for(p), filename_of(p)),
    }
}

fn filename_of(p: &std::path::Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "image".into())
}

/// Strip the leading rich-reply quote fallback ("> …" lines) from a body.
fn strip_reply_fallback(body: &str) -> String {
    if !body.starts_with("> ") {
        return body.to_string();
    }
    body.lines()
        .skip_while(|l| l.starts_with("> ") || l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
        .trim_start()
        .to_string()
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
