//! Byte-level links to the radio: a real serial port or the built-in simulator.

use crate::sim::SimRadio;
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub trait CatPort: Send {
    /// Send one 5-byte frame and collect up to `expected` reply bytes.
    /// Returns whatever arrived before the timeout; errors only if nothing did.
    fn transact(&mut self, frame: [u8; 5], expected: usize) -> io::Result<Vec<u8>>;
}

// ---------------------------------------------------------------- serial

pub struct SerialCat {
    port: Box<dyn serialport::SerialPort>,
    timeout: Duration,
}

impl SerialCat {
    pub fn open(name: &str, baud: u32) -> io::Result<Self> {
        let mut port = serialport::new(name, baud)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::Two) // FT-817 CAT is 8N2
            .flow_control(serialport::FlowControl::None)
            .timeout(Duration::from_millis(20))
            .open()
            .map_err(|e| io::Error::other(format!("{name}: {e}")))?;
        // Many interfaces (Digirig, CT-62 clones, SignaLink-style boxes) key the
        // transmitter on RTS or DTR. A monitor must never transmit: drop both.
        let _ = port.write_request_to_send(false);
        let _ = port.write_data_terminal_ready(false);
        Ok(Self { port, timeout: Duration::from_millis(250) })
    }
}

impl CatPort for SerialCat {
    fn transact(&mut self, frame: [u8; 5], expected: usize) -> io::Result<Vec<u8>> {
        let _ = self.port.clear(serialport::ClearBuffer::Input);
        self.port.write_all(&frame)?;
        self.port.flush()?;

        let mut out = Vec::with_capacity(expected);
        let mut buf = [0u8; 16];
        let deadline = Instant::now() + self.timeout;
        // After the first byte, a short quiet gap ends the reply early
        // (BD answers 1 byte in receive instead of 2).
        let mut last_byte_at: Option<Instant> = None;
        while out.len() < expected && Instant::now() < deadline {
            match self.port.read(&mut buf[..expected - out.len()]) {
                Ok(0) => {}
                Ok(n) => {
                    out.extend_from_slice(&buf[..n]);
                    last_byte_at = Some(Instant::now());
                }
                Err(e) if e.kind() == io::ErrorKind::TimedOut => {
                    if let Some(t) = last_byte_at {
                        if t.elapsed() > Duration::from_millis(40) {
                            break;
                        }
                    }
                }
                Err(e) => return Err(e),
            }
        }
        if out.is_empty() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "timed out"));
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------- demo

/// A link to the built-in simulated radio. The radio is shared (Arc) so a
/// demo session can keep its EEPROM between operations.
pub struct DemoCat {
    radio: Arc<Mutex<SimRadio>>,
    delay: Duration,
}

impl DemoCat {
    pub fn new() -> Self {
        Self::with_radio(Arc::new(Mutex::new(SimRadio::new())))
    }
    pub fn with_radio(radio: Arc<Mutex<SimRadio>>) -> Self {
        // Roughly the wire time of 5 + up to 5 bytes at 38400 baud, 8N2.
        Self { radio, delay: Duration::from_millis(3) }
    }
    /// No artificial wire delay (tests).
    pub fn instant(mut self) -> Self {
        self.delay = Duration::ZERO;
        self
    }
}

impl Default for DemoCat {
    fn default() -> Self {
        Self::new()
    }
}

impl CatPort for DemoCat {
    fn transact(&mut self, frame: [u8; 5], _expected: usize) -> io::Result<Vec<u8>> {
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
        let r = self.radio.lock().unwrap().respond(&frame);
        if r.is_empty() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "timed out"));
        }
        Ok(r)
    }
}

// ---------------------------------------------------------------- ports

#[derive(serde::Serialize)]
pub struct PortInfo {
    pub name: String,
    pub description: String,
}

pub fn list_ports() -> Vec<PortInfo> {
    let mut v: Vec<PortInfo> = serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .map(|p| {
            let description = match p.port_type {
                serialport::SerialPortType::UsbPort(u) => {
                    let mut d = u.product.unwrap_or_default();
                    if let Some(m) = u.manufacturer {
                        if !d.contains(&m) {
                            d = format!("{m} {d}");
                        }
                    }
                    d.trim().to_string()
                }
                serialport::SerialPortType::BluetoothPort => "Bluetooth".into(),
                serialport::SerialPortType::PciPort => "PCI".into(),
                serialport::SerialPortType::Unknown => String::new(),
            };
            PortInfo { name: p.port_name, description }
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}
