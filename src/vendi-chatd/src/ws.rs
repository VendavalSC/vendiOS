//! WebSocket transport — the QML app connects here (QtWebSockets can't open a
//! Unix socket). Same JSON protocol as the Unix socket: the client sends Cmd
//! objects as text frames; the daemon replies + pushes events as text frames.

use crate::backend::Backend;
use crate::protocol::{Cmd, Outgoing};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

pub async fn serve(backend: Backend, addr: &str) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    eprintln!("vendi-chatd: websocket on ws://{addr}");
    loop {
        let (stream, _) = listener.accept().await?;
        let b = backend.clone();
        tokio::spawn(async move {
            match tokio_tungstenite::accept_async(stream).await {
                Ok(ws) => { let _ = handle(ws, b).await; }
                Err(e) => eprintln!("vendi-chatd: ws handshake failed: {e}"),
            }
        });
    }
}

async fn handle<S>(ws: tokio_tungstenite::WebSocketStream<S>, backend: Backend) -> anyhow::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (mut tx, mut rx) = ws.split();
    let mut events = backend.subscribe();

    tx.send(Message::Text(Outgoing::Status { state: backend.status().await.into() }.to_line())).await?;

    loop {
        tokio::select! {
            ev = events.recv() => {
                if let Ok(out) = ev {
                    if tx.send(Message::Text(out.to_line())).await.is_err() { break; }
                }
            }
            msg = rx.next() => {
                let Some(msg) = msg else { break };
                let msg = match msg { Ok(m) => m, Err(_) => break };
                match msg {
                    Message::Text(t) => {
                        for out in handle_cmd(&backend, &t).await {
                            if tx.send(Message::Text(out.to_line())).await.is_err() { return Ok(()); }
                        }
                    }
                    Message::Ping(p) => { let _ = tx.send(Message::Pong(p)).await; }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

/// Run one command, returning the direct responses (pushed events arrive via the
/// broadcast subscription).
async fn handle_cmd(backend: &Backend, line: &str) -> Vec<Outgoing> {
    let cmd: Cmd = match serde_json::from_str(line) {
        Ok(c) => c,
        Err(e) => return vec![Outgoing::Error { message: format!("bad command: {e}") }],
    };
    match cmd {
        Cmd::Register { user, password } => match backend.login(&user, &password, true).await {
            Ok(()) => vec![],
            Err(e) => vec![Outgoing::Error { message: e.to_string() }],
        },
        Cmd::Login { user, password } => match backend.login(&user, &password, false).await {
            Ok(()) => vec![],
            Err(e) => vec![Outgoing::Error { message: e.to_string() }],
        },
        Cmd::Logout => {
            let _ = backend.logout().await;
            vec![]
        }
        Cmd::SearchUsers { query } => {
            vec![Outgoing::SearchResults { users: backend.search_users(&query).await }]
        }
        Cmd::Block { user } => match backend.block(&user).await {
            Ok(()) => vec![],
            Err(e) => vec![Outgoing::Error { message: e.to_string() }],
        },
        Cmd::Unblock { user } => match backend.unblock(&user).await {
            Ok(()) => vec![],
            Err(e) => vec![Outgoing::Error { message: e.to_string() }],
        },
        Cmd::StartChat { user } => match backend.start_chat(&user).await {
            Ok(()) => vec![],
            Err(e) => vec![Outgoing::Error { message: e.to_string() }],
        },
        Cmd::AcceptInvite { room } => match backend.accept_invite(&room).await {
            Ok(()) => vec![],
            Err(e) => vec![Outgoing::Error { message: e.to_string() }],
        },
        Cmd::RejectInvite { room } => match backend.reject_invite(&room).await {
            Ok(()) => vec![],
            Err(e) => vec![Outgoing::Error { message: e.to_string() }],
        },
        Cmd::ListRooms => vec![Outgoing::Rooms { rooms: backend.list_rooms().await }],
        Cmd::Timeline { room, limit } => {
            let messages = backend.timeline(&room, limit).await;
            vec![Outgoing::Timeline { room, messages }]
        }
        Cmd::Send { room, body, reply_to } => {
            match backend.send(&room, &body, reply_to.as_deref()).await {
                Ok(()) => vec![],
                Err(e) => vec![Outgoing::Error { message: e.to_string() }],
            }
        }
        Cmd::SendImage { room, path } => match backend.send_image(&room, &path).await {
            Ok(()) => vec![],
            Err(e) => vec![Outgoing::Error { message: e.to_string() }],
        },
        Cmd::React { room, event_id, key } => match backend.react(&room, &event_id, &key).await {
            Ok(()) => vec![],
            Err(e) => vec![Outgoing::Error { message: e.to_string() }],
        },
        Cmd::MarkRead { room } => { let _ = backend.mark_read(&room).await; vec![] }
    }
}
