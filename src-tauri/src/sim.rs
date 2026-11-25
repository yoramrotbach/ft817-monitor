//! A simulated FT-817 that answers CAT frames byte-for-byte like the radio.
//! Used by Demo mode and by the `sim_serial` example (a virtual radio on a
//! serial port / pty) so the full decode path can be tested without hardware.
//!
//! The scenario repeats every 60 s:
//!   0-30 s  14.074 MHz DIG USER-U, FT8-style: TX in alternate 15 s slots
//!  30-45 s  7.030 MHz CW-N, keyed in short bursts
//!  45-60 s  145.500 MHz FM, squelch opening and closing

use crate::cat::{self, ee};
use std::time::Instant;

pub struct SimRadio {
    start: Instant,
    rng: u64,
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

impl SimRadio {
    pub fn new() -> Self {
        Self { start: Instant::now(), rng: 0x9E37_79B9_7F4A_7C15 }
    }

    fn noise(&mut self, span: u8) -> u8 {
        // xorshift64
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng % (span as u64 + 1)) as u8
    }

    fn snapshot(&mut self) -> Snapshot {
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

    /// Answer one 5-byte CAT frame exactly like the radio would.
    pub fn respond(&mut self, frame: &[u8; 5]) -> Vec<u8> {
        let s = self.snapshot();
        match frame[4] {
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
                    // KA7OEI layout: PWR|SWR, MOD|ALC
                    vec![(s.pwr << 4) | s.swr, (s.modulation << 4) | s.alc]
                } else {
                    vec![0x00]
                }
            }
            cat::OP_READ_EEPROM => {
                let addr = ((frame[0] as u16) << 8) | frame[1] as u16;
                let byte = |a: u16| -> u8 {
                    match a {
                        ee::VFO_STATE => 0x80,                   // VFO mode, VFO A
                        ee::PANEL_1 => 0x20 | s.agc_bits,        // NB on
                        ee::PANEL_2 => if s.hz < 10_000_000 { 0x30 } else { 0x00 }, // KYR+BK on 40 m
                        ee::DISPLAY => 0x10,                     // amber
                        ee::DIG_MODE => 0x80,                    // USER-U
                        ee::TX_POWER => 0x00,                    // HI
                        ee::ANT_SPLIT => 0x10,                   // 2 m on rear connector
                        _ => 0x00,
                    }
                };
                vec![byte(addr), byte(addr + 1)]
            }
            _ => vec![], // radio ignores unknown frames
        }
    }
}
