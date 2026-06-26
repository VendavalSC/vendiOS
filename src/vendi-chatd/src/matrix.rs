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

use crate::protocol::{Message, Outgoing, Room, UserHit};
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
        Ok(Some(Self::start(client, events).await?))
    }

    /// Runtime sign-in (optionally creating the account first), persisting creds
    /// so the next launch auto-signs-in.
    pub async fn connect(
        events: broadcast::Sender<Outgoing>,
        user: &str,
        password: &str,
        register: bool,
    ) -> anyhow::Result<Self> {
        let homeserver = homeserver_url();
        let client = Client::builder().homeserver_url(&homeserver).build().await?;
        if register {
            do_register(&client, user, password).await?;
        }
        client
            .matrix_auth()
            .login_username(user, password)
            .initial_device_display_name("vendiMessage")
            .await?;
        save_config(&homeserver, user, password);
        Self::start(client, events).await
    }

    /// Wire the live event handlers and kick off background sync.
    async fn start(client: Client, events: broadcast::Sender<Outgoing>) -> anyhow::Result<Self> {
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

        Ok(Self { client, events })
    }

    pub async fn list_rooms(&self) -> Vec<Room> {
        let mut out = Vec::new();
        for r in self.client.rooms() {
            let state = r.state();
            if matches!(state, matrix_sdk::RoomState::Left) {
                continue; // a declined/left chat shouldn't show
            }
            let invite = matches!(state, matrix_sdk::RoomState::Invited);
            let name = r
                .display_name()
                .await
                .map(|d| d.to_string())
                .unwrap_or_else(|_| r.room_id().to_string());
            let unread = r.unread_notification_counts().notification_count as u32;
            let peer = r.direct_targets().into_iter().next().map(|u| u.to_string()).unwrap_or_default();
            out.push(Room {
                id: r.room_id().to_string(),
                name,
                preview: String::new(),
                unread,
                color: color_for(r.room_id().as_str()),
                invite,
                peer,
            });
        }
        out
    }

    /// Normalize "bob" / "@bob" / "@bob:vendi.chat" → a full "@bob:server" id.
    fn full_user_id(&self, user: &str) -> String {
        if user.starts_with('@') && user.contains(':') {
            return user.to_string();
        }
        let server = self
            .client
            .user_id()
            .map(|u| u.server_name().to_string())
            .unwrap_or_else(|| "vendi.chat".to_string());
        format!("@{}:{}", user.trim_start_matches('@'), server)
    }

    /// Start a new chat (DM) with a user; accepts "@bob:vendi.chat" or "bob".
    pub async fn start_chat(&self, user: &str) -> anyhow::Result<()> {
        let uid = matrix_sdk::ruma::UserId::parse(self.full_user_id(user))?;
        self.client.create_dm(&uid).await?;
        Ok(())
    }

    /// Find users. Tries the homeserver's user directory first; continuwuity
    /// doesn't populate one, so it falls back to resolving "@<query>:server"
    /// directly via a profile lookup (exact-username match — fine for a walled
    /// garden where people know each other's handles).
    pub async fn search_users(&self, query: &str) -> Vec<UserHit> {
        let q = query.trim().trim_start_matches('@');
        if q.is_empty() {
            return Vec::new();
        }

        // 1) directory search (works on servers that index one)
        if let Ok(resp) = self.client.search_users(query, 20).await {
            if !resp.results.is_empty() {
                return resp
                    .results
                    .into_iter()
                    .map(|u| {
                        let id = u.user_id.to_string();
                        let name = u
                            .display_name
                            .unwrap_or_else(|| localpart(&id));
                        UserHit { id, name }
                    })
                    .collect();
            }
        }

        // 2) walled-garden fallback: resolve the exact handle
        let full = self.full_user_id(q);
        if let Ok(uid) = matrix_sdk::ruma::UserId::parse(&full) {
            if let Ok(profile) = self.client.get_profile(&uid).await {
                let name = profile.displayname.unwrap_or_else(|| q.to_string());
                return vec![UserHit { id: full, name }];
            }
        }
        Vec::new()
    }

    /// Block (ignore) a user — their events stop arriving.
    pub async fn block(&self, user: &str) -> anyhow::Result<()> {
        let uid = matrix_sdk::ruma::UserId::parse(self.full_user_id(user))?;
        self.client.account().ignore_user(&uid).await?;
        Ok(())
    }

    /// Unblock a previously blocked user.
    pub async fn unblock(&self, user: &str) -> anyhow::Result<()> {
        let uid = matrix_sdk::ruma::UserId::parse(self.full_user_id(user))?;
        self.client.account().unignore_user(&uid).await?;
        Ok(())
    }

    /// Accept a pending chat request (join the invited room).
    pub async fn accept_invite(&self, room: &str) -> anyhow::Result<()> {
        let rid = RoomId::parse(room)?;
        let Some(room) = self.client.get_room(&rid) else {
            anyhow::bail!("unknown room {room}");
        };
        room.join().await?;
        Ok(())
    }

    /// Decline a pending chat request (leave the invited room).
    pub async fn reject_invite(&self, room: &str) -> anyhow::Result<()> {
        let rid = RoomId::parse(room)?;
        let Some(room) = self.client.get_room(&rid) else {
            anyhow::bail!("unknown room {room}");
        };
        room.leave().await?;
        Ok(())
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

const DEFAULT_HOMESERVER: &str = "https://server.emerald-ayu.ts.net";

fn config_path() -> Option<std::path::PathBuf> {
    Some(dirs::config_dir()?.join("vendi/chat.conf"))
}

fn read_kv() -> HashMap<String, String> {
    let mut m = HashMap::new();
    if let Some(p) = config_path() {
        if let Ok(text) = std::fs::read_to_string(p) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    m.insert(k.trim().to_string(), v.trim().to_string());
                }
            }
        }
    }
    m
}

