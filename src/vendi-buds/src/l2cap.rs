//! Raw Bluetooth L2CAP sockets (`AF_BLUETOOTH` / `BTPROTO_L2CAP`).
//!
//! Not exposed by the generic `libc` crate (these are BlueZ/Linux-specific,
//! from `<bluetooth/bluetooth.h>` + `<bluetooth/l2cap.h>`), so the constants
//! and the `sockaddr_l2` layout are reproduced here to match the kernel ABI.

use anyhow::{bail, Context, Result};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

const AF_BLUETOOTH: libc::c_int = 31;
const BTPROTO_L2CAP: libc::c_int = 0;
/// Classic BR/EDR address type — AirPods answer AAP over classic Bluetooth
/// (this is what macOS's PacketLogger captures it as), not BLE.
const BDADDR_BREDR: u8 = 0x00;

/// A 6-byte Bluetooth device address, in the byte order the kernel expects —
/// which is the REVERSE of the human-readable `AA:BB:CC:DD:EE:FF` form. This
/// is a well-known BlueZ gotcha: get it backwards and connect() just times
/// out with no useful error.
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct BdAddr([u8; 6]);

impl BdAddr {
    fn parse(s: &str) -> Result<Self> {
        let mut bytes = [0u8; 6];
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 6 {
            bail!("not a bluetooth address: {s}");
        }
        for (i, p) in parts.iter().enumerate() {
            bytes[5 - i] = u8::from_str_radix(p, 16).with_context(|| format!("bad octet in {s}"))?;
        }
        Ok(BdAddr(bytes))
    }
}

#[repr(C, packed)]
struct SockAddrL2 {
    l2_family: libc::sa_family_t,
    l2_psm: u16,      // little-endian on the wire; x86_64 is already LE
    l2_bdaddr: BdAddr,
    l2_cid: u16,
    l2_bdaddr_type: u8,
}

/// A connected L2CAP socket to a remote device's fixed PSM.
pub struct L2capSocket {
    fd: OwnedFd,
}

impl L2capSocket {
    /// Connect to `addr` (e.g. "AA:BB:CC:DD:EE:FF") on the given PSM.
    /// Blocks until connected or the kernel gives up (BlueZ's own supervision
    /// timeout applies — typically a few seconds if the device is out of range
    /// or not actually offering that PSM).
    pub fn connect(addr: &str, psm: u16) -> Result<Self> {
        let bdaddr = BdAddr::parse(addr)?;
        // SAFETY: standard libc socket() call, no aliasing/lifetime concerns.
        let raw = unsafe { libc::socket(AF_BLUETOOTH, libc::SOCK_SEQPACKET, BTPROTO_L2CAP) };
        if raw < 0 {
            return Err(io::Error::last_os_error()).context("socket(AF_BLUETOOTH, SEQPACKET, L2CAP)");
        }
        // SAFETY: raw is a just-created, valid, owned fd.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };

        let sa = SockAddrL2 {
            l2_family: AF_BLUETOOTH as libc::sa_family_t,
            l2_psm: psm.to_le(),
            l2_bdaddr: bdaddr,
            l2_cid: 0,
            l2_bdaddr_type: BDADDR_BREDR,
        };
        // SAFETY: sa is a validly-initialized sockaddr_l2, sized correctly,
        // and outlives this call (it's a local on the stack, not moved).
        let ret = unsafe {
            libc::connect(
                fd.as_raw_fd(),
                &sa as *const SockAddrL2 as *const libc::sockaddr,
                std::mem::size_of::<SockAddrL2>() as libc::socklen_t,
            )
        };
        if ret < 0 {
            return Err(io::Error::last_os_error()).with_context(|| format!("L2CAP connect to {addr} psm={psm:#x}"));
        }
        Ok(Self { fd })
    }

    /// Duplicate the underlying fd so a reader thread and a command-writer
    /// can each own a handle to the same connection — concurrent read()
    /// (one thread) + write() (another) on the same socket is safe without
    /// extra locking; it's only concurrent *writes* that would need it, and
    /// only the main thread ever writes.
    pub fn try_clone(&self) -> Result<Self> {
        let raw = unsafe { libc::dup(self.fd.as_raw_fd()) };
        if raw < 0 {
            return Err(io::Error::last_os_error()).context("dup(l2cap fd)");
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        Ok(Self { fd })
    }
}

impl io::Read for L2capSocket {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr() as *mut _, buf.len()) };
        if n < 0 { Err(io::Error::last_os_error()) } else { Ok(n as usize) }
    }
}

impl io::Write for L2capSocket {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = unsafe { libc::write(self.fd.as_raw_fd(), buf.as_ptr() as *const _, buf.len()) };
        if n < 0 { Err(io::Error::last_os_error()) } else { Ok(n as usize) }
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}
