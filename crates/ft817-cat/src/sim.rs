//! A simulated FT-817 that answers CAT frames byte-for-byte like the radio,
//! including a full EEPROM image that can be read (BB) and written (BC).
//!
//! Used by Demo modes, by the `sim_serial` example (a virtual radio on a serial
//! port / pty) and by tests. It also mimics the dangerous behavior documented by
//! KA7OEI: changing EEPROM bytes 0-3 (described as an integrity checksum;
//! a fixed A5 5A A5 5A on real FT-817NDs) wipes the EEPROM.
//!
//! Animated scenario (monitor demo), repeating every 60 s:
//!   0-30 s  14.074 MHz DIG USER-U, FT8-style: TX in alternate 15 s slots
//!  30-45 s  7.030 MHz CW-N, keyed in short bursts
//!  45-60 s  145.500 MHz FM, squelch opening and closing
//! Static mode (EEPROM tool demo): radio idle in receive, nothing changes.

use crate::cat::{self, ee};
use std::time::Instant;

/// FT-817 EEPROM size as documented (valid addresses 0x0000-0x1925).
pub const SIM_EE_LEN: usize = 0x1926;
const OP_WRITE_EEPROM: u8 = 0xBC;
const OP_FACTORY_RESET: u8 = 0xBE;

pub struct SimRadio {
    start: Instant,
    rng: u64,
    animate: bool,
    /// Reply to an EEPROM read past the end (None = one zero byte).
    pub past_end_reply: Option<Vec<u8>>,
    /// Full EEPROM image.
    pub ee: Vec<u8>,
    /// Percentage (0-100) of replies that are dropped or corrupted.
    pub fault_percent: u8,
    /// Number of EEPROM write frames received.
    pub writes: u32,
    /// Set if the simulated radio wiped itself (checksum bytes changed or BE received).
    pub wiped: bool,
}

impl Default for SimRadio {
    fn default() -> Self {
        Self::new()
    }
}

struct Snapshot {
    hz: u64,
    mode_byte: u8,
    ptt: bool,
    s_raw: u8,
    squelched: bool,
    pwr: u8,
    swr: u8,
    alc: u8,
    modulation: u8,
    agc_bits: u8,
}

fn default_image() -> Vec<u8> {
    let mut ee = vec![0u8; SIM_EE_LEN];
    for (a, b) in ee.iter_mut().enumerate() {
        // Deterministic filler standing in for VFOs and memories.
        *b = ((a as u32).wrapping_mul(37) ^ 0x5A) as u8;
    }
    ee[0..4].copy_from_slice(&[0xA5, 0x5A, 0xA5, 0x5A]); // fixed marker, as on real FT-817NDs
    ee[4] = 0xD8; // radio version (KA7OEI example values)
    ee[5] = 0xBF;
    ee[6] = 0x00;
    for i in 0x07..=0x52usize {
        ee[i] = 0x40 + ((i as u32 * 29) % 0x80) as u8; // 76 soft-calibration values
    }
    ee[0x53] = 0x8E; // VCC calibration
    ee[ee::VFO_STATE as usize] = 0x80; // VFO A
    ee[ee::PANEL_1 as usize] = 0x21; // AGC FAST, NB on
    ee[ee::PANEL_2 as usize] = 0x00;
    ee[ee::DISPLAY as usize] = 0x10; // amber
    ee[0x64] = 0x80 | 0x05; // CAT rate 38400 (bits 7-6 = 10) + other bits
    ee[ee::DIG_MODE as usize] = 0x80; // USER-U
    ee[ee::TX_POWER as usize] = 0x00; // HI
    ee[ee::ANT_SPLIT as usize] = 0x10; // 2 m on rear connector
    ee
}

