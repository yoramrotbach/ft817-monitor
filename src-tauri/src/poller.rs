//! Polling loops. Each runs on its own thread until `stop` is set and pushes
//! RigState snapshots to `emit` (throttled to ~30 updates per second).

use crate::cat::{self, ee, DigMode, Mode};
use crate::state::{Link, RigState};
use crate::transport::{CatPort, DemoCat, SerialCat};
use serde::Deserialize;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase", tag = "kind")]
pub enum Source {
    Serial { port: String, baud: u32 },
    Rigctld { host: String, port: u16 },
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
    fail_and_wait_for(st, error, RETRY_SECS, stop, emit)
}

fn fail_and_wait_for(st: &mut RigState, error: String, secs: u32, stop: &AtomicBool, emit: &impl Fn(&RigState)) {
    st.link = Link::Lost;
    st.error = Some(error);
    for left in (1..=secs).rev() {
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
        Source::Rigctld { host, port } => run_rigctld(&host, port, &stop, &emit),
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

// ---------------------------------------------------------------- rigctld

const RIGCTLD_REPLY_TIMEOUT: Duration = Duration::from_secs(10);
/// After rigctld stops answering, wait this long before reconnecting so it
/// can finish the requests it's still working on.
const RIGCTLD_RECOVER_SECS: u32 = 15;

struct Rigctld {
    r: BufReader<TcpStream>,
    w: TcpStream,
}

impl Rigctld {
    fn connect(host: &str, port: u16) -> io::Result<Self> {
        let addr = (host, port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::other("cannot resolve host"))?;
        let s = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
        // Hamlib's FT-817 driver retries a missed reply for several seconds;
        // give it time rather than abandoning a busy rigctld.
        s.set_read_timeout(Some(RIGCTLD_REPLY_TIMEOUT))?;
        s.set_nodelay(true)?;
        Ok(Self { r: BufReader::new(s.try_clone()?), w: s })
    }

    /// Send a command and read `lines` reply lines. `Ok(None)` = rig/backend
    /// reported an error for this command (e.g. not supported).
    fn cmd(&mut self, c: &str, lines: usize) -> io::Result<Option<Vec<String>>> {
        self.w.write_all(format!("{c}\n").as_bytes())?;
        self.read_reply(lines)
    }

    /// Send several commands in one write, then read each reply in order.
    /// Saves a network round trip per command. Each entry: (command, reply lines).
    fn batch(&mut self, cmds: &[(&str, usize)]) -> io::Result<Vec<Option<Vec<String>>>> {
        let mut req = String::new();
        for (c, _) in cmds {
            req.push_str(c);
            req.push('\n');
        }
        self.w.write_all(req.as_bytes())?;
        let mut replies = Vec::with_capacity(cmds.len());
        for &(_, lines) in cmds {
            replies.push(self.read_reply(lines)?);
        }
        Ok(replies)
    }

    fn read_reply(&mut self, lines: usize) -> io::Result<Option<Vec<String>>> {
        let mut out = Vec::with_capacity(lines);
        for _ in 0..lines {
            let mut l = String::new();
            if self.r.read_line(&mut l)? == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "rigctld closed the connection"));
            }
            let l = l.trim().to_string();
            if l.starts_with("RPRT") {
                return Ok(None); // error reply is a single line
            }
            out.push(l);
        }
        Ok(Some(out))
    }
}

fn reply_num(r: &Option<Vec<String>>) -> Option<f64> {
    r.as_ref().and_then(|v| v.first()).and_then(|x| x.parse::<f64>().ok())
}

// Hamlib's FT-817 driver converts the radio's raw 0-15 meter values with
// these tables (rigs/yaesu/ft817.c). Reversing them gives back the raw
// value, so rigctld bars use the same scale as direct mode.
const HL_PWR_W: [f64; 9] = [0.0, 0.5, 0.75, 1.0, 1.7, 2.5, 3.3, 4.1, 5.0];
const HL_SWR: [f64; 11] = [1.0, 1.4, 1.8, 2.13, 2.25, 3.7, 6.0, 7.0, 8.0, 9.0, 10.0];

/// Nearest raw index for a value from a monotonic Hamlib calibration table.
fn raw_from_table(v: f64, table: &[f64]) -> u8 {
    let mut best = 0usize;
    for (i, t) in table.iter().enumerate() {
        if (v - t).abs() < (v - table[best]).abs() {
            best = i;
        }
    }
    best as u8
}

/// Hamlib ALC table: raw 0-5 -> 0, 20, ... 100. Raw values above 5 read as 100.
fn raw_alc(v: f64) -> u8 {
    (v / 20.0).round().clamp(0.0, 5.0) as u8
}

