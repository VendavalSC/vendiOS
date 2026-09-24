//! vendi-buds — background daemon + CLI for Bluetooth earbuds.
//!
//! AirPods get full support (noise-mode control + per-bud/case battery) over
//! their real AAP protocol on L2CAP PSM 0x1001. Anything else that's
//! connected over Bluetooth still shows up (name, connected state) with a
//! generic icon and no controls — vendiOS doesn't know its protocol, but it
//! shouldn't pretend nothing's there either.
//!
//! Usage:
//!   vendi-buds            run as the background daemon (started by vendi-session)
//!   vendi-buds noise MODE  tell the running daemon to switch noise mode
//!   vendi-buds status      print the current state as JSON

mod aap;
mod control;
mod discovery;
mod l2cap;
mod state;

use anyhow::{bail, Context, Result};
use aap::{Event, NoiseMode};
use state::{Kind, State};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("noise") => {
            let mode = args.get(2).context("usage: vendi-buds noise <off|anc|transparency|adaptive>")?;
            if NoiseMode::parse(mode).is_none() {
                bail!("unknown noise mode: {mode} (want off|anc|transparency|adaptive)");
            }
            send_command(&format!("noise {mode}"))
        }
        Some("status") => print_status(),
        None => run_daemon(),
        Some(other) => bail!("unknown command: {other} (want: noise <mode> | status | [no args] to run as daemon)"),
    }
}

fn send_command(line: &str) -> Result<()> {
    use std::os::unix::net::UnixStream;
    let mut s = UnixStream::connect(control::socket_path()).context("vendi-buds daemon not running")?;
    writeln!(s, "{line}")?;
    Ok(())
}

fn print_status() -> Result<()> {
    match std::fs::read_to_string(State::path()) {
        Ok(s) => { print!("{s}"); Ok(()) }
        Err(_) => {
            // No state file yet (daemon never ran, or nothing's connected) —
            // print the same shape a disconnected daemon would, not an error.
            println!("{}", serde_json::to_string(&State::disconnected())?);
            Ok(())
        }
    }
}

fn run_daemon() -> Result<()> {
    let (cmd_tx, cmd_rx) = mpsc::channel::<control::Command>();
    {
        let tx = cmd_tx.clone();
        std::thread::spawn(move || {
            if let Err(e) = control::serve(tx) {
                tracing::error!(?e, "control socket server exited");
            }
        });
    }

    loop {
        let Some(dev) = discovery::connected_device() else {
            State::disconnected().write().ok();
            std::thread::sleep(Duration::from_secs(3));
            continue;
        };
        tracing::info!(mac = %dev.mac, name = %dev.name, "device connected, starting session");
        if let Err(e) = run_session(&dev, &cmd_rx) {
            tracing::warn!(?e, mac = %dev.mac, "session ended");
        }
        State::disconnected().write().ok();
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// One connected-device session: tries the AAP handshake; on success, drives
/// full AirPods support until disconnect, otherwise falls back to a generic
/// "something's connected" state and just watches for disconnect.
fn run_session(dev: &discovery::ConnectedDevice, cmd_rx: &mpsc::Receiver<control::Command>) -> Result<()> {
    let sock = match l2cap::L2capSocket::connect(&dev.mac, aap::AAP_PSM) {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(?e, "AAP handshake unavailable — treating as generic device");
            return run_generic_session(dev);
        }
    };
    run_airpods_session(dev, sock, cmd_rx)
}

fn run_generic_session(dev: &discovery::ConnectedDevice) -> Result<()> {
    let state = State {
        connected: true,
        name: dev.name.clone(),
        kind: Kind::Generic,
        noise_mode: None,
        battery: Default::default(),
    };
    state.write().ok();
    // No live protocol to watch — just poll bluetoothctl until it disconnects.
    loop {
        std::thread::sleep(Duration::from_secs(5));
        match discovery::connected_device() {
            Some(d) if d.mac == dev.mac => continue,
            _ => return Ok(()),
        }
    }
}

fn run_airpods_session(
    dev: &discovery::ConnectedDevice,
    mut sock: l2cap::L2capSocket,
    cmd_rx: &mpsc::Receiver<control::Command>,
) -> Result<()> {
    sock.write_all(aap::HANDSHAKE)?;
    sock.write_all(aap::REQUEST_NOTIFICATIONS)?;
    sock.write_all(aap::ENABLE_ADAPTIVE_FEATURES)?;

    let state = Arc::new(std::sync::Mutex::new(State {
        connected: true,
        name: dev.name.clone(),
        kind: Kind::Airpods,
        noise_mode: None,
        battery: Default::default(),
    }));
    state.lock().unwrap().write().ok();

    let connected = Arc::new(AtomicBool::new(true));
    let mut writer = sock.try_clone().context("clone l2cap socket for writer")?;

    {
        let state = Arc::clone(&state);
        let connected = Arc::clone(&connected);
        std::thread::spawn(move || {
            let mut buf = [0u8; 1024];
            loop {
                let n = match sock.read(&mut buf) {
                    Ok(0) | Err(_) => break, // 0 = orderly close; err = link dropped
                    Ok(n) => n,
                };
                tracing::debug!(bytes = %hex(&buf[..n]), "rx");
                match aap::parse(&buf[..n]) {
                    Some(Event::Battery(b)) => {
                        let mut st = state.lock().unwrap();
                        st.battery = b;
                        st.write().ok();
                    }
                    Some(Event::NoiseMode(m)) => {
                        let mut st = state.lock().unwrap();
                        st.noise_mode = Some(m);
                        st.write().ok();
                    }
                    other => tracing::debug!(?other, "rx (unparsed as battery/noise)"),
                }
            }
            connected.store(false, Ordering::SeqCst);
        });
    }

    // Main thread: apply commands to the live socket until the reader thread
    // signals the link is gone. recv_timeout doubles as the poll interval for
    // noticing that.
    while connected.load(Ordering::SeqCst) {
        match cmd_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(control::Command::SetNoiseMode(mode)) => {
                let pkt = aap::set_noise_mode(mode);
                tracing::debug!(bytes = %hex(&pkt), "tx noise mode");
                if let Err(e) = writer.write_all(&pkt) {
                    tracing::warn!(?e, "failed to send noise mode — link likely dropped");
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(())
}
