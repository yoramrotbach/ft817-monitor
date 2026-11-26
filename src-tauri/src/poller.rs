//! Polling loops. Each runs on its own thread until `stop` is set and pushes
//! RigState snapshots to `emit` (throttled to ~30 updates per second).

use crate::cat::{self, ee, DigMode, Mode};
use crate::state::{Link, RigState};
use crate::transport::{CatPort, DemoCat, SerialCat};
use serde::Deserialize;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase", tag = "kind")]
pub enum Source {
    Serial { port: String, baud: u32 },
    Demo,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MonitorConfig {
    pub source: Source,
    /// Use the undocumented BD command for SWR/ALC/MOD while transmitting.
    #[serde(default)]
    pub read_tx_meters: bool,
}

const EMIT_EVERY: Duration = Duration::from_millis(33);
const SLOW_EVERY: Duration = Duration::from_millis(700);
/// One fast cycle is 2-3 frames. 25 ms per cycle is about 100-120 frames/s,
/// close to the rate the CatDisplay hardware has used on this radio for years.
const CYCLE_MIN: Duration = Duration::from_millis(25);

struct Rate {
    count: u32,
    since: Instant,
    value: f32,
}

impl Rate {
    fn new() -> Self {
        Self { count: 0, since: Instant::now(), value: 0.0 }
    }
    fn tick(&mut self) -> f32 {
        self.count += 1;
        let dt = self.since.elapsed().as_secs_f32();
        if dt >= 1.0 {
            self.value = self.count as f32 / dt;
            self.count = 0;
            self.since = Instant::now();
        }
        self.value
    }
}

fn sleep_unless_stopped(stop: &AtomicBool, d: Duration) {
    let end = Instant::now() + d;
    while Instant::now() < end && !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Seconds between connection attempts. Long enough to read the error.
const RETRY_SECS: u32 = 5;

fn begin_attempt(st: &mut RigState, what: &str, emit: &impl Fn(&RigState)) {
    st.attempt += 1;
    st.retry_in = None;
    st.link = Link::Connecting;
    st.message = if st.attempt > 1 { format!("{what} (attempt {})…", st.attempt) } else { format!("{what}…") };
    emit(st);
}

fn link_up(st: &mut RigState, msg: &str) {
    st.link = Link::Up;
    st.message = msg.into();
    st.error = None;
    st.attempt = 0;
    st.retry_in = None;
}

/// Show the error, then count down to the next attempt, one update per second.
fn fail_and_wait(st: &mut RigState, error: String, stop: &AtomicBool, emit: &impl Fn(&RigState)) {
    st.link = Link::Lost;
    st.error = Some(error);
    for left in (1..=RETRY_SECS).rev() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        st.retry_in = Some(left);
        st.message = format!("Next attempt in {left} s");
        emit(st);
        sleep_unless_stopped(stop, Duration::from_secs(1));
    }
    st.retry_in = None;
}

pub fn run(cfg: MonitorConfig, stop: Arc<AtomicBool>, emit: impl Fn(&RigState)) {
    match cfg.source.clone() {
        Source::Serial { port, baud } => {
            let label = format!("Serial {port} @ {baud}");
            run_cat(&cfg, &label, &stop, &emit, || {
                SerialCat::open(&port, baud).map(|p| Box::new(p) as Box<dyn CatPort>)
            })
        }
        Source::Demo => run_cat(&cfg, "Demo (simulated FT-817)", &stop, &emit, || {
            Ok(Box::new(DemoCat::new()) as Box<dyn CatPort>)
        }),
    }
    let st = RigState { link: Link::Stopped, message: "Stopped".into(), ..Default::default() };
    emit(&st);
}

// ---------------------------------------------------------------- direct CAT

fn ask(port: &mut Box<dyn CatPort>, frame: [u8; 5]) -> io::Result<Vec<u8>> {
    let n = cat::response_len(frame[4]);
    port.transact(frame, n)
}

fn ask_exact(port: &mut Box<dyn CatPort>, frame: [u8; 5]) -> io::Result<Vec<u8>> {
    let n = cat::response_len(frame[4]);
    let r = port.transact(frame, n)?;
    if r.len() != n {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("short reply ({} of {n} bytes)", r.len())));
    }
    Ok(r)
}

