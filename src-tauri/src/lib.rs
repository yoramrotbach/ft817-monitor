//! 817 Monitor: a read-only desktop CAT monitor for the Yaesu FT-817/818.

pub use ft817_cat::{cat, sim, transport};
pub mod poller;
pub mod state;

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

/// Open the Settings window, or bring it to the front if it's already open.
/// It is a separate window so it can be sized and moved independently.
#[tauri::command]
async fn open_settings(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.unminimize();
        let _ = w.show();
        return w.set_focus().map_err(|e| e.to_string());
    }
    tauri::WebviewWindowBuilder::new(&app, "settings", tauri::WebviewUrl::App("settings.html".into()))
        .title("817 Monitor — Settings")
        .inner_size(440.0, 640.0)
        .min_inner_size(380.0, 360.0)
        .resizable(true)
        .build()
        .map(|_| ())
        .map_err(|e| e.to_string())
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
        .invoke_handler(tauri::generate_handler![list_ports, start_monitor, stop_monitor, open_settings])
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
                let app = window.app_handle();
                match window.label() {
                    // Closing the display ends the app: stop polling, close Settings.
                    "main" => {
                        app.state::<Monitor>().stop();
                        if let Some(w) = app.get_webview_window("settings") {
                            let _ = w.close();
                        }
                    }
                    // Closing Settings tells the display to apply connection changes.
                    "settings" => {
                        let _ = app.emit("settings-closed", ());
                    }
                    _ => {}
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running 817 Monitor");
}
