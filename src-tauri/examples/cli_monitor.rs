//! Headless monitor: prints one JSON line per update. Useful for debugging a
//! cable or running on a box with no display.
//!
//!   cargo run --example cli_monitor -- serial /dev/ttyUSB0 38400 [--bd]
//!   cargo run --example cli_monitor -- rigctld 127.0.0.1 4532
//!   cargo run --example cli_monitor -- demo

use ft817_monitor_lib::cat::MeterMap;
use ft817_monitor_lib::poller::{run, MonitorConfig, Source};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let source = match a.first().map(String::as_str) {
        Some("serial") => Source::Serial {
            port: a.get(1).cloned().expect("port"),
            baud: a.get(2).and_then(|b| b.parse().ok()).unwrap_or(38400),
        },
        Some("rigctld") => Source::Rigctld {
            host: a.get(1).cloned().unwrap_or_else(|| "127.0.0.1".into()),
            port: a.get(2).and_then(|p| p.parse().ok()).unwrap_or(4532),
        },
        _ => Source::Demo,
    };
    let cfg = MonitorConfig { source, read_tx_meters: a.iter().any(|x| x == "--bd"), meter_map: MeterMap::Ka7oei };
    run(cfg, Arc::new(AtomicBool::new(false)), |st| {
        println!("{}", serde_json::to_string(st).unwrap());
    });
}
