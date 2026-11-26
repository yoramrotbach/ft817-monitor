//! EEPROM backup and restore for the FT-817/818.
//!
//! This module is the ONLY code in the project that can write to a radio.
//! Safety rules, all enforced here and covered by tests:
//!   1. Addresses 0x00-0x05 are never written (0-3 = integrity checksum whose
//!      change can wipe the whole EEPROM; 4-5 = radio version bytes).
//!   2. Restore is refused if the backup's version bytes differ from the radio's.
//!   3. Backups are read twice; unstable bytes are re-read until consistent.
//!   4. Only bytes that differ are written, each one read back and verified.
//!   5. The radio's current CAT baud-rate bits (0x64 bits 7-6) are kept, so
//!      the link cannot drop in the middle of a restore.
//!   6. The radio must be in receive with the dial lock off.
//!   7. No factory-reset (0xBE) frame can be built anywhere.

use ft817_cat::cat::{self, ee};
use ft817_cat::transport::CatPort;
use std::fmt;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const PROTECTED_END: u16 = 0x0006; // 0x00-0x05 are never written
pub const VERSION_BYTES: std::ops::Range<usize> = 4..6;
pub const CAT_RATE_ADDR: usize = 0x64;
pub const CAT_RATE_MASK: u8 = 0xC0;
pub const CAL_RANGE: std::ops::RangeInclusive<usize> = 0x07..=0x53; // 76 soft-cal + VCC
/// Documented FT-817/818 EEPROM size: valid addresses 0x0000-0x1925.
/// The backup reads exactly this range and never asks past the end, because
/// real radios don't all answer out-of-range reads the same way.
pub const EE_LEN: usize = 0x1926;
const OP_WRITE_EEPROM: u8 = 0xBC;
const WRITE_SETTLE: Duration = Duration::from_millis(30);
const READ_RETRIES: u32 = 4;

#[derive(Debug)]
pub enum EeError {
    Io(io::Error),
    Read { addr: usize, detail: String },
    Cancelled,
    Transmitting,
    DialLocked,
    Protected(u16),
    Unstable(Vec<usize>),
    LengthMismatch { radio: usize, backup: usize },
    VersionMismatch { radio: [u8; 2], backup: [u8; 2] },
    LinkUnreliable(usize),
    TooShort(usize),
}

impl fmt::Display for EeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EeError::Io(e) => write!(f, "No reliable reply from the radio ({e}). Check the cable, power and baud rate."),
            EeError::Read { addr, detail } => write!(f, "No reliable reply from the radio when reading EEPROM address 0x{addr:04X} ({detail}). Check the cable, power and baud rate."),
            EeError::Cancelled => write!(f, "Cancelled."),
            EeError::Transmitting => write!(f, "The radio is transmitting. Put it in receive and try again."),
            EeError::DialLocked => write!(f, "The radio's dial LOCK is on. The FT-817 can stop answering CAT commands while locked; turn LOCK off and try again."),
            EeError::Protected(a) => write!(f, "Refused to write protected address 0x{a:04X}."),
            EeError::Unstable(v) => write!(f, "{} EEPROM bytes read differently on repeated reads (first at 0x{:04X}). Leave the radio untouched and try again.", v.len(), v.first().copied().unwrap_or(0)),
            EeError::LengthMismatch { radio, backup } => write!(f, "The backup is {backup} bytes but this radio's EEPROM is {radio} bytes. It belongs to a different model."),
            EeError::VersionMismatch { radio, backup } => write!(f, "The backup's radio version bytes ({:02X} {:02X}) don't match this radio ({:02X} {:02X}). It is from a different model or hardware variant, so restoring is refused.", backup[0], backup[1], radio[0], radio[1]),
            EeError::LinkUnreliable(a) => write!(f, "Writes stopped verifying at 0x{a:04X}; the restore was stopped. Your pre-restore backup is saved, so nothing is lost: check the cable and run the restore again."),
            EeError::TooShort(n) => write!(f, "The radio reported only {n} EEPROM bytes, which is too small to be valid."),
        }
    }
}

impl From<io::Error> for EeError {
    fn from(e: io::Error) -> Self {
        EeError::Io(e)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Progress {
    pub phase: String,
    pub done: usize,
    pub total: usize,
}

pub struct Ctx<'a> {
    pub port: &'a mut dyn CatPort,
    pub cancel: &'a AtomicBool,
    pub progress: &'a mut dyn FnMut(Progress),
}

impl Ctx<'_> {
    fn check_cancel(&self) -> Result<(), EeError> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(EeError::Cancelled)
        } else {
            Ok(())
        }
    }
    fn report(&mut self, phase: &str, done: usize, total: usize) {
        (self.progress)(Progress { phase: phase.into(), done, total });
    }
}