/// The homeserver the app signs in to (chat.conf override, else the default).
fn homeserver_url() -> String {
    read_kv()
        .get("homeserver")
        .filter(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| DEFAULT_HOMESERVER.to_string())
}

fn load_config() -> Option<Config> {
    let m = read_kv();
    let user = m.get("user").cloned().unwrap_or_default();
    let password = m.get("password").cloned().unwrap_or_default();
    if user.is_empty() || password.is_empty() {
        return None; // not signed in yet
    }
    Some(Config { homeserver: homeserver_url(), user, password })
}

/// Persist the session so the next launch auto-signs-in.
pub fn save_config(homeserver: &str, user: &str, password: &str) {
    if let Some(p) = config_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let body = format!(
            "# vendiMessage session (written by vendi-chatd)\nhomeserver = {homeserver}\nuser = {user}\npassword = {password}\n"
        );
        let _ = std::fs::write(p, body);
    }
}

/// Forget the saved session (keep the homeserver, drop user/password).
pub fn clear_config() {
    let hs = homeserver_url();
    if let Some(p) = config_path() {
        let _ = std::fs::write(p, format!("# vendiMessage session (signed out)\nhomeserver = {hs}\n"));
    }
}

/// Create an account via the UIA dummy flow (open registration).
async fn do_register(client: &Client, user: &str, password: &str) -> anyhow::Result<()> {
    use matrix_sdk::ruma::api::client::account::register;
    use matrix_sdk::ruma::api::client::uiaa::{AuthData, Dummy};
    let mut req = register::v3::Request::new();
    req.username = Some(user.to_string());
    req.password = Some(password.to_string());
    req.inhibit_login = true;
    match client.matrix_auth().register(req.clone()).await {
        Ok(_) => Ok(()),
        Err(e) => {
            // a 401 UIA challenge gives us the session id for the dummy stage
            if let Some(info) = e.as_uiaa_response() {
                let mut dummy = Dummy::new();
                dummy.session = info.session.clone();
                req.auth = Some(AuthData::Dummy(dummy));
                client.matrix_auth().register(req).await?;
                Ok(())
            } else {
                Err(e.into())
            }
        }
    }
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

/// "@bob:vendi.chat" → "bob"
fn localpart(id: &str) -> String {
    id.trim_start_matches('@').split(':').next().unwrap_or(id).to_string()
}

/// Deterministic avatar colour from the room id (so it's stable across runs).
fn color_for(seed: &str) -> String {
    let h: u32 = seed.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    const PALETTE: [&str; 6] = ["#5b7cfa", "#f0883e", "#bc6bd9", "#56b36a", "#d9534f", "#e0b341"];
    PALETTE[(h as usize) % PALETTE.len()].to_string()
}
