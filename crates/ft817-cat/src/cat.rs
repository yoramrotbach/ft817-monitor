//! FT-817 / FT-817ND / FT-818 CAT protocol: read-only commands and decoders.
//!
//! Sources (see docs/PROTOCOL.md for details):
//!   - Yaesu FT-817 operating manual, CAT section (documented commands 03, E7, F7)
//!   - KA7OEI "CAT interface programming with the FT-817" and "FT-817 memory map"
//!   - Hamlib rigs/yaesu/ft817.c (field-tested decoding, incl. undocumented BB/BD)
//!
//! SAFETY: this program never writes to the radio. Every outgoing frame goes
//! through `frame()`, which only accepts the five read-only opcodes below.
//! Opcode 0xBE (factory reset, destroys calibration) and 0xBC (EEPROM write)
//! can never be produced.

use serde::{Deserialize, Serialize};
use std::fmt;

pub const OP_READ_FREQ_MODE: u8 = 0x03;
pub const OP_READ_RX_STATUS: u8 = 0xE7;
pub const OP_READ_TX_STATUS: u8 = 0xF7;
pub const OP_READ_TX_METER: u8 = 0xBD; // undocumented
pub const OP_READ_EEPROM: u8 = 0xBB; // undocumented

const READ_ONLY_OPCODES: [u8; 5] = [
    OP_READ_FREQ_MODE,
    OP_READ_RX_STATUS,
    OP_READ_TX_STATUS,
    OP_READ_TX_METER,
    OP_READ_EEPROM,
];

/// EEPROM addresses this program reads (KA7OEI FT-817 memory map).
pub mod ee {
    pub const VFO_STATE: u16 = 0x55; // VFO A/B, MEM/VFO, HOME, QMB
    pub const PANEL_1: u16 = 0x57; // AGC, DSP, PBT, NB, LOCK, FST
    pub const PANEL_2: u16 = 0x58; // meter mode, KYR, BK, VLT, VOX
    pub const DISPLAY: u16 = 0x5B; // LCD color (blue/amber)
    pub const DIG_MODE: u16 = 0x65; // DIG sub-mode in top 3 bits
    pub const TX_POWER: u16 = 0x79; // TX power level, scan flags
    pub const ANT_SPLIT: u16 = 0x7A; // antenna front/rear per band, split
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatError {
    OpcodeNotAllowed(u8),
    BadLength { expected: usize, got: usize },
    BadBcd(u8),
}

impl fmt::Display for CatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CatError::OpcodeNotAllowed(op) => write!(f, "opcode 0x{op:02X} is not a read-only command"),
            CatError::BadLength { expected, got } => write!(f, "expected {expected} bytes, got {got}"),
            CatError::BadBcd(b) => write!(f, "invalid BCD byte 0x{b:02X}"),
        }
    }
}

/// Build a 5-byte CAT frame. Rejects anything that is not a read command.
pub fn frame(params: [u8; 4], opcode: u8) -> Result<[u8; 5], CatError> {
    if !READ_ONLY_OPCODES.contains(&opcode) {
        return Err(CatError::OpcodeNotAllowed(opcode));
    }
    Ok([params[0], params[1], params[2], params[3], opcode])
}

pub fn simple(opcode: u8) -> Result<[u8; 5], CatError> {
    frame([0; 4], opcode)
}

pub fn eeprom_read(addr: u16) -> [u8; 5] {
    // Opcode is a constant from the allow-list, so this cannot fail.
    frame([(addr >> 8) as u8, (addr & 0xFF) as u8, 0, 0], OP_READ_EEPROM).unwrap()
}

/// Expected response length for each command (BD returns 1 byte in RX, 2 in TX).
pub fn response_len(opcode: u8) -> usize {
    match opcode {
        OP_READ_FREQ_MODE => 5,
        OP_READ_RX_STATUS | OP_READ_TX_STATUS => 1,
        OP_READ_TX_METER | OP_READ_EEPROM => 2,
        _ => 0,
    }
}

