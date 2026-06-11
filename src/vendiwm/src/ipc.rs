// vendi IPC — Unix socket + newline-delimited JSON, sway-style.
//
// Path: $XDG_RUNTIME_DIR/<wayland-socket-name>.ipc.sock
// Env:  VENDIWM_SOCK (set when the compositor starts) so clients can find it.
//
// Protocol:
//   Client → server: one JSON request per line ({"cmd": "...", ...})
//   Server → client: one JSON response per line ({"ok": true, ...} or {"error": "..."})
//   After {"cmd": "subscribe"}, the server pushes one JSON event per line
//   on the same connection. The connection stays open until the client drops.
//
// Implemented as a non-blocking poller called once per frame from the backend.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::layout::Direction;
use crate::state::State;

// ── protocol types ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
pub enum Request {
    /// Launch a process. argv joined by spaces is also accepted via shell.
    Spawn         { args: Vec<String> },
    /// Focus a window by ID.
    Focus         { window: u32 },
    /// Close a window. Omit `window` to close the focused one.
    Close         { window: Option<u32> },
    /// Snapshot of all windows in the layout tree.
    ListWindows,
    /// Snapshot of all workspaces.
    ListWorkspaces,
    /// All active keybinds (defaults merged with user overrides).
    ListBinds,
    /// Switch to a workspace (created on demand).
    Workspace      { id: u32 },
    /// Set the direction of the next split.
    Split         { dir: SplitDir },
    /// Move window to a workspace. (Stubbed.)
    Move          { window: u32, to_workspace: u32 },
    /// Subscribe to event push. Connection stays open after this.
    Subscribe     { events: Vec<EventKind> },
    /// Capture the next composed frame to a PNG (default /tmp/vendiwm-shot.png).
    Screenshot    { path: Option<String> },
    /// Lock the session (vendi-lock, the compositor-native lock screen).
    Lock,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitDir { Horizontal, Vertical }
impl From<SplitDir> for Direction {
    fn from(d: SplitDir) -> Direction {
        match d { SplitDir::Horizontal => Direction::Horizontal, SplitDir::Vertical => Direction::Vertical }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum EventKind { Window, Workspace }

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Response {
    Ok        { ok: bool },
    Error     { error: String },
    Windows   { windows: Vec<WindowInfo> },
    Workspaces{ workspaces: Vec<WorkspaceInfo> },
    Binds     { binds: Vec<BindInfo> },
}

#[derive(Debug, Serialize)]
pub struct BindInfo {
    pub chord:  String,
    pub action: String,
}

#[derive(Debug, Serialize)]
pub struct WindowInfo {
    pub id:        u32,
    pub title:     String,
    pub focused:   bool,
    pub workspace: u32,
    pub floating:  bool,
}

#[derive(Debug, Serialize, Clone)]
pub struct WorkspaceInfo {
    pub id:      u32,
    pub focused: bool,
    /// Number of windows living on this workspace.
    pub windows: usize,
}

#[derive(Debug, Serialize, Clone)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Event {
    WindowOpened      { id: u32, title: String },
    WindowClosed      { id: u32 },
    /// id 0 = nothing focused.
    WindowFocused     { id: u32, title: String },
    WindowTitle       { id: u32, title: String, focused: bool },
    WorkspacesChanged { active: u32, workspaces: Vec<WorkspaceInfo> },
}

// ── server ────────────────────────────────────────────────────────────────────

struct ClientConn {
    stream: UnixStream,
    buf:    Vec<u8>,
    subs:   Vec<EventKind>,
}

pub struct Server {
    listener: UnixListener,
    clients:  Vec<ClientConn>,
    path:     PathBuf,
    /// Queued events to push to subscribed clients on next poll.
    outbox:   Vec<Event>,
}

impl Server {
    /// Bind a fresh IPC socket adjacent to the wayland socket and export
    /// $VENDIWM_SOCK so clients can find it.
    pub fn bind(wayland_socket_name: &str) -> Result<Self> {
        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
            .ok_or_else(|| anyhow::anyhow!("XDG_RUNTIME_DIR not set"))?;
        let path = PathBuf::from(runtime_dir)
            .join(format!("{wayland_socket_name}.ipc.sock"));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).context("bind ipc socket")?;
        listener.set_nonblocking(true).context("set nonblocking")?;
        // SAFETY: single-threaded env mutation at startup — no other threads yet.
        unsafe { std::env::set_var("VENDIWM_SOCK", &path); }
        tracing::info!(path = %path.display(), "IPC listening");
        Ok(Self { listener, clients: Vec::new(), path, outbox: Vec::new() })
    }

    /// Enqueue an event for push delivery on next poll.
    pub fn emit(&mut self, event: Event) {
        self.outbox.push(event);
    }