/// Hamlib's normal filter width per mode (ft817.c filter list). A narrower
/// width reported by `m` means the radio's narrow filter is on.
fn hamlib_normal_width(mode: &str) -> i64 {
    match mode {
        "AM" => 6000,
        "FM" | "PKTFM" | "" | "None" | "NONE" => 9000,
        "WFM" => 15000,
        _ => 2200,
    }
}

/// Map Hamlib's mode name and passband to the same label direct mode shows.
fn hamlib_mode_label(m: &str, width_hz: Option<i64>) -> String {
    use cat::{DigMode as D, Mode as M};
    let (mode, dig) = match m {
        "LSB" => (M::Lsb, None),
        "USB" => (M::Usb, None),
        "CW" => (M::Cw, None),
        "CWR" => (M::Cwr, None),
        "AM" => (M::Am, None),
        "FM" => (M::Fm, None),
        "WFM" => (M::Wfm, None),
        // Hamlib doesn't recognize the FT-817ND's PKT code (0xFC) and reports
        // no mode; on this radio that only happens in PKT.
        "PKTFM" | "" | "None" | "NONE" => (M::Pkt, None),
        "PKTUSB" => (M::Dig, Some(D::UserU)),
        "PKTLSB" => (M::Dig, Some(D::UserL)),
        "RTTYR" | "RTTY" => (M::Dig, Some(D::Rtty)),
        "PSK" => (M::Dig, Some(D::PskU)),
        "PSKR" => (M::Dig, Some(D::PskL)),
        other => return other.to_string(),
    };
    let narrow = matches!(width_hz, Some(w) if w > 0 && w < hamlib_normal_width(m));
    cat::mode_label(mode, narrow, dig)
}

