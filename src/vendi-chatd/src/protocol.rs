//! The line-delimited JSON IPC protocol between vendi-chatd and its clients
//! (the QML app, the quickshell notch quick-reply). One JSON object per line.

use serde::{Deserialize, Serialize};

/// A command sent by a client to the daemon.
#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Cmd {
    /// List the conversations.
    ListRooms,
    /// Fetch recent messages for a room.
    Timeline { room: String, #[serde(default)] limit: Option<u32> },
    /// Send a text message to a room.
    Send { room: String, body: String },
    /// Send an image: `path` is a local file the daemon uploads.
    SendImage { room: String, path: String },
    /// Mark a room as read.
    MarkRead { room: String },
}

/// A message the daemon sends back to a client (responses + pushed events).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Outgoing {
    /// Connection / sync status.
    Status { state: String },
    /// Reply to ListRooms.
    Rooms { rooms: Vec<Room> },
    /// Reply to Timeline.
    Timeline { room: String, messages: Vec<Message> },
    /// A newly arrived (or just-sent) message — pushed to every client.
    Message { room: String, message: Message },
    /// An error for a command.
    Error { message: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct Room {
    pub id: String,
    pub name: String,
    pub preview: String,
    pub unread: u32,
    /// stable colour seed for the avatar monogram
    pub color: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Message {
    pub id: String,
    pub sender: String,
    pub body: String,
    pub mine: bool,
    /// human time label, e.g. "9:01 AM"
    pub ts: String,
    /// "text" or "image"
    pub kind: String,
    /// for images: the LOCAL cached file path the client downloaded it to (so the
    /// server can purge the original — storage lives on clients, not the server).
    pub media: String,
}

impl Message {
    pub fn text(id: String, sender: String, body: String, mine: bool, ts: String) -> Self {
        Self { id, sender, body, mine, ts, kind: "text".into(), media: String::new() }
    }
    pub fn image(id: String, sender: String, mine: bool, ts: String, media: String) -> Self {
        Self { id, sender, body: String::new(), mine, ts, kind: "image".into(), media }
    }
}

impl Outgoing {
    pub fn to_line(&self) -> String {
        let mut s = serde_json::to_string(self).unwrap_or_else(|_| "{}".into());
        s.push('\n');
        s
    }
}
