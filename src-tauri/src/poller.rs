//! Polling loops. Each runs on its own thread until `stop` is set and pushes
//! RigState snapshots to `emit` (throttled to ~30 updates per second).

use crate::cat::{self, ee, DigMode, MeterMap, Mode};
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
    #[serde(default)]
    pub meter_map: MeterMap,
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
            if let Some(m) = cat::decode_tx_meter(&r, cfg.meter_map) {
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
        st.link = Link::Connecting;
        st.message = "Connecting…".into();
        emit(&st);
        let mut port = match open() {
            Ok(p) => p,
            Err(e) => {
                st.link = Link::Lost;
                st.message = format!("Cannot open port: {e}");
                emit(&st);
                sleep_unless_stopped(stop, Duration::from_secs(2));
                continue;
            }
        };

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
                        st.link = Link::Up;
                        st.message = "Connected".into();
                        last_emit = Instant::now() - EMIT_EVERY;
                    }
                    st.polls_per_sec = rate.tick();
                }
                Err(e) => {
                    fails += 1;
                    st.errors += 1;
                    if fails >= 3 {
                        st.link = Link::Lost;
                        st.message = format!(
                            "No reply from the radio ({e}). Check the cable, that the radio is on, and that the baud rate matches the radio's CAT RATE menu setting."
                        );
                        emit(&st);
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
        if st.link == Link::Lost {
            sleep_unless_stopped(stop, Duration::from_secs(1));
        }
    }
}

// ---------------------------------------------------------------- rigctld

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
        s.set_read_timeout(Some(Duration::from_millis(1500)))?;
        s.set_nodelay(true)?;
        Ok(Self { r: BufReader::new(s.try_clone()?), w: s })
    }

    /// Send a command and read `lines` reply lines. `Ok(None)` = rig/backend
    /// reported an error for this command (e.g. not supported).
    fn cmd(&mut self, c: &str, lines: usize) -> io::Result<Option<Vec<String>>> {
        self.w.write_all(format!("{c}\n").as_bytes())?;
        let mut out = Vec::with_capacity(lines);
        for _ in 0..lines {
            let mut l = String::new();
            if self.r.read_line(&mut l)? == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "rigctld closed the connection"));
            }
            let l = l.trim().to_string();
            if l.starts_with("RPRT") {
                return Ok(None);
            }
            out.push(l);
        }
        Ok(Some(out))
    }

    fn num(&mut self, c: &str) -> io::Result<Option<f64>> {
        Ok(self.cmd(c, 1)?.and_then(|v| v[0].parse::<f64>().ok()))
    }
}

fn hamlib_mode_label(m: &str) -> String {
    match m {
        "PKTUSB" => "DIG USER-U".into(),
        "PKTLSB" => "DIG USER-L".into(),
        "RTTYR" | "RTTY" => "DIG RTTY".into(),
        "PSK" => "DIG PSK-U".into(),
        "PSKR" => "DIG PSK-L".into(),
        "PKTFM" => "PKT".into(),
        other => other.into(),
    }
}

fn to_scale(v: f64) -> u8 {
    (v.clamp(0.0, 1.0) * 15.0).round() as u8
}

fn swr_to_scale(swr: f64) -> u8 {
    // 1.0 -> 0, 3.0 or worse -> 15
    (((swr - 1.0) / 2.0).clamp(0.0, 1.0) * 15.0).round() as u8
}

fn run_rigctld(host: &str, port: u16, stop: &AtomicBool, emit: &impl Fn(&RigState)) {
    let mut st = RigState { source: format!("rigctld {host}:{port}"), ..Default::default() };
    while !stop.load(Ordering::Relaxed) {
        st.link = Link::Connecting;
        st.message = "Connecting to rigctld…".into();
        emit(&st);
        let mut rc = match Rigctld::connect(host, port) {
            Ok(r) => r,
            Err(e) => {
                st.link = Link::Lost;
                st.message = format!("Cannot reach rigctld: {e}. Start it with: rigctld -m 1020 -r <port> -s 38400 (FT-817; use 1041 for FT-818)");
                emit(&st);
                sleep_unless_stopped(stop, Duration::from_secs(2));
                continue;
            }
        };
        let mut rate = Rate::new();
        let mut last_slow = Instant::now() - Duration::from_secs(10);

        let result: io::Result<()> = (|| {
            while !stop.load(Ordering::Relaxed) {
                if let Some(f) = rc.num("f")? {
                    let hz = f as u64;
                    st.freq_hz = Some(hz);
                    st.band = Some(cat::band_name(hz).into());
                }
                if let Some(v) = rc.cmd("m", 2)? {
                    st.mode = Some(hamlib_mode_label(&v[0]));
                }
                st.tx = rc.num("t")?.map(|v| v != 0.0).unwrap_or(false);
                if st.tx {
                    st.po = rc.num("l RFPOWER_METER")?.map(to_scale);
                    st.swr = rc.num("l SWR")?.map(swr_to_scale);
                    st.alc = rc.num("l ALC")?.map(to_scale);
                    st.modulation = None;
                } else {
                    if let Some(raw) = rc.num("l RAWSTR")? {
                        let s = raw.clamp(0.0, 15.0) as u8;
                        st.s_raw = Some(s);
                        st.s_label = Some(cat::s_label(s));
                    }
                    st.squelched = rc.num("\\get_dcd")?.map(|v| v == 0.0);
                    st.po = Some(0);
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
                    st.link = Link::Up;
                    st.message = "Connected (shared via rigctld)".into();
                }
                st.polls_per_sec = rate.tick();
                emit(&st);
                // Be gentle: other programs share this radio.
                std::thread::sleep(Duration::from_millis(100));
            }
            Ok(())
        })();

        if let Err(e) = result {
            st.link = Link::Lost;
            st.errors += 1;
            st.message = format!("rigctld connection lost: {e}");
            emit(&st);
            sleep_unless_stopped(stop, Duration::from_secs(2));
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
            let cfg = MonitorConfig { source: Source::Demo, read_tx_meters: true, meter_map: MeterMap::Ka7oei };
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
        assert!(up.polls_per_sec > 10.0 && up.polls_per_sec < 45.0, "rate {}", up.polls_per_sec);
        assert_eq!(v.last().unwrap().link, Link::Stopped);
    }
}
