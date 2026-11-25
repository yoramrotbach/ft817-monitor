//! 817 EEPROM: back up and restore the EEPROM of a Yaesu FT-817/818.
//! Companion to 817 Monitor; this is the only app in the project that writes.

pub mod backupfile;
pub mod engine;

use engine::{Ctx, EeError, Plan, Progress, RestoreReport};
use ft817_cat::sim::SimRadio;
use ft817_cat::transport::{self, CatPort, DemoCat, SerialCat};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase", tag = "kind")]
pub enum Source {
    Serial { port: String, baud: u32 },
    Demo,
}

struct Pending {
    plan: Plan,
    current: Vec<u8>,
    made: Instant,
}

#[derive(Default)]
struct Loaded {
    image: Vec<u8>,
}

struct AppState {
    demo_radio: Arc<Mutex<SimRadio>>,
    last_backup_json: Mutex<Option<String>>,
    loaded: Mutex<Option<Loaded>>,
    pending: Mutex<Option<Pending>>,
    cancel: Arc<AtomicBool>,
    busy: AtomicBool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            demo_radio: Arc::new(Mutex::new(SimRadio::new_static())),
            last_backup_json: Mutex::new(None),
            loaded: Mutex::new(None),
            pending: Mutex::new(None),
            cancel: Arc::new(AtomicBool::new(false)),
            busy: AtomicBool::new(false),
        }
    }
}

struct BusyGuard<'a>(&'a AtomicBool);
impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn claim(state: &AppState) -> Result<BusyGuard<'_>, String> {
    if state.busy.swap(true, Ordering::SeqCst) {
        return Err("Another operation is still running.".into());
    }
    state.cancel.store(false, Ordering::SeqCst);
    Ok(BusyGuard(&state.busy))
}

fn open(state: &AppState, src: &Source) -> Result<Box<dyn CatPort>, String> {
    match src {
        Source::Serial { port, baud } => SerialCat::open(port, *baud)
            .map(|p| Box::new(p) as Box<dyn CatPort>)
            .map_err(|e| format!("Cannot open the serial port: {e}")),
        Source::Demo => Ok(Box::new(DemoCat::with_radio(state.demo_radio.clone()))),
    }
}

fn now_stamp() -> (String, String) {
    let t = chrono::Local::now();
    (t.format("%Y-%m-%d %H:%M:%S %z").to_string(), t.format("%Y%m%d-%H%M%S").to_string())
}

fn backups_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let d = app.path().app_data_dir().map_err(|e| e.to_string())?.join("backups");
    std::fs::create_dir_all(&d).map_err(|e| format!("Cannot create {}: {e}", d.display()))?;
    Ok(d)
}

