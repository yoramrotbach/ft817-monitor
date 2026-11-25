//! Command-line EEPROM backup/restore (same engine as the app).
//!
//!   ee_cli backup  <port|demo> [baud] <out.817ee>
//!   ee_cli restore <port>      [baud] <backup.817ee> --yes
//!
//! restore always saves a PRE-RESTORE backup next to the backup file first.

use ft817_cat::transport::{CatPort, SerialCat};
use ft817_eeprom_lib::{backupfile, engine};
use std::sync::atomic::AtomicBool;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: ee_cli backup <port|demo> [baud] <out.817ee> | restore <port> [baud] <file.817ee> --yes";
    let cmd = a.first().expect(usage).as_str();
    let port_name = a.get(1).expect(usage);
    let (baud, file) = match a.get(2).and_then(|b| b.parse::<u32>().ok()) {
        Some(b) => (b, a.get(3).expect(usage).clone()),
        None => (38400, a.get(2).expect(usage).clone()),
    };
    let mut port: Box<dyn CatPort> = if port_name == "demo" {
        Box::new(ft817_cat::transport::DemoCat::with_radio(std::sync::Arc::new(std::sync::Mutex::new(
            ft817_cat::sim::SimRadio::new_static(),
        ))))
    } else {
        Box::new(SerialCat::open(port_name, baud).unwrap_or_else(|e| die(&e.to_string())))
    };
    let cancel = AtomicBool::new(false);
    let mut last = String::new();
    let mut progress = |p: engine::Progress| {
        if p.phase != last {
            eprintln!("{}", p.phase);
            last = p.phase.clone();
        }
    };
    let mut ctx = engine::Ctx { port: port.as_mut(), cancel: &cancel, progress: &mut progress };
    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S %z").to_string();
    match cmd {
        "backup" => {
            let img = engine::backup(&mut ctx).unwrap_or_else(|e| die(&e.to_string()));
            std::fs::write(&file, backupfile::to_json(&backupfile::make(&img, &stamp, "Backup (CLI)"))).unwrap();
            println!("Saved {} bytes to {file} (sha256 {})", img.len(), backupfile::sha256_hex(&img));
        }
        "restore" => {
            if !a.iter().any(|x| x == "--yes") {
                die("restore writes to the radio; add --yes to confirm");
            }
            let text = std::fs::read_to_string(&file).unwrap_or_else(|e| die(&e.to_string()));
            let (backup, _) = backupfile::parse(&text).unwrap_or_else(|e| die(&e));
            let current = engine::backup(&mut ctx).unwrap_or_else(|e| die(&e.to_string()));
            let pre = format!("{file}.PRE-RESTORE.817ee");
            std::fs::write(&pre, backupfile::to_json(&backupfile::make(&current, &stamp, "Pre-restore (CLI)"))).unwrap();
            eprintln!("Pre-restore backup saved to {pre}");
            let plan = engine::plan(&current, &backup).unwrap_or_else(|e| die(&e.to_string()));
            eprintln!("{} bytes differ ({} writes); CAT rate kept: {}", plan.bytes_changed, plan.writes.len(), plan.cat_rate_kept);
            let rep = engine::restore(&mut ctx, &plan, &current, &backup).unwrap_or_else(|e| die(&e.to_string()));
            println!("Wrote {} pairs ({} retries); mismatches after verify: {}", rep.written, rep.retried, rep.final_mismatches.len());
        }
        _ => die(usage),
    }
}

fn die(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1)
}
