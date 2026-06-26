//! vendi-chatd — the vendiMessage backend daemon.
//!
//! Owns the chat session (mock now; matrix-sdk with `--features matrix`) and
//! exposes it over a local Unix socket using a line-delimited JSON protocol
//! (see protocol.rs). The QML app and the quickshell notch quick-reply both
//! connect to the same socket: they send commands and receive responses plus
//! pushed message events. Long-running (a systemd user service) so messages
//! arrive and the notch can reply while the app is closed.

mod backend;
#[cfg(feature = "matrix")]
mod matrix;
mod protocol;
mod ws;

const WS_ADDR: &str = "127.0.0.1:8765";

use backend::Backend;
use protocol::{Cmd, Outgoing};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let backend = Backend::new().await?;

    let path = socket_path();
    let _ = std::fs::remove_file(&path); // clear a stale socket
    let listener = UnixListener::bind(&path)?;
    eprintln!("vendi-chatd: listening on {}", path.display());

    // websocket transport for the QML client
    {
        let b = backend.clone();
        tokio::spawn(async move {
            if let Err(e) = ws::serve(b, WS_ADDR).await {
                eprintln!("vendi-chatd: websocket server error: {e}");
            }
        });
    }

    // tidy the socket on Ctrl-C / SIGTERM
    {
        let p = path.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            let _ = std::fs::remove_file(&p);
            std::process::exit(0);
        });
    }

    loop {
        let (stream, _) = listener.accept().await?;
        let b = backend.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, b).await {
                eprintln!("vendi-chatd: client ended: {e}");
            }
        });
    }
}

async fn handle_client(stream: UnixStream, backend: Backend) -> anyhow::Result<()> {
    let (read, write) = stream.into_split();
    let write = Arc::new(Mutex::new(write));

    // forward pushed events (incoming messages, status) to this client
    {
        let write = write.clone();
        let mut rx = backend.subscribe();
        tokio::spawn(async move {
            while let Ok(ev) = rx.recv().await {
                let mut w = write.lock().await;
                if w.write_all(ev.to_line().as_bytes()).await.is_err() {
                    break;
                }
            }
        });
    }

    // greet with the current auth state
    send(&write, &Outgoing::Status { state: backend.status().await.into() }).await;

    // command loop
    let mut lines = BufReader::new(read).lines();
    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let cmd: Cmd = match serde_json::from_str(line) {
            Ok(c) => c,
            Err(e) => {
                send(&write, &Outgoing::Error { message: format!("bad command: {e}") }).await;
                continue;
            }
        };
        match cmd {
            Cmd::Register { user, password } => {
                if let Err(e) = backend.login(&user, &password, true).await {
                    send(&write, &Outgoing::Error { message: e.to_string() }).await;
                }
            }
            Cmd::Login { user, password } => {
                if let Err(e) = backend.login(&user, &password, false).await {
                    send(&write, &Outgoing::Error { message: e.to_string() }).await;
                }
            }
            Cmd::Logout => {
                let _ = backend.logout().await;
            }
            Cmd::SearchUsers { query } => {
                let users = backend.search_users(&query).await;
                send(&write, &Outgoing::SearchResults { users }).await;
            }
            Cmd::Block { user } => {
                if let Err(e) = backend.block(&user).await {
                    send(&write, &Outgoing::Error { message: e.to_string() }).await;
                }
            }
            Cmd::Unblock { user } => {
                if let Err(e) = backend.unblock(&user).await {
                    send(&write, &Outgoing::Error { message: e.to_string() }).await;
                }
            }
            Cmd::StartChat { user } => {
                if let Err(e) = backend.start_chat(&user).await {
                    send(&write, &Outgoing::Error { message: e.to_string() }).await;
                }
            }
            Cmd::AcceptInvite { room } => {
                if let Err(e) = backend.accept_invite(&room).await {
                    send(&write, &Outgoing::Error { message: e.to_string() }).await;
                }
            }
            Cmd::RejectInvite { room } => {
                if let Err(e) = backend.reject_invite(&room).await {
                    send(&write, &Outgoing::Error { message: e.to_string() }).await;
                }
            }
            Cmd::ListRooms => {
                let rooms = backend.list_rooms().await;
                send(&write, &Outgoing::Rooms { rooms }).await;
            }
            Cmd::Timeline { room, limit } => {
                let messages = backend.timeline(&room, limit).await;
                send(&write, &Outgoing::Timeline { room, messages }).await;
            }
            Cmd::Send { room, body, reply_to } => {
                if let Err(e) = backend.send(&room, &body, reply_to.as_deref()).await {
                    send(&write, &Outgoing::Error { message: e.to_string() }).await;
                }
            }
            Cmd::SendImage { room, path } => {
                if let Err(e) = backend.send_image(&room, &path).await {
                    send(&write, &Outgoing::Error { message: e.to_string() }).await;
                }
            }
            Cmd::React { room, event_id, key } => {
                if let Err(e) = backend.react(&room, &event_id, &key).await {
                    send(&write, &Outgoing::Error { message: e.to_string() }).await;
                }
            }
            Cmd::MarkRead { room } => {
                let _ = backend.mark_read(&room).await;
            }
        }
    }
    Ok(())
}

async fn send(write: &Arc<Mutex<tokio::net::unix::OwnedWriteHalf>>, msg: &Outgoing) {
    let mut w = write.lock().await;
    let _ = w.write_all(msg.to_line().as_bytes()).await;
}

/// $XDG_RUNTIME_DIR/vendi-chat.sock, else /tmp/vendi-chat-<uid>.sock.
fn socket_path() -> std::path::PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        return std::path::Path::new(&dir).join("vendi-chat.sock");
    }
    let uid = unsafe { libc_getuid() };
    std::path::PathBuf::from(format!("/tmp/vendi-chat-{uid}.sock"))
}

// tiny getuid without pulling the libc crate
unsafe extern "C" {
    #[link_name = "getuid"]
    fn libc_getuid() -> u32;
}