fn run_rigctld(host: &str, port: u16, stop: &AtomicBool, emit: &impl Fn(&RigState)) {
    let mut st = RigState { source: format!("rigctld {host}:{port}"), ..Default::default() };
    while !stop.load(Ordering::Relaxed) {
        begin_attempt(&mut st, "Connecting to rigctld", emit);
        let mut rc = match Rigctld::connect(host, port) {
            Ok(r) => r,
            Err(e) => {
                fail_and_wait(&mut st, format!("Cannot reach rigctld: {e}. Start it with: rigctld -m 1020 -r <port> -s 38400 (FT-817; use 1041 for FT-818)"), stop, emit);
                continue;
            }
        };
        let mut rate = Rate::new();
        let mut last_slow = Instant::now() - Duration::from_secs(10);

        let result: io::Result<()> = (|| {
            let mut last_mode = Instant::now() - Duration::from_secs(10);
            let mut pwr_cmd = "l RFPOWER_METER_WATTS";
            while !stop.load(Ordering::Relaxed) {
                // Every cycle: frequency and PTT, sent together.
                let r = rc.batch(&[("f", 1), ("t", 1)])?;
                if let Some(f) = reply_num(&r[0]) {
                    let hz = f as u64;
                    st.freq_hz = Some(hz);
                    st.band = Some(cat::band_name(hz).into());
                }
                st.tx = reply_num(&r[1]).map(|v| v != 0.0).unwrap_or(false);
                if st.tx {
                    let r = rc.batch(&[(pwr_cmd, 1), ("l SWR", 1), ("l ALC", 1)])?;
                    if r[0].is_none() && pwr_cmd == "l RFPOWER_METER_WATTS" {
                        pwr_cmd = "l RFPOWER"; // Hamlib before 4.6; same table in the FT-817 driver
                    }
                    st.po = reply_num(&r[0]).map(|w| raw_from_table(w, &HL_PWR_W));
                    st.swr = reply_num(&r[1]).map(|v| raw_from_table(v, &HL_SWR));
                    st.alc = reply_num(&r[2]).map(raw_alc);
                    st.modulation = None; // Hamlib doesn't report MOD
                } else {
                    let r = rc.batch(&[("l RAWSTR", 1), ("\\get_dcd", 1)])?;
                    if let Some(raw) = reply_num(&r[0]) {
                        let s = raw.clamp(0.0, 15.0) as u8;
                        st.s_raw = Some(s);
                        st.s_label = Some(cat::s_label(s));
                    }
                    st.squelched = reply_num(&r[1]).map(|v| v == 0.0);
                    st.po = Some(0);
                    st.swr = None;
                    st.alc = None;
                }
                // About 3 times a second: mode and filter width.
                if last_mode.elapsed() >= Duration::from_millis(330) {
                    if let Some(v) = rc.cmd("m", 2)? {
                        let width = v.get(1).and_then(|w| w.parse::<i64>().ok());
                        st.mode = Some(hamlib_mode_label(&v[0], width));
                    }
                    last_mode = Instant::now();
                }
                if last_slow.elapsed() >= Duration::from_secs(1) {
                    if let Some(v) = rc.cmd("v", 1)? {
                        st.vfo = Some(match v[0].as_str() {
                            "VFOA" => "VFO A".into(),
                            "VFOB" => "VFO B".into(),
                            "MEM" => "MEM".into(),
                            o => o.into(),
                        });
                    }
                    if let Some(v) = rc.cmd("s", 2)? {
                        st.split = Some(v[0] == "1");
                    }
                    last_slow = Instant::now();
                }
                if st.link != Link::Up {
                    link_up(&mut st, "Connected (shared via rigctld)");
                }
                st.polls_per_sec = rate.tick();
                emit(&st);
                // Gentle pace: the radio and other programs share rigctld.
                std::thread::sleep(Duration::from_millis(100));
            }
            Ok(())
        })();

        if let Err(e) = result {
            st.errors += 1;
            if matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) {
                drop(rc); // close this connection; rigctld discards its pending requests
                fail_and_wait_for(
                    &mut st,
                    format!("rigctld didn't answer within {} s. The radio may have missed a request; giving rigctld time to recover.", RIGCTLD_REPLY_TIMEOUT.as_secs()),
                    RIGCTLD_RECOVER_SECS,
                    stop,
                    emit,
                );
            } else {
                fail_and_wait(&mut st, format!("rigctld connection lost: {e}"), stop, emit);
            }
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

    #[test]
    fn hamlib_meter_values_map_back_to_raw() {
        assert_eq!(raw_from_table(0.0, &HL_PWR_W), 0);
        assert_eq!(raw_from_table(2.5, &HL_PWR_W), 5);
        assert_eq!(raw_from_table(5.0, &HL_PWR_W), 8);
        assert_eq!(raw_from_table(1.0, &HL_SWR), 0);
        assert_eq!(raw_from_table(3.7, &HL_SWR), 5);
        assert_eq!(raw_from_table(10.0, &HL_SWR), 10);
        assert_eq!(raw_alc(0.0), 0);
        assert_eq!(raw_alc(60.0), 3);
        assert_eq!(raw_alc(100.0), 5);
    }

    #[test]
    fn hamlib_modes_and_narrow() {
        assert_eq!(hamlib_mode_label("USB", Some(2200)), "USB");
        assert_eq!(hamlib_mode_label("CW", Some(500)), "CW-N");
        assert_eq!(hamlib_mode_label("FM", Some(9000)), "FM");
        assert_eq!(hamlib_mode_label("FM", Some(6250)), "FM-N");
        assert_eq!(hamlib_mode_label("PKTUSB", Some(2200)), "DIG USER-U");
        assert_eq!(hamlib_mode_label("RTTYR", Some(500)), "DIG-N RTTY");
        // FT-817ND PKT: Hamlib reports no mode.
        assert_eq!(hamlib_mode_label("", Some(9000)), "PKT");
        assert_eq!(hamlib_mode_label("None", None), "PKT");
        assert_eq!(hamlib_mode_label("PKTFM", Some(9000)), "PKT");
    }

    #[test]
    fn rigctld_batch_reads_replies_in_order() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let mut w = s.try_clone().unwrap();
            for line in BufReader::new(s).lines() {
                let reply = match line.unwrap().as_str() {
                    "f" => "14074000\n",
                    "t" => "0\n",
                    "m" => "USB\n2200\n",
                    _ => "RPRT -11\n",
                };
                w.write_all(reply.as_bytes()).unwrap();
            }
        });
        let mut rc = Rigctld::connect("127.0.0.1", port).unwrap();
        let r = rc.batch(&[("f", 1), ("bogus", 1), ("m", 2), ("t", 1)]).unwrap();
        assert_eq!(reply_num(&r[0]), Some(14074000.0));
        assert!(r[1].is_none());
        assert_eq!(r[2].as_ref().unwrap(), &vec!["USB".to_string(), "2200".to_string()]);
        assert_eq!(reply_num(&r[3]), Some(0.0));
    }


    #[test]
    fn slow_rigctld_reply_is_not_a_lost_connection() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let mut w = s.try_clone().unwrap();
            for _ in BufReader::new(s).lines() {
                // Hamlib retrying a missed reply from the radio.
                std::thread::sleep(Duration::from_secs(3));
                w.write_all(b"14074000\n").unwrap();
            }
        });
        let mut rc = Rigctld::connect("127.0.0.1", port).unwrap();
        let r = rc.batch(&[("f", 1)]).unwrap();
        assert_eq!(reply_num(&r[0]), Some(14074000.0));
    }

}
