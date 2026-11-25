//! A virtual FT-817 on a serial port, for testing without a radio.
//!
//! Linux/macOS:  socat -d -d pty,raw,echo=0,link=/tmp/radio pty,raw,echo=0,link=/tmp/app
//!               cargo run --example sim_serial -- /tmp/radio
//!               then point 817 Monitor at /tmp/app
//! Windows:      create a virtual COM pair (e.g. com0com) and pass one end here.

use ft817_monitor_lib::sim::SimRadio;
use std::io::{Read, Write};
use std::time::Duration;

fn main() {
    let path = std::env::args().nth(1).expect("usage: sim_serial <port>");
    let mut port = serialport::new(&path, 38400)
        .stop_bits(serialport::StopBits::Two)
        .timeout(Duration::from_millis(100))
        .open()
        .expect("open port");
    let mut radio = SimRadio::new();
    let mut frame = Vec::with_capacity(5);
    let mut buf = [0u8; 64];
    eprintln!("virtual FT-817 listening on {path}");
    loop {
        match port.read(&mut buf) {
            Ok(n) => {
                for &b in &buf[..n] {
                    frame.push(b);
                    if frame.len() == 5 {
                        let f: [u8; 5] = frame.clone().try_into().unwrap();
                        let reply = radio.respond(&f);
                        port.write_all(&reply).unwrap();
                        frame.clear();
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => frame.clear(), // resync on idle
            Err(e) => panic!("{e}"),
        }
    }
}