fn ee_byte(port: &mut Box<dyn CatPort>, addr: u16) -> io::Result<u8> {
    Ok(ask_exact(port, cat::eeprom_read(addr))?[0])
}

struct Slow {
    dig: Option<DigMode>,
    ant_split: Option<u8>,
}

fn poll_fast(port: &mut Box<dyn CatPort>, st: &mut RigState, cfg: &MonitorConfig, slow: &Slow) -> io::Result<Mode> {
    let tx = cat::decode_tx_status(ask_exact(port, cat::simple(cat::OP_READ_TX_STATUS).unwrap())?[0]);
    st.tx = tx.ptt;
    if tx.ptt {
        st.po = Some(tx.po_raw);
        st.high_swr = Some(tx.high_swr);
        if cfg.read_tx_meters {
            let r = ask(port, cat::simple(cat::OP_READ_TX_METER).unwrap())?;
            if let Some(m) = cat::decode_tx_meter(&r) {
                st.swr = Some(m.swr);
                st.alc = Some(m.alc);
                st.modulation = Some(m.modulation);
            }
        } else {
            st.swr = None;
            st.alc = None;
            st.modulation = None;
        }
    } else {
        let rx = cat::decode_rx_status(ask_exact(port, cat::simple(cat::OP_READ_RX_STATUS).unwrap())?[0]);
        st.s_raw = Some(rx.s_raw);
        st.s_label = Some(cat::s_label(rx.s_raw));
        st.squelched = Some(rx.squelched);
        st.tone_mismatch = Some(rx.tone_mismatch);
        st.discr_off = Some(rx.discr_off);
        st.po = Some(0);
        st.high_swr = Some(false);
    }

    let fm = cat::decode_freq_mode(&ask_exact(port, cat::simple(cat::OP_READ_FREQ_MODE).unwrap())?)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    st.freq_hz = Some(fm.hz);
    st.band = Some(cat::band_name(fm.hz).to_string());
    let dig = if fm.mode == Mode::Dig { slow.dig } else { None };
    st.mode = Some(cat::mode_label(fm.mode, fm.narrow, dig));
    if let Some(b) = slow.ant_split {
        st.antenna = Some(cat::decode_antenna(b, fm.hz).to_string());
    }
    Ok(fm.mode)
}

fn poll_slow(port: &mut Box<dyn CatPort>, st: &mut RigState, slow: &mut Slow, mode: Mode) -> io::Result<()> {
    st.vfo = Some(cat::decode_vfo_state(ee_byte(port, ee::VFO_STATE)?).label);
    let p = cat::decode_panel(ee_byte(port, ee::PANEL_1)?, ee_byte(port, ee::PANEL_2)?);
    st.agc = Some(p.agc);
    st.dsp = Some(p.dsp);
    st.pbt = Some(p.pbt);
    st.nb = Some(p.nb);
    st.lock = Some(p.lock);
    st.fst = Some(p.fst);
    st.meter_mode = Some(p.meter_mode);
    st.kyr = Some(p.kyr);
    st.bk = Some(p.bk);
    st.vox = Some(p.vox);
    st.tx_power = Some(cat::decode_tx_power(ee_byte(port, ee::TX_POWER)?));
    let b7a = ee_byte(port, ee::ANT_SPLIT)?;
    slow.ant_split = Some(b7a);
    st.split = Some(cat::decode_split(b7a));
    st.lcd_color = Some(cat::decode_color(ee_byte(port, ee::DISPLAY)?).to_string());
    slow.dig = if mode == Mode::Dig { DigMode::from_eeprom(ee_byte(port, ee::DIG_MODE)?) } else { None };
    Ok(())
}

