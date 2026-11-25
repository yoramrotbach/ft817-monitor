//! 817 Monitor: a read-only desktop CAT monitor for the Yaesu FT-817/818.

pub mod cat;
pub mod poller;
pub mod sim;
pub mod state;
pub mod transport;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Default)]
struct Monitor {
    worker: Mutex<Option<(Arc<AtomicBool>, JoinHandle<()>)>>,
}

impl Monitor {
    fn stop(&self) {
        if let Some((flag, handle)) = self.worker.lock().unwrap().take() {
            flag.store(true, Ordering::Relaxed);
            let _ = handle.join();
        }
    }
}

#[tauri::command]
fn list_ports() -> Vec<transport::PortInfo> {
    transport::list_ports()
}

#[tauri::command]
async fn start_monitor(app: AppHandle, monitor: State<'_, Monitor>, config: poller::MonitorConfig) -> Result<(), String> {
    monitor.stop();
    let flag = Arc::new(AtomicBool::new(false));
    let f2 = flag.clone();
    let handle = std::thread::Builder::new()
        .name("cat-poller".into())
        .spawn(move || {
            poller::run(config, f2, |st| {
                let _ = app.emit("rig-state", st);
            })
        })
        .map_err(|e| e.to_string())?;
    *monitor.worker.lock().unwrap() = Some((flag, handle));
    Ok(())
}

#[tauri::command]
async fn stop_monitor(monitor: State<'_, Monitor>) -> Result<(), String> {
    monitor.stop();
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .manage(Monitor::default())
        .invoke_handler(tauri::generate_handler![list_ports, start_monitor, stop_monitor])
        .setup(|app| {
            // `--demo` starts the simulated radio at launch (handy for a first look).
            if std::env::args().any(|a| a == "--demo") {
                let handle = app.handle().clone();
                let cfg = poller::MonitorConfig {
                    source: poller::Source::Demo,
                    read_tx_meters: true,
                    meter_map: cat::MeterMap::Ka7oei,
                };
                tauri::async_runtime::spawn(async move {
                    // Give the page a moment to subscribe to events.
                    std::thread::sleep(std::time::Duration::from_millis(800));
                    let state = handle.state::<Monitor>();
                    let _ = start_monitor(handle.clone(), state, cfg).await;
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                window.app_handle().state::<Monitor>().stop();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running 817 Monitor");
}