    /// Non-blocking pump — call from the main loop once per frame.
    pub fn poll(&mut self, state: &mut State) {
        // 1. Accept new connections.
        loop {
            match self.listener.accept() {
                Ok((stream, _addr)) => {
                    let _ = stream.set_nonblocking(true);
                    self.clients.push(ClientConn { stream, buf: Vec::new(), subs: Vec::new() });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => { tracing::warn!(?e, "ipc accept failed"); break; }
            }
        }

        // 2. Read requests from each client. Disconnected clients are pruned.
        let mut idx = 0;
        while idx < self.clients.len() {
            let mut drop_client = false;
            let mut chunk = [0u8; 1024];
            loop {
                match self.clients[idx].stream.read(&mut chunk) {
                    Ok(0)  => { drop_client = true; break; }
                    Ok(n)  => self.clients[idx].buf.extend_from_slice(&chunk[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(_) => { drop_client = true; break; }
                }
            }
            // Drain complete newline-delimited messages.
            while let Some(pos) = self.clients[idx].buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.clients[idx].buf.drain(..=pos).collect();
                let line = &line[..line.len() - 1];   // strip newline
                let response = handle_line(idx, line, &mut self.clients, state);
                if let Some(json) = response {
                    let mut bytes = json.into_bytes();
                    bytes.push(b'\n');
                    if self.clients[idx].stream.write_all(&bytes).is_err() {
                        drop_client = true;
                        break;
                    }
                }
            }
            if drop_client { self.clients.remove(idx); } else { idx += 1; }
        }

        // 3. Push queued events to subscribed clients.
        if !self.outbox.is_empty() {
            let events: Vec<Event> = self.outbox.drain(..).collect();
            let mut idx = 0;
            while idx < self.clients.len() {
                let subs = self.clients[idx].subs.clone();
                let mut dropped = false;
                for ev in &events {
                    let kind = match ev {
                        Event::WindowOpened { .. }
                        | Event::WindowClosed { .. }
                        | Event::WindowFocused { .. }
                        | Event::WindowTitle { .. } => EventKind::Window,
                        Event::WorkspacesChanged { .. } => EventKind::Workspace,
                    };
                    if !subs.contains(&kind) { continue; }
                    let mut bytes = serde_json::to_vec(ev).unwrap_or_default();
                    bytes.push(b'\n');
                    if self.clients[idx].stream.write_all(&bytes).is_err() {
                        dropped = true;
                        break;
                    }
                }
                if dropped { self.clients.remove(idx); } else { idx += 1; }
            }
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

// ── request dispatch ──────────────────────────────────────────────────────────

fn handle_line(client_idx: usize, line: &[u8], clients: &mut [ClientConn], state: &mut State) -> Option<String> {
    let req: Request = match serde_json::from_slice(line) {
        Ok(r)  => r,
        Err(e) => return Some(serde_json::to_string(&Response::Error { error: format!("parse: {e}") }).unwrap()),
    };

    let resp = match req {
        Request::Spawn { args } => {
            let cmd = args.join(" ");
            tracing::info!(%cmd, "ipc spawn");
            match std::process::Command::new("sh").arg("-c").arg(&cmd).spawn() {
                Ok(_)  => Response::Ok { ok: true },
                Err(e) => Response::Error { error: e.to_string() },
            }
        }
        Request::Focus { window } => {
            if state.focus_window_by_id(window) {
                Response::Ok { ok: true }
            } else {
                Response::Error { error: format!("no window with id {window}") }
            }
        }
        Request::Close { window } => {
            let target = match window {
                None => state.focused_window(),
                Some(id) => state.workspaces.all_windows().into_iter()
                    .find(|w| crate::state::window_id(w) == id),
            };
            match target.and_then(|w| w.toplevel().cloned()) {
                Some(t) => { t.send_close(); Response::Ok { ok: true } }
                None    => Response::Error { error: "no such window".into() },
            }
        }
        Request::ListWindows => {
            use smithay::wayland::compositor::with_states;
            use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
            let mut out = Vec::new();
            let focused_win = state.focused_window();
            for w in state.workspaces.all_windows() {
                let id = crate::state::window_id(&w);
                let title = if let Some(t) = w.toplevel() {
                    with_states(t.wl_surface(), |states| {
                        states.data_map.get::<XdgToplevelSurfaceData>()
                            .and_then(|d| d.lock().ok().and_then(|a| a.title.clone()))
                            .unwrap_or_default()
                    })
                } else { String::new() };
                let workspace = state.workspaces.find_workspace(&w).unwrap_or(0);
                let floating  = state.workspaces.find_workspace(&w)
                    .map(|_| !state.workspaces.active_ref().tree.contains(&w)
                          && state.workspaces.active_ref().floating.iter().any(|(fw, _)| fw == &w))
                    .unwrap_or(false);
                let focused = focused_win.as_ref() == Some(&w);
                out.push(WindowInfo { id, title, focused, workspace, floating });
            }
            Response::Windows { windows: out }
        }
        Request::ListWorkspaces => {
            let (active, list) = state.workspaces.snapshot();
            Response::Workspaces {
                workspaces: list.into_iter()
                    .map(|(id, windows)| WorkspaceInfo { id, focused: id == active, windows })
                    .collect(),
            }
        }
        Request::ListBinds => {
            Response::Binds {
                binds: state.config.keybinds_pretty.iter()
                    .map(|(chord, action)| BindInfo { chord: chord.clone(), action: action.clone() })
                    .collect(),
            }
        }
        Request::Workspace { id } => {
            state.switch_workspace(id);
            Response::Ok { ok: true }
        }
        Request::Split { dir } => {
            state.workspaces.active().tree.next_split_override = Some(dir.into());
            Response::Ok { ok: true }
        }
        Request::Move { window: _, to_workspace } => {
            state.move_focused_to_workspace(to_workspace);
            Response::Ok { ok: true }
        }
        Request::Subscribe { events } => {
            clients[client_idx].subs = events;
            Response::Ok { ok: true }
        }
        Request::Screenshot { path } => {
            state.screenshot = Some(path.unwrap_or_else(|| "/tmp/vendiwm-shot.png".into()));
            state.pending_redraw = true;
            Response::Ok { ok: true }
        }
        Request::Lock => {
            state.lock_session();
            Response::Ok { ok: true }
        }
    };

    Some(serde_json::to_string(&resp).unwrap_or_else(|e| format!(r#"{{"error":"serialize: {e}"}}"#)))
}
