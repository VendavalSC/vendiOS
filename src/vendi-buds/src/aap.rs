//! Apple Accessory Protocol (AAP/AACP) — the protocol AirPods speak over an
//! L2CAP channel on PSM 0x1001. Packet formats are from reverse-engineering
//! notes (byte-level opcode/packet documentation only, no code lifted from
//! any existing implementation): handshake, noise-control mode, and battery
//! status. Deliberately NOT implementing the rest of the protocol (hearing
//! aid, conversational awareness, stem config, EQ, ...) — vendi-buds stays
//! intentionally small.

pub const AAP_PSM: u16 = 0x1001;

/// Sent once right after the L2CAP connection opens. The AirPods stay silent
/// until they see this exact packet.
pub const HANDSHAKE: &[u8] = &[
    0x00, 0x00, 0x04, 0x00, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// Subscribes to notifications (battery, noise mode, ear detection, ...).
/// Without this the AirPods never volunteer any status. Uses the all-bits-set
/// variant (FF FF FF FF, vs. the FE-in-byte-3 variant some captures show) —
/// the differing bit plausibly gates a notification category (battery is the
/// prime suspect after the first real-hardware test never got one).
pub const REQUEST_NOTIFICATIONS: &[u8] = &[0x04, 0x00, 0x04, 0x00, 0x0F, 0x00, 0xFF, 0xFF, 0xFF, 0xFF];

/// AirPods Pro 2 gate Adaptive Transparency (and always-on conversational
/// awareness) behind this packet — without it, asking for Adaptive mode gets
/// silently downgraded to something else instead (observed: reported back as
/// Off on real hardware). Harmless to send on models that don't have the
/// feature; only Pro 2 is documented, but there's nothing model-specific
/// about sending it early in every session.
pub const ENABLE_ADAPTIVE_FEATURES: &[u8] =
    &[0x04, 0x00, 0x04, 0x00, 0x4d, 0x00, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];

const HEADER: [u8; 4] = [0x04, 0x00, 0x04, 0x00];
const OP_BATTERY: u16 = 0x0004;
const OP_CONTROL: u16 = 0x0009;
const CTRL_LISTENING_MODE: u8 = 0x0D;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoiseMode {
    Off,
    Anc,
    Transparency,
    Adaptive,
}

impl NoiseMode {
    fn from_byte(b: u8) -> Option<Self> {
        match b {
            0x01 => Some(Self::Off),
            0x02 => Some(Self::Anc),
            0x03 => Some(Self::Transparency),
            0x04 => Some(Self::Adaptive),
            _ => None,
        }
    }
    fn to_byte(self) -> u8 {
        match self {
            Self::Off => 0x01,
            Self::Anc => 0x02,
            Self::Transparency => 0x03,
            Self::Adaptive => 0x04,
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "off" => Some(Self::Off),
            "anc" => Some(Self::Anc),
            "transparency" => Some(Self::Transparency),
            "adaptive" => Some(Self::Adaptive),
            _ => None,
        }
    }
}

/// Build the packet that asks the AirPods to switch noise mode. They answer
/// with the same shape of packet once the mode has actually changed (or with
/// a different mode than asked for, on APs that don't support Adaptive).
pub fn set_noise_mode(mode: NoiseMode) -> Vec<u8> {
    let mut p = HEADER.to_vec();
    p.extend_from_slice(&OP_CONTROL.to_le_bytes());
    p.push(CTRL_LISTENING_MODE);
    p.push(mode.to_byte());
    p.extend_from_slice(&[0x00, 0x00, 0x00]);
    p
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct ComponentBattery {
    pub level: u8,     // percent, 0-100
    pub charging: bool,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Battery {
    pub left:  Option<ComponentBattery>,
    pub right: Option<ComponentBattery>,
    pub case:  Option<ComponentBattery>,
}

#[derive(Debug, Clone)]
pub enum Event {
    Battery(Battery),
    NoiseMode(NoiseMode),
    /// Recognized header but a body shape we don't parse (fine — we only
    /// care about a couple of opcodes; everything else is ignored upstream).
    Other,
}

/// Parse one AAP packet (already framed — see `PacketReader`) into an event.
pub fn parse(pkt: &[u8]) -> Option<Event> {
    if pkt.len() < 6 || pkt[0..4] != HEADER {
        return None;
    }
    let opcode = u16::from_le_bytes([pkt[4], pkt[5]]);
    match opcode {
        OP_BATTERY => parse_battery(pkt).map(Event::Battery),
        OP_CONTROL if pkt.len() >= 8 && pkt[6] == CTRL_LISTENING_MODE => {
            NoiseMode::from_byte(pkt[7]).map(Event::NoiseMode)
        }
        _ => Some(Event::Other),
    }
}

fn parse_battery(pkt: &[u8]) -> Option<Battery> {
    // 04 00 04 00 04 00 [count] ([component] 01 [level] [status] 01) * count
    let count = *pkt.get(6)? as usize;
    let mut out = Battery::default();
    let mut off = 7usize;
    const STATUS_DISCONNECTED: u8 = 0x04;
    for _ in 0..count {
        let component = *pkt.get(off)?;
        let level = *pkt.get(off + 2)?;
        let status = *pkt.get(off + 3)?;
        // "Disconnected" (e.g. the case, whenever the buds are out of it —
        // it has no live link to report a real reading over) ships level=0
        // as a placeholder, not an actual empty battery. Treat it the same
        // as never having heard from that component at all.
        if status == STATUS_DISCONNECTED {
            off += 5;
            continue;
        }
        let charging = status == 0x01;
        let cb = ComponentBattery { level, charging };
        match component {
            0x08 => out.case = Some(cb),
            0x04 => out.left = Some(cb),
            0x02 => out.right = Some(cb),
            _ => {}
        }
        off += 5;
    }
    Some(out)
}