fn run_cat(
    cfg: &MonitorConfig,
    label: &str,
    stop: &AtomicBool,
    emit: &impl Fn(&RigState),
    open: impl Fn() -> io::Result<Box<dyn CatPort>>,
) {
    let mut st = RigState { source: label.to_string(), ..Default::default() };
    while !stop.load(Ordering::Relaxed) {
        begin_attempt(&mut st, "Connecting", emit);
        let mut port = match open() {
            Ok(p) => p,
            Err(e) => {
                fail_and_wait(&mut st, format!("Cannot open the serial port: {e}. Check that the cable is plugged in and no other program is using the port."), stop, emit);
                continue;
            }
        };
        let mut lost: Option<String> = None;

        let mut slow = Slow { dig: None, ant_split: None };
        let mut last_slow = Instant::now() - SLOW_EVERY;
        let mut last_emit = Instant::now() - EMIT_EVERY;
        let mut rate = Rate::new();
        let mut fails = 0u32;
        let mut mode = Mode::Usb;

        while !stop.load(Ordering::Relaxed) {
            let cycle_start = Instant::now();
            let mut result = poll_fast(&mut port, &mut st, cfg, &slow).map(|m| mode = m);
            if result.is_ok() && last_slow.elapsed() >= SLOW_EVERY {
                result = poll_slow(&mut port, &mut st, &mut slow, mode);
                last_slow = Instant::now();
            }
            match result {
                Ok(()) => {
                    fails = 0;
                    if st.link != Link::Up {
                        link_up(&mut st, "Connected");
                        last_emit = Instant::now() - EMIT_EVERY;
                    }
                    st.polls_per_sec = rate.tick();
                }
                Err(e) => {
                    fails += 1;
                    st.errors += 1;
                    if fails >= 3 {
                        lost = Some(format!(
                            "No reply from the radio ({e}). Check the cable, that the radio is on, and that the baud rate matches the radio's CAT RATE menu setting."
                        ));
                        break; // reopen the port
                    }
                }
            }
            if last_emit.elapsed() >= EMIT_EVERY {
                emit(&st);
                last_emit = Instant::now();
            }
            if let Some(rest) = CYCLE_MIN.checked_sub(cycle_start.elapsed()) {
                std::thread::sleep(rest);
            }
        }
        drop(port);
        if let Some(err) = lost {
            fail_and_wait(&mut st, err, stop, emit);
        }
    }
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn demo_end_to_end_produces_decoded_state() {
        let stop = Arc::new(AtomicBool::new(false));
        let seen: Arc<Mutex<Vec<RigState>>> = Arc::default();
        let (s2, seen2) = (stop.clone(), seen.clone());
        let h = std::thread::spawn(move || {
            let cfg = MonitorConfig { source: Source::Demo, read_tx_meters: true };
            run(cfg, s2, |st| seen2.lock().unwrap().push(st.clone()));
        });
        std::thread::sleep(Duration::from_millis(1500));
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
        let v = seen.lock().unwrap();
        let up = v.iter().rev().find(|s| s.link == Link::Up).expect("link came up");
        assert_eq!(up.freq_hz, Some(14_074_000));
        assert_eq!(up.mode.as_deref(), Some("DIG USER-U"));
        assert_eq!(up.band.as_deref(), Some("20m"));
        assert_eq!(up.vfo.as_deref(), Some("VFO A"));
        assert_eq!(up.agc.as_deref(), Some("FAST"));
        assert_eq!(up.nb, Some(true));
        assert_eq!(up.lcd_color.as_deref(), Some("amber"));
        // Loose bounds: shared CI runners (macOS especially) have coarse sleep timing.
        assert!(up.polls_per_sec > 2.0 && up.polls_per_sec < 45.0, "rate {}", up.polls_per_sec);
        assert_eq!(v.last().unwrap().link, Link::Stopped);
    }

    #[test]
    fn connection_error_stays_visible_through_retries() {
        let stop = Arc::new(AtomicBool::new(false));
        let seen: Arc<Mutex<Vec<RigState>>> = Arc::default();
        let (s2, seen2) = (stop.clone(), seen.clone());
        let h = std::thread::spawn(move || {
            let cfg = MonitorConfig {
                source: Source::Serial { port: "/nonexistent/port".into(), baud: 38400 },
                read_tx_meters: false,
            };
            run(cfg, s2, |st| seen2.lock().unwrap().push(st.clone()));
        });
        std::thread::sleep(Duration::from_millis(5600)); // first wait + second attempt
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
        let v = seen.lock().unwrap();
        // Countdown 5..1 after the first failure, one update per second.
        let countdown: Vec<u32> = v.iter().filter_map(|s| s.retry_in).take(5).collect();
        assert_eq!(countdown, vec![5, 4, 3, 2, 1]);
        // During the second attempt the earlier error is still shown.
        let second = v.iter().find(|s| s.attempt == 2 && s.link == Link::Connecting).expect("second attempt");
        assert!(second.error.as_deref().unwrap_or("").contains("Cannot open the serial port"));
        assert!(second.message.contains("attempt 2"));
    }

}