// ---------------------------------------------------------------- frames

/// The only way to build an EEPROM write frame. Refuses the protected header.
pub fn write_frame(addr: u16, data: [u8; 2]) -> Result<[u8; 5], EeError> {
    if addr < PROTECTED_END {
        return Err(EeError::Protected(addr));
    }
    Ok([(addr >> 8) as u8, (addr & 0xFF) as u8, data[0], data[1], OP_WRITE_EEPROM])
}

// ---------------------------------------------------------------- reading

enum Pair {
    Data([u8; 2]),
    End,
}

fn read_pair(port: &mut dyn CatPort, addr: usize) -> Result<Pair, EeError> {
    let mut detail = String::new();
    for _ in 0..READ_RETRIES {
        match port.transact(cat::eeprom_read(addr as u16), 2) {
            Ok(r) if r.len() == 2 => return Ok(Pair::Data([r[0], r[1]])),
            Ok(r) if r.len() == 1 && r[0] == 0 => return Ok(Pair::End),
            Ok(r) => {
                let hex: Vec<String> = r.iter().map(|b| format!("{b:02X}")).collect();
                detail = format!("got {} byte(s): {}", r.len(), hex.join(" "));
            }
            Err(e) => detail = e.to_string(),
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(EeError::Read { addr, detail })
}

/// A 1-byte zero reply means "past the end", but the same thing can happen
/// as a glitch. Confirm the end by asking twice more.
fn confirmed_end(port: &mut dyn CatPort, addr: usize) -> Result<bool, EeError> {
    for _ in 0..2 {
        if let Pair::Data(_) = read_pair(port, addr)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Radio must be in receive with the dial lock off. Each check is asked up
/// to three times so a single corrupted reply can't block or fool it.
pub fn preflight(port: &mut dyn CatPort) -> Result<(), EeError> {
    let mut receiving = false;
    let mut last_err = None;
    for _ in 0..3 {
        match port.transact(cat::simple(cat::OP_READ_TX_STATUS).unwrap(), 1) {
            Ok(r) if r.first() == Some(&0xFF) => {
                receiving = true;
                break;
            }
            Ok(_) => {}
            Err(e) => last_err = Some(e),
        }
    }
    if !receiving {
        return Err(match last_err {
            Some(e) => EeError::Io(e),
            None => EeError::Transmitting,
        });
    }
    let mut locked_votes = 0;
    for _ in 0..3 {
        if let Pair::Data([p57, _]) = read_pair(port, ee::PANEL_1 as usize)? {
            if p57 & 0x40 != 0 {
                locked_votes += 1;
            }
        }
    }
    if locked_votes >= 2 {
        return Err(EeError::DialLocked);
    }
    Ok(())
}

fn read_pass(ctx: &mut Ctx, label: &str) -> Result<Vec<u8>, EeError> {
    let mut img = Vec::with_capacity(EE_LEN);
    let mut addr = 0usize;
    while addr < EE_LEN {
        ctx.check_cancel()?;
        match read_pair(ctx.port, addr)? {
            Pair::Data(d) => img.extend_from_slice(&d),
            Pair::End => {
                // A radio with a smaller EEPROM than documented.
                if confirmed_end(ctx.port, addr)? {
                    break;
                }
                continue; // glitch: read this address again
            }
        }
        addr += 2;
        if addr % 64 == 0 {
            ctx.report(label, addr.min(EE_LEN), EE_LEN);
        }
    }
    img.truncate(EE_LEN);
    ctx.report(label, img.len(), img.len());
    if img.len() < 0x400 {
        return Err(EeError::TooShort(img.len()));
    }
    Ok(img)
}

/// Read the whole EEPROM twice and reconcile any differences by re-reading.
pub fn backup(ctx: &mut Ctx) -> Result<Vec<u8>, EeError> {
    preflight(ctx.port)?;
    let mut a = read_pass(ctx, "Reading EEPROM (pass 1 of 2)")?;
    let b = read_pass(ctx, "Verifying (pass 2 of 2)")?;
    if a.len() != b.len() {
        return Err(EeError::Unstable(vec![a.len().min(b.len())]));
    }
    let mut diffs: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i]).map(|i| i & !1).collect();
    diffs.dedup();
    let mut unstable = Vec::new();
    for (n, &pair) in diffs.iter().enumerate() {
        ctx.check_cancel()?;
        ctx.report("Re-reading inconsistent bytes", n, diffs.len());
        // Accept a value once three consecutive reads agree.
        let mut agreed = None;
        let mut prev: Option<[u8; 2]> = None;
        let mut streak = 0;
        for _ in 0..12 {
            if let Pair::Data(d) = read_pair(ctx.port, pair)? {
                if prev == Some(d) {
                    streak += 1;
                } else {
                    streak = 1;
                    prev = Some(d);
                }
                if streak == 3 {
                    agreed = Some(d);
                    break;
                }
            }
        }
        match agreed {
            Some(d) => {
                a[pair] = d[0];
                if pair + 1 < a.len() {
                    a[pair + 1] = d[1];
                }
            }
            None => unstable.push(pair),
        }
    }
    if !unstable.is_empty() {
        return Err(EeError::Unstable(unstable));
    }
    Ok(a)
}

// ---------------------------------------------------------------- restore

#[derive(Debug, Clone, serde::Serialize)]
pub struct Write {
    pub addr: u16,
    pub data: [u8; 2],
    pub was: [u8; 2],
}

#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct RegionCounts {
    pub calibration: usize,
    pub settings: usize,
    pub vfos: usize,
    pub memories: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Plan {
    pub writes: Vec<Write>,
    pub bytes_changed: usize,
    pub regions: RegionCounts,
    /// The backup's CAT rate differed and the radio's current rate was kept.
    pub cat_rate_kept: bool,
}

/// Region names follow the FT-817 memory map (approximate on the FT-818).
pub fn region(addr: usize) -> &'static str {
    match addr {
        0x00..=0x06 => "header",
        0x07..=0x53 => "calibration",
        0x54..=0x7C => "settings",
        0x7D..=0x44E => "vfos",
        _ => "memories",
    }
}

/// The image the radio should hold after a restore: the backup, except
/// bytes 0-5 and the CAT-rate bits, which keep the radio's current values.
pub fn target_image(current: &[u8], backup: &[u8]) -> Vec<u8> {
    let mut t = backup.to_vec();
    t[..PROTECTED_END as usize].copy_from_slice(&current[..PROTECTED_END as usize]);
    if CAT_RATE_ADDR < t.len() {
        t[CAT_RATE_ADDR] = (backup[CAT_RATE_ADDR] & !CAT_RATE_MASK) | (current[CAT_RATE_ADDR] & CAT_RATE_MASK);
    }
    t
}

pub fn plan(current: &[u8], backup: &[u8]) -> Result<Plan, EeError> {
    if current.len() != backup.len() {
        return Err(EeError::LengthMismatch { radio: current.len(), backup: backup.len() });
    }
    if current[VERSION_BYTES] != backup[VERSION_BYTES] {
        return Err(EeError::VersionMismatch {
            radio: [current[4], current[5]],
            backup: [backup[4], backup[5]],
        });
    }
    let target = target_image(current, backup);
    let mut writes = Vec::new();
    let mut regions = RegionCounts::default();
    let mut bytes_changed = 0;
    let mut addr = PROTECTED_END as usize;
    while addr < target.len() {
        // Pair writes always cover addr and addr+1; at the very end, the
        // second byte is the radio's own current value.
        let hi = addr + 1;
        let want = [target[addr], if hi < target.len() { target[hi] } else { 0 }];
        let was = [current[addr], if hi < current.len() { current[hi] } else { 0 }];
        if hi >= target.len() {
            // Last odd byte: skip if equal, otherwise write it with a neighbor pair below.
            if want[0] != was[0] {
                let a = addr - 1;
                writes.push(Write { addr: a as u16, data: [target[a], target[addr]], was: [current[a], current[addr]] });
                bytes_changed += 1;
                *region_slot(&mut regions, addr) += 1;
            }
            break;
        }
        if want != was {
            for (i, a) in [addr, hi].into_iter().enumerate() {
                if want[i] != was[i] {
                    bytes_changed += 1;
                    *region_slot(&mut regions, a) += 1;
                }
            }
            writes.push(Write { addr: addr as u16, data: want, was });
        }
        addr += 2;
    }
    let cat_rate_kept = CAT_RATE_ADDR < backup.len()
        && (backup[CAT_RATE_ADDR] & CAT_RATE_MASK) != (current[CAT_RATE_ADDR] & CAT_RATE_MASK);
    Ok(Plan { writes, bytes_changed, regions, cat_rate_kept })
}

fn region_slot(r: &mut RegionCounts, addr: usize) -> &mut usize {
    match region(addr) {
        "calibration" => &mut r.calibration,
        "settings" => &mut r.settings,
        "vfos" => &mut r.vfos,
        _ => &mut r.memories,
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RestoreReport {
    pub written: usize,
    pub retried: usize,
    /// Bytes that still differ from the backup after the final full read-back.
    pub final_mismatches: Vec<usize>,
}

/// Execute a plan made from `current` (which must be fresh) and `backup`.
pub fn restore(ctx: &mut Ctx, plan: &Plan, current: &[u8], backup: &[u8]) -> Result<RestoreReport, EeError> {
    preflight(ctx.port)?;
    // Re-check the version bytes on the live radio right before writing.
    if let Pair::Data(v) = read_pair(ctx.port, 4)? {
        if v != [backup[4], backup[5]] {
            return Err(EeError::VersionMismatch { radio: v, backup: [backup[4], backup[5]] });
        }
    }
    let total = plan.writes.len();
    let mut retried = 0;
    let mut consecutive_failures = 0;
    for (n, w) in plan.writes.iter().enumerate() {
        ctx.check_cancel()?;
        ctx.report("Writing", n, total);
        let frame = write_frame(w.addr, w.data)?;
        let mut ok = false;
        for attempt in 0..3 {
            if attempt > 0 {
                retried += 1;
            }
            // The radio normally acknowledges with one byte; a missing ack is
            // tolerated because the read-back below is what counts.
            let _ = ctx.port.transact(frame, 1);
            std::thread::sleep(WRITE_SETTLE);
            if let Ok(Pair::Data(d)) = read_pair(ctx.port, w.addr as usize) {
                if d == w.data {
                    ok = true;
                    break;
                }
            }
        }
        if ok {
            consecutive_failures = 0;
        } else {
            consecutive_failures += 1;
            if consecutive_failures >= 3 {
                return Err(EeError::LinkUnreliable(w.addr as usize));
            }
        }
    }
    ctx.report("Writing", total, total);

    // Final check: read everything again and compare with what we intended.
    let target = target_image(current, backup);
    let after = read_pass(ctx, "Final verification")?;
    let final_mismatches = if after.len() != target.len() {
        vec![after.len().min(target.len())]
    } else {
        let mut bad = Vec::new();
        for i in (0..target.len()).filter(|&i| after[i] != target[i]) {
            // A single read can be corrupted in transit: confirm before reporting.
            let pair = i & !1;
            let mut confirmed_bad = true;
            for _ in 0..3 {
                if let Ok(Pair::Data(d)) = read_pair(ctx.port, pair) {
                    if d[i - pair] == target[i] {
                        confirmed_bad = false;
                        break;
                    }
                }
            }
            if confirmed_bad {
                bad.push(i);
            }
        }
        bad
    };
    Ok(RestoreReport { written: total, retried, final_mismatches })
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use ft817_cat::sim::{SimRadio, SIM_EE_LEN};
    use ft817_cat::transport::DemoCat;
    use std::sync::{Arc, Mutex};

    fn rig(fault: u8) -> (Arc<Mutex<SimRadio>>, DemoCat) {
        let mut r = SimRadio::new_static();
        r.fault_percent = fault;
        let r = Arc::new(Mutex::new(r));
        (r.clone(), DemoCat::with_radio(r).instant())
    }

    fn run<T>(port: &mut DemoCat, f: impl FnOnce(&mut Ctx) -> T) -> T {
        let cancel = AtomicBool::new(false);
        let mut sink = |_p: Progress| {};
        let mut ctx = Ctx { port, cancel: &cancel, progress: &mut sink };
        f(&mut ctx)
    }

    #[test]
    fn write_frame_refuses_header() {
        for a in 0..PROTECTED_END {
            assert!(matches!(write_frame(a, [0, 0]), Err(EeError::Protected(_))));
        }
        assert_eq!(write_frame(0x0006, [1, 2]).unwrap(), [0x00, 0x06, 1, 2, 0xBC]);
    }

    #[test]
    fn backup_matches_radio_exactly() {
        let (r, mut port) = rig(0);
        let img = run(&mut port, |c| backup(c)).unwrap();
        assert_eq!(img.len(), SIM_EE_LEN);
        assert_eq!(img, r.lock().unwrap().ee);
    }

    #[test]
    fn backup_never_reads_past_the_documented_end() {
        // Real radios differ here: a non-zero byte, two junk bytes, or silence.
        for reply in [vec![0x1F], vec![0xAA, 0x55], vec![]] {
            let (r, mut port) = rig(0);
            r.lock().unwrap().past_end_reply = Some(reply.clone());
            let img = run(&mut port, |c| backup(c)).unwrap_or_else(|e| panic!("{reply:?}: {e}"));
            assert_eq!(img, r.lock().unwrap().ee);
        }
    }

    #[test]
    fn backup_survives_dropped_and_corrupted_replies() {
        let (r, mut port) = rig(3); // 3% of replies dropped or bit-flipped
        let img = run(&mut port, |c| backup(c)).unwrap();
        assert_eq!(img, r.lock().unwrap().ee);
    }

    #[test]
    fn restore_brings_back_changed_bytes_and_never_touches_header() {
        let (r, mut port) = rig(0);
        let saved = run(&mut port, |c| backup(c)).unwrap();
        {
            let mut g = r.lock().unwrap();
            g.ee[0x10] ^= 0xFF; // calibration
            g.ee[0x57] ^= 0x01; // settings
            g.ee[0x500] ^= 0x55; // memories
            g.ee[SIM_EE_LEN - 1] ^= 0x01; // last byte
        }
        let current = run(&mut port, |c| backup(c)).unwrap();
        let p = plan(&current, &saved).unwrap();
        assert_eq!(p.bytes_changed, 4);
        assert_eq!(p.regions.calibration, 1);
        assert!(p.writes.iter().all(|w| w.addr >= PROTECTED_END));
        let rep = run(&mut port, |c| restore(c, &p, &current, &saved)).unwrap();
        assert!(rep.final_mismatches.is_empty(), "{:?}", rep.final_mismatches);
        let g = r.lock().unwrap();
        assert_eq!(g.ee, saved);
        assert!(!g.wiped);
        assert_eq!(g.writes as usize, p.writes.len());
    }

    #[test]
    fn restore_with_faults_still_ends_identical() {
        let (r, mut port) = rig(0);
        let saved = run(&mut port, |c| backup(c)).unwrap();
        for a in (0x100..0x140).step_by(3) {
            r.lock().unwrap().ee[a] ^= 0xA5;
        }
        r.lock().unwrap().fault_percent = 4;
        let current = run(&mut port, |c| backup(c)).unwrap();
        let p = plan(&current, &saved).unwrap();
        let rep = run(&mut port, |c| restore(c, &p, &current, &saved)).unwrap();
        assert!(rep.final_mismatches.is_empty());
        assert_eq!(r.lock().unwrap().ee, saved);
    }

    #[test]
    fn version_mismatch_is_refused() {
        let (_r, mut port) = rig(0);
        let current = run(&mut port, |c| backup(c)).unwrap();
        let mut other = current.clone();
        other[5] ^= 0x01;
        assert!(matches!(plan(&current, &other), Err(EeError::VersionMismatch { .. })));
        let mut short = current.clone();
        short.truncate(0x1000);
        assert!(matches!(plan(&current, &short), Err(EeError::LengthMismatch { .. })));
    }

    #[test]
    fn checksum_and_cat_rate_are_preserved() {
        let (_r, mut port) = rig(0);
        let current = run(&mut port, |c| backup(c)).unwrap();
        let mut b = current.clone();
        b[0] ^= 0xFF; // a backup with a different checksum byte
        b[CAT_RATE_ADDR] = (b[CAT_RATE_ADDR] & !CAT_RATE_MASK) | 0x00; // 4800 baud
        b[CAT_RATE_ADDR] ^= 0x01; // and a real setting change in the same byte
        let p = plan(&current, &b).unwrap();
        assert!(p.cat_rate_kept);
        assert_eq!(p.writes.len(), 1);
        let w = &p.writes[0];
        assert_eq!(w.addr as usize, CAT_RATE_ADDR);
        assert_eq!(w.data[0] & CAT_RATE_MASK, current[CAT_RATE_ADDR] & CAT_RATE_MASK);
        assert_eq!(w.data[0] & 0x01, b[CAT_RATE_ADDR] & 0x01);
    }

    #[test]
    fn preflight_refuses_locked_radio() {
        let (r, mut port) = rig(0);
        r.lock().unwrap().ee[0x57] |= 0x40;
        assert!(matches!(run(&mut port, |c| backup(c)), Err(EeError::DialLocked)));
    }

    #[test]
    fn simulator_wipes_if_checksum_written() {
        // Demonstrates the hazard the guard protects against.
        let mut r = SimRadio::new_static();
        r.respond(&[0x00, 0x00, 0x00, 0x00, 0xBC]);
        assert!(r.wiped);
    }
}