// ---------------------------------------------------------------- freq/mode

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Mode {
    Lsb,
    Usb,
    Cw,
    Cwr,
    Am,
    Wfm,
    Fm,
    Dig,
    Pkt,
    Unknown(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DigMode {
    Rtty,
    PskL,
    PskU,
    UserL,
    UserU,
}

impl DigMode {
    pub fn from_eeprom(b: u8) -> Option<DigMode> {
        match b >> 5 {
            0 => Some(DigMode::Rtty),
            1 => Some(DigMode::PskL),
            2 => Some(DigMode::PskU),
            3 => Some(DigMode::UserL),
            4 => Some(DigMode::UserU),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            DigMode::Rtty => "RTTY",
            DigMode::PskL => "PSK-L",
            DigMode::PskU => "PSK-U",
            DigMode::UserL => "USER-L",
            DigMode::UserU => "USER-U",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreqMode {
    pub hz: u64,
    pub mode: Mode,
    pub narrow: bool,
}

fn bcd(b: u8) -> Result<u64, CatError> {
    let hi = b >> 4;
    let lo = b & 0x0F;
    if hi > 9 || lo > 9 {
        return Err(CatError::BadBcd(b));
    }
    Ok((hi * 10 + lo) as u64)
}

/// Command 03: 4 BCD bytes (10 Hz resolution, MSB first) + mode byte.
pub fn decode_freq_mode(b: &[u8]) -> Result<FreqMode, CatError> {
    if b.len() != 5 {
        return Err(CatError::BadLength { expected: 5, got: b.len() });
    }
    let mut tens_of_hz = 0u64;
    for &x in &b[..4] {
        tens_of_hz = tens_of_hz * 100 + bcd(x)?;
    }
    let m = b[4];
    let mode = match m & 0x7F {
        0x00 => Mode::Lsb,
        0x01 => Mode::Usb,
        0x02 => Mode::Cw,
        0x03 => Mode::Cwr,
        0x04 => Mode::Am,
        0x06 => Mode::Wfm,
        0x08 => Mode::Fm,
        0x0A => Mode::Dig,
        0x0C => Mode::Pkt,
        _ => Mode::Unknown(m),
    };
    Ok(FreqMode { hz: tens_of_hz * 10, mode, narrow: m & 0x80 != 0 })
}

pub fn mode_label(mode: Mode, narrow: bool, dig: Option<DigMode>) -> String {
    let base = match mode {
        Mode::Lsb => "LSB".to_string(),
        Mode::Usb => "USB".to_string(),
        Mode::Cw => "CW".to_string(),
        Mode::Cwr => "CWR".to_string(),
        Mode::Am => "AM".to_string(),
        Mode::Wfm => "WFM".to_string(),
        Mode::Fm => "FM".to_string(),
        Mode::Dig => "DIG".to_string(),
        Mode::Pkt => "PKT".to_string(),
        Mode::Unknown(b) => format!("?{b:02X}"),
    };
    let base = if narrow { format!("{base}-N") } else { base };
    match (mode, dig) {
        (Mode::Dig, Some(d)) => format!("{base} {}", d.label()),
        _ => base,
    }
}

// ---------------------------------------------------------------- RX status

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RxStatus {
    /// 0..=15: 0-9 = S0..S9, 10 = S9+10 dB, 11 = S9+20 dB ...
    pub s_raw: u8,
    /// FM/PKT: discriminator off-center (bit 5)
    pub discr_off: bool,
    /// CTCSS/DCS enabled but wrong or missing tone (bit 6)
    pub tone_mismatch: bool,
    /// Receiver squelched (bit 7); false = signal present / BUSY
    pub squelched: bool,
}

/// Command E7 (valid only in receive).
pub fn decode_rx_status(b: u8) -> RxStatus {
    RxStatus {
        s_raw: b & 0x0F,
        discr_off: b & 0x20 != 0,
        tone_mismatch: b & 0x40 != 0,
        squelched: b & 0x80 != 0,
    }
}

pub fn s_label(s_raw: u8) -> String {
    if s_raw <= 9 {
        format!("S{s_raw}")
    } else {
        format!("S9+{}", (s_raw as u32 - 9) * 10)
    }
}

// ---------------------------------------------------------------- TX status

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxStatus {
    pub ptt: bool,
    /// Power output reading 0..=15 (independent of the radio's meter setting)
    pub po_raw: u8,
    pub high_swr: bool,
}

/// Command F7. In receive the radio answers 0xFF (this is how Hamlib detects PTT).
/// Bit 5 (split) is deliberately ignored: published sources disagree on it.
/// Split is read from EEPROM 0x7A bit 7 instead.
pub fn decode_tx_status(b: u8) -> TxStatus {
    if b == 0xFF {
        return TxStatus { ptt: false, po_raw: 0, high_swr: false };
    }
    TxStatus { ptt: true, po_raw: b & 0x0F, high_swr: b & 0x40 != 0 }
}

// ---------------------------------------------------------------- TX meters

/// Published sources disagree on how the undocumented BD reply packs its
/// four nibbles, so the layout is a user setting. All agree on PWR.
/// `Tested` comes from a real FT-817ND (Nov 2026): byte 0 low nibble rose
/// with no antenna connected (SWR); byte 1 low followed the voice (MOD);
/// byte 1 high stayed at zero at normal drive (ALC).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MeterMap {
    /// Field-tested on an FT-817ND: byte0 = PWR|SWR, byte1 = ALC|MOD
    #[default]
    Tested,
    /// KA7OEI: byte0 = PWR|SWR, byte1 = MOD|ALC
    Ka7oei,
    /// Hamlib-style: byte0 = PWR|ALC, byte1 = SWR|MOD
    Alternate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxMeter {
    pub pwr: u8,
    pub swr: u8,
    pub alc: u8,
    pub modulation: u8,
}

/// Command BD. Returns None in receive (1-byte 0x00 reply).
pub fn decode_tx_meter(b: &[u8], map: MeterMap) -> Option<TxMeter> {
    if b.len() < 2 {
        return None;
    }
    let (b0, b1) = (b[0], b[1]);
    Some(match map {
        MeterMap::Tested => TxMeter { pwr: b0 >> 4, swr: b0 & 0x0F, alc: b1 >> 4, modulation: b1 & 0x0F },
        MeterMap::Ka7oei => TxMeter { pwr: b0 >> 4, swr: b0 & 0x0F, modulation: b1 >> 4, alc: b1 & 0x0F },
        MeterMap::Alternate => TxMeter { pwr: b0 >> 4, alc: b0 & 0x0F, swr: b1 >> 4, modulation: b1 & 0x0F },
    })
}

// ---------------------------------------------------------------- EEPROM flags

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct VfoState {
    pub label: String, // "VFO A", "VFO B", "MEM", "HOME", "QMB", "MTQMB", "MTUNE"
}

pub fn decode_vfo_state(b: u8) -> VfoState {
    let vfo_mode = b & 0x80 != 0;
    let label = if b & 0x10 != 0 {
        "HOME"
    } else if b & 0x04 != 0 {
        "QMB"
    } else if b & 0x02 != 0 {
        "MTQMB"
    } else if vfo_mode {
        if b & 0x01 != 0 { "VFO B" } else { "VFO A" }
    } else if b & 0x20 != 0 {
        "MTUNE"
    } else {
        "MEM"
    };
    VfoState { label: label.to_string() }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Panel {
    pub agc: String,
    pub dsp: bool,
    pub pbt: bool,
    pub nb: bool,
    pub lock: bool,
    pub fst: bool,
    pub meter_mode: String,
    pub kyr: bool,
    pub bk: bool,
    pub vlt: bool,
    pub vox: bool,
}

pub fn decode_panel(p57: u8, p58: u8) -> Panel {
    Panel {
        agc: ["AUTO", "FAST", "SLOW", "OFF"][(p57 & 0x03) as usize].to_string(),
        dsp: p57 & 0x04 != 0,
        pbt: p57 & 0x10 != 0,
        nb: p57 & 0x20 != 0,
        lock: p57 & 0x40 != 0,
        fst: p57 & 0x80 != 0,
        meter_mode: ["PWR", "ALC", "SWR", "MOD"][(p58 & 0x03) as usize].to_string(),
        kyr: p58 & 0x10 != 0,
        bk: p58 & 0x20 != 0,
        vlt: p58 & 0x40 != 0,
        vox: p58 & 0x80 != 0,
    }
}

pub fn decode_tx_power(b: u8) -> String {
    ["HI", "L3", "L2", "L1"][(b & 0x03) as usize].to_string()
}

pub fn decode_split(b7a: u8) -> bool {
    b7a & 0x80 != 0
}

/// Antenna (front/rear) for the band containing `hz`. EEPROM 0x7A holds one bit
/// per band group; the group is inferred from the frequency.
pub fn decode_antenna(b7a: u8, hz: u64) -> &'static str {
    let mhz = hz as f64 / 1e6;
    let bit = if mhz < 50.0 {
        0 // HF
    } else if mhz < 56.0 {
        1 // 6 m
    } else if (76.0..108.0).contains(&mhz) {
        2 // FM broadcast
    } else if (108.0..137.0).contains(&mhz) {
        3 // airband
    } else if (144.0..148.0).contains(&mhz) {
        4 // 2 m
    } else if (420.0..470.0).contains(&mhz) {
        5 // UHF
    } else {
        6 // general coverage ("phantom")
    };
    if b7a & (1 << bit) != 0 { "REAR" } else { "FRONT" }
}

/// Radio LCD color setting (menu #15): Color1 = blue, Color2 = amber.
pub fn decode_color(b5b: u8) -> &'static str {
    if b5b & 0x10 != 0 { "amber" } else { "blue" }
}

// ---------------------------------------------------------------- bands

pub fn band_name(hz: u64) -> &'static str {
    let k = hz / 1000;
    match k {
        1800..=2000 => "160m",
        3500..=4000 => "80m",
        5250..=5450 => "60m",
        7000..=7300 => "40m",
        10100..=10150 => "30m",
        14000..=14350 => "20m",
        18068..=18168 => "17m",
        21000..=21450 => "15m",
        24890..=24990 => "12m",
        28000..=29700 => "10m",
        50000..=54000 => "6m",
        76000..=107999 => "FM BC",
        108000..=136999 => "AIR",
        144000..=148000 => "2m",
        430000..=450000 => "70cm",
        _ => "GEN",
    }
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_write_and_reset_opcodes() {
        for op in [0x00u8, 0x01, 0x07, 0x08, 0x0F, 0x8F, 0xBC, 0xBE] {
            assert_eq!(frame([0; 4], op), Err(CatError::OpcodeNotAllowed(op)));
        }
        for op in READ_ONLY_OPCODES {
            assert!(frame([0; 4], op).is_ok());
        }
    }

    #[test]
    fn eeprom_frame_layout() {
        assert_eq!(eeprom_read(0x0055), [0x00, 0x55, 0, 0, 0xBB]);
        assert_eq!(eeprom_read(0x01B5), [0x01, 0xB5, 0, 0, 0xBB]);
    }

    #[test]
    fn freq_mode_uhf_fm() {
        // 439.70 MHz FM: BCD 43 97 00 00, mode byte 08
        let fm = decode_freq_mode(&[0x43, 0x97, 0x00, 0x00, 0x08]).unwrap();
        assert_eq!(fm.hz, 439_700_000);
        assert_eq!(fm.mode, Mode::Fm);
        assert!(!fm.narrow);
    }

    #[test]
    fn freq_mode_hf_and_narrow() {
        let fm = decode_freq_mode(&[0x01, 0x40, 0x74, 0x00, 0x0A]).unwrap();
        assert_eq!(fm.hz, 14_074_000);
        assert_eq!(fm.mode, Mode::Dig);
        let cwn = decode_freq_mode(&[0x00, 0x70, 0x30, 0x50, 0x82]).unwrap();
        assert_eq!(cwn.hz, 7_030_500);
        assert_eq!(cwn.mode, Mode::Cw);
        assert!(cwn.narrow);
        assert_eq!(mode_label(cwn.mode, cwn.narrow, None), "CW-N");
        assert_eq!(mode_label(Mode::Dig, false, Some(DigMode::UserU)), "DIG USER-U");
    }

    #[test]
    fn freq_mode_rejects_garbage() {
        assert_eq!(decode_freq_mode(&[0x1A, 0, 0, 0, 0]), Err(CatError::BadBcd(0x1A)));
        assert!(matches!(decode_freq_mode(&[0, 0, 0]), Err(CatError::BadLength { .. })));
    }

    #[test]
    fn rx_status_bits() {
        let s = decode_rx_status(0x09);
        assert_eq!((s.s_raw, s.squelched, s.tone_mismatch, s.discr_off), (9, false, false, false));
        let s = decode_rx_status(0xE3);
        assert_eq!((s.s_raw, s.squelched, s.tone_mismatch, s.discr_off), (3, true, true, true));
        assert_eq!(s_label(0), "S0");
        assert_eq!(s_label(9), "S9");
        assert_eq!(s_label(11), "S9+20");
    }

    #[test]
    fn tx_status_rx_is_ff() {
        assert!(!decode_tx_status(0xFF).ptt);
        let t = decode_tx_status(0x47);
        assert!(t.ptt);
        assert_eq!(t.po_raw, 7);
        assert!(t.high_swr);
    }

    #[test]
    fn tx_meter_maps() {
        assert_eq!(decode_tx_meter(&[0x00], MeterMap::Ka7oei), None);
        let m = decode_tx_meter(&[0xA2, 0x53], MeterMap::Ka7oei).unwrap();
        assert_eq!((m.pwr, m.swr, m.modulation, m.alc), (10, 2, 5, 3));
        let m = decode_tx_meter(&[0xA2, 0x53], MeterMap::Tested).unwrap();
        assert_eq!((m.pwr, m.swr, m.alc, m.modulation), (10, 2, 5, 3));
        assert_eq!(MeterMap::default(), MeterMap::Tested);
        let m = decode_tx_meter(&[0xA2, 0x53], MeterMap::Alternate).unwrap();
        assert_eq!((m.pwr, m.alc, m.swr, m.modulation), (10, 2, 5, 3));
    }

    #[test]
    fn eeprom_flags() {
        assert_eq!(decode_vfo_state(0x80).label, "VFO A");
        assert_eq!(decode_vfo_state(0x81).label, "VFO B");
        assert_eq!(decode_vfo_state(0x00).label, "MEM");
        assert_eq!(decode_vfo_state(0x90).label, "HOME");
        let p = decode_panel(0b0110_0010, 0b1011_0011);
        assert_eq!(p.agc, "SLOW");
        assert!(p.nb && p.lock && !p.dsp && !p.fst);
        assert_eq!(p.meter_mode, "MOD");
        assert!(p.vox && p.bk && p.kyr && !p.vlt);
        assert_eq!(decode_tx_power(0x03), "L1");
        assert!(decode_split(0x80));
        assert_eq!(decode_antenna(0b0001_0000, 145_500_000), "REAR");
        assert_eq!(decode_antenna(0b0001_0000, 14_074_000), "FRONT");
        assert_eq!(DigMode::from_eeprom(0x80), Some(DigMode::UserU));
        assert_eq!(decode_color(0x10), "amber");
    }

    #[test]
    fn bands() {
        assert_eq!(band_name(14_074_000), "20m");
        assert_eq!(band_name(5_357_000), "60m");
        assert_eq!(band_name(9_500_000), "GEN");
        assert_eq!(band_name(145_500_000), "2m");
    }
}