impl SimRadio {
    /// Animated radio for the monitor demo.
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            rng: 0x9E37_79B9_7F4A_7C15,
            animate: true,
            ee: default_image(),
            past_end_reply: None,
            fault_percent: 0,
            writes: 0,
            wiped: false,
        }
    }

    /// Idle radio in receive whose EEPROM only changes when written.
    pub fn new_static() -> Self {
        Self { animate: false, ..Self::new() }
    }

    fn noise(&mut self, span: u8) -> u8 {
        // xorshift64
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng % (span as u64 + 1)) as u8
    }

    fn chance(&mut self, percent: u8) -> bool {
        percent > 0 && self.noise(99) < percent
    }

    fn snapshot(&mut self) -> Snapshot {
        if !self.animate {
            return Snapshot {
                hz: 14_060_000,
                mode_byte: 0x02,
                ptt: false,
                s_raw: 2,
                squelched: false,
                pwr: 0,
                swr: 0,
                alc: 0,
                modulation: 0,
                agc_bits: self.ee[ee::PANEL_1 as usize] & 0x03,
            };
        }
        let t = self.start.elapsed().as_secs_f64() % 60.0;
        if t < 30.0 {
            let slot = (t / 15.0) as u32;
            let in_slot = t % 15.0;
            let ptt = slot % 2 == 1 && (0.5..13.1).contains(&in_slot);
            Snapshot {
                hz: 14_074_000,
                mode_byte: 0x0A,
                ptt,
                s_raw: 3 + self.noise(5),
                squelched: false,
                pwr: if ptt { 11 + self.noise(1) } else { 0 },
                swr: if ptt { 2 } else { 0 },
                alc: if ptt { self.noise(2) } else { 0 },
                modulation: if ptt { 6 + self.noise(2) } else { 0 },
                agc_bits: 0b01, // FAST
            }
        } else if t < 45.0 {
            let keyed = ((t * 4.0) as u32) % 7 < 3 && (t % 5.0) < 3.0;
            let tune = ((t - 30.0) * 20.0) as u64 * 10; // slow tuning drift
            Snapshot {
                hz: 7_030_000 + tune,
                mode_byte: 0x82, // CW narrow
                ptt: keyed,
                s_raw: 5 + self.noise(4),
                squelched: false,
                pwr: if keyed { 7 } else { 0 },
                swr: if keyed { 1 } else { 0 },
                alc: 0,
                modulation: 0,
                agc_bits: 0b01,
            }
        } else {
            let open = (t % 6.0) < 3.5;
            Snapshot {
                hz: 145_500_000,
                mode_byte: 0x08,
                ptt: false,
                s_raw: if open { 10 + self.noise(2) } else { self.noise(1) },
                squelched: !open,
                pwr: 0,
                swr: 0,
                alc: 0,
                modulation: 0,
                agc_bits: 0b00, // AUTO
            }
        }
    }

    fn ee_read(&self, a: usize, s: &Snapshot) -> u8 {
        if self.animate {
            // Live panel state for the monitor demo.
            match a as u16 {
                ee::PANEL_1 => return 0x20 | s.agc_bits,
                ee::PANEL_2 => return if s.hz < 10_000_000 { 0x30 } else { 0x00 },
                _ => {}
            }
        }
        self.ee.get(a).copied().unwrap_or(0)
    }

    fn wipe(&mut self) {
        self.ee = vec![0xFF; SIM_EE_LEN];
        self.wiped = true;
    }

    /// Answer one 5-byte CAT frame exactly like the radio would.
    pub fn respond(&mut self, frame: &[u8; 5]) -> Vec<u8> {
        let s = self.snapshot();
        let fault = self.chance(self.fault_percent);
        let mut reply = match frame[4] {
            cat::OP_READ_FREQ_MODE => {
                let mut tens = s.hz / 10;
                let mut out = [0u8; 5];
                for i in (0..4).rev() {
                    let two = (tens % 100) as u8;
                    out[i] = ((two / 10) << 4) | (two % 10);
                    tens /= 100;
                }
                out[4] = s.mode_byte;
                out.to_vec()
            }
            cat::OP_READ_RX_STATUS => {
                let mut b = s.s_raw & 0x0F;
                if s.squelched {
                    b |= 0x80;
                }
                vec![b]
            }
            cat::OP_READ_TX_STATUS => {
                if s.ptt {
                    vec![s.pwr & 0x0F]
                } else {
                    vec![0xFF]
                }
            }
            cat::OP_READ_TX_METER => {
                if s.ptt {
                    // Layout seen on a real FT-817ND: PWR|SWR, ALC|MOD
                    vec![(s.pwr << 4) | s.swr, (s.alc << 4) | s.modulation]
                } else {
                    vec![0x00]
                }
            }
            cat::OP_READ_EEPROM => {
                let a = (((frame[0] as u16) << 8) | frame[1] as u16) as usize;
                if a >= self.ee.len() {
                    // Documented: one zero byte. Tests can override this to
                    // mimic radios that answer differently past the end.
                    self.past_end_reply.clone().unwrap_or_else(|| vec![0x00])
                } else {
                    vec![self.ee_read(a, &s), self.ee_read(a + 1, &s)]
                }
            }
            OP_WRITE_EEPROM => {
                let a = (((frame[0] as u16) << 8) | frame[1] as u16) as usize;
                self.writes += 1;
                if fault {
                    return vec![0x00]; // radio "missed" the write but still acked
                }
                let before: Vec<u8> = self.ee[0..4].to_vec();
                for (i, &d) in [frame[2], frame[3]].iter().enumerate() {
                    if let Some(b) = self.ee.get_mut(a + i) {
                        *b = d;
                    }
                }
                if self.ee[0..4] != before[..] {
                    self.wipe();
                }
                vec![0x00]
            }
            OP_FACTORY_RESET => {
                self.wipe();
                vec![]
            }
            _ => vec![], // radio ignores unknown frames
        };
        if fault && !reply.is_empty() {
            if self.noise(1) == 0 {
                reply.clear(); // dropped reply
            } else {
                let i = self.noise((reply.len() - 1) as u8) as usize;
                reply[i] ^= 1 << self.noise(7); // corrupted bit
            }
        }
        reply
    }
}