/// Run a blocking engine job with progress events and the busy/cancel guard.
fn with_radio<T>(
    app: &AppHandle,
    state: &AppState,
    src: &Source,
    job: impl FnOnce(&mut Ctx) -> Result<T, EeError>,
) -> Result<T, String> {
    let mut port = open(state, src)?;
    let cancel = state.cancel.clone();
    let app2 = app.clone();
    let mut last = Instant::now() - Duration::from_secs(1);
    let mut progress = move |p: Progress| {
        if p.done == p.total || last.elapsed() > Duration::from_millis(80) {
            let _ = app2.emit("progress", &p);
            last = Instant::now();
        }
    };
    let mut ctx = Ctx { port: port.as_mut(), cancel: &cancel, progress: &mut progress };
    job(&mut ctx).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- commands

#[derive(Serialize)]
struct ImageInfo {
    length: usize,
    version_bytes: String,
    sha256: String,
    created: String,
    calibration: Vec<u8>,
    vcc_cal: u8,
    path: String,
    note: String,
}

fn info(img: &[u8], created: &str, path: &str, note: &str) -> ImageInfo {
    ImageInfo {
        length: img.len(),
        version_bytes: format!("{:02X} {:02X}", img[4], img[5]),
        sha256: backupfile::sha256_hex(img),
        created: created.into(),
        calibration: img[0x07..=0x52].to_vec(),
        vcc_cal: img[0x53],
        path: path.into(),
        note: note.into(),
    }
}

#[tauri::command]
fn list_ports() -> Vec<transport::PortInfo> {
    transport::list_ports()
}

#[tauri::command]
fn get_backups_dir(app: AppHandle) -> Result<String, String> {
    Ok(backups_dir(&app)?.display().to_string())
}

#[tauri::command]
fn cancel(state: State<'_, AppState>) {
    state.cancel.store(true, Ordering::SeqCst);
}

#[tauri::command]
async fn backup(app: AppHandle, source: Source) -> Result<ImageInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let _g = claim(&state)?;
        let img = with_radio(&app, &state, &source, engine::backup)?;
        let (created, stamp) = now_stamp();
        let file = backupfile::make(&img, &created, "Backup");
        let json = backupfile::to_json(&file);
        let path = backups_dir(&app)?.join(format!("FT817_EEPROM_{stamp}.817ee"));
        std::fs::write(&path, &json).map_err(|e| format!("Backup read OK but saving failed: {e}"))?;
        *state.last_backup_json.lock().unwrap() = Some(json);
        Ok(info(&img, &created, &path.display().to_string(), "Backup"))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn save_backup_as(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let json = state.last_backup_json.lock().unwrap().clone().ok_or("No backup in this session yet.")?;
    std::fs::write(&path, json).map_err(|e| format!("Cannot save {path}: {e}"))
}

#[tauri::command]
fn load_backup(state: State<'_, AppState>, path: String) -> Result<ImageInfo, String> {
    let text = std::fs::read_to_string(&path).map_err(|e| format!("Cannot read {path}: {e}"))?;
    let (img, meta) = backupfile::parse(&text)?;
    let out = info(&img, &meta.created, &path, &meta.note);
    *state.loaded.lock().unwrap() = Some(Loaded { image: img });
    *state.pending.lock().unwrap() = None;
    Ok(out)
}

#[derive(Serialize)]
struct PlanInfo {
    writes: usize,
    bytes_changed: usize,
    regions: engine::RegionCounts,
    cat_rate_kept: bool,
    pre_restore_backup: String,
    changes: Vec<engine::Write>,
}

#[tauri::command]
async fn prepare_restore(app: AppHandle, source: Source) -> Result<PlanInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let _g = claim(&state)?;
        let backup = state.loaded.lock().unwrap().as_ref().map(|l| l.image.clone()).ok_or("Open a backup file first.")?;
        // Reading the radio now doubles as the automatic pre-restore backup.
        let current = with_radio(&app, &state, &source, engine::backup)?;
        let (created, stamp) = now_stamp();
        let path = backups_dir(&app)?.join(format!("FT817_EEPROM_{stamp}_PRE-RESTORE.817ee"));
        let json = backupfile::to_json(&backupfile::make(&current, &created, "Automatic backup taken before a restore"));
        std::fs::write(&path, json).map_err(|e| format!("Could not save the pre-restore backup ({e}), so the restore won't continue."))?;
        let plan = engine::plan(&current, &backup).map_err(|e| e.to_string())?;
        let out = PlanInfo {
            writes: plan.writes.len(),
            bytes_changed: plan.bytes_changed,
            regions: plan.regions.clone(),
            cat_rate_kept: plan.cat_rate_kept,
            pre_restore_backup: path.display().to_string(),
            changes: plan.writes.iter().take(200).cloned().collect(),
        };
        *state.pending.lock().unwrap() = Some(Pending { plan, current, made: Instant::now() });
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn run_restore(app: AppHandle, source: Source) -> Result<RestoreReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let _g = claim(&state)?;
        let backup = state.loaded.lock().unwrap().as_ref().map(|l| l.image.clone()).ok_or("Open a backup file first.")?;
        let pending = state.pending.lock().unwrap().take().ok_or("Compare with the radio first.")?;
        if pending.made.elapsed() > Duration::from_secs(600) {
            return Err("The comparison is more than 10 minutes old. Compare with the radio again so the restore uses its current contents.".into());
        }
        with_radio(&app, &state, &source, |ctx| engine::restore(ctx, &pending.plan, &pending.current, &backup))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Demo only: change some calibration, settings and memory bytes on the
/// simulated radio so a restore has something to fix.
#[tauri::command]
fn demo_change_radio(state: State<'_, AppState>) -> usize {
    let mut r = state.demo_radio.lock().unwrap();
    let addrs = [0x0C, 0x21, 0x57, 0x5C, 0x79, 0x150, 0x600, 0x601, 0x1200];
    for &a in &addrs {
        r.ee[a] ^= 0x2D;
    }
    addrs.len()
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            list_ports,
            get_backups_dir,
            cancel,
            backup,
            save_backup_as,
            load_backup,
            prepare_restore,
            run_restore,
            demo_change_radio
        ])
        .run(tauri::generate_context!())
        .expect("error while running 817 EEPROM");
}
