//! Tauri shell around the BoothReady engine.
//!
//! Commands are thin: they look up state, run engine code on a blocking
//! thread, and return plain data. Long operations stream progress as events
//! so the UI stays responsive (PRD §70).

mod dto;
mod elevate;

use boothready_core::demo::DemoContent;
use boothready_core::drive::{analyze_drive, DriveReport};
use boothready_core::identify::{apply_user_choice, Candidate, Identification, UsbCatalog};
use boothready_core::library::scan_libraries;
use boothready_core::manifest::{now_unix, read_manifest, write_manifest, Manifest, PreparationState, VerifyMode};
use boothready_core::planner::{plan_kit, DriveCandidate, KitPlan, KitRequest, Role};
use boothready_core::privileged::{DeviceFingerprint, HelperEvent, HelperRequest};
use boothready_core::rules::{assess_drive, DriveAssessment, Preset, Ruleset};
use boothready_core::store::{media_key, GigProfile, KnownMedia, Store};
use boothready_core::verify::{verify_volume, Expectations, VerifyReport, VerifyRequest};
use boothready_helper::{Backend, DemoBackend, NativeBackend};
use boothready_model::{DeviceEvent, FilesystemKind, PartitionScheme, PhysicalDevice};
use boothready_platform::{Platform, PlatformError, Watcher};
use dto::*;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Clone)]
enum Mode {
    Real,
    Demo(PathBuf),
}

struct Inner {
    mode: Mode,
    platform: Arc<dyn Platform>,
    watcher: Option<Watcher>,
    reports: HashMap<String, DriveReport>,
    identities: HashMap<String, Identification>,
    store: Option<Store>,
}

pub struct AppState {
    inner: Mutex<Inner>,
    rules: Ruleset,
    catalog: UsbCatalog,
    cancel_verify: Arc<AtomicBool>,
}

type Res<T> = Result<T, String>;

fn e2s<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

impl AppState {
    fn platform(&self) -> Arc<dyn Platform> {
        self.inner.lock().unwrap().platform.clone()
    }

    fn backend(&self) -> Box<dyn Backend> {
        match &self.inner.lock().unwrap().mode {
            Mode::Demo(root) => Box::new(DemoBackend::new(root.join("usb"))),
            Mode::Real => Box::new(NativeBackend::new()),
        }
    }

    fn demo_root(&self) -> Option<PathBuf> {
        match &self.inner.lock().unwrap().mode {
            Mode::Demo(root) => Some(root.clone()),
            Mode::Real => None,
        }
    }

    fn device(&self, id: &str) -> Res<PhysicalDevice> {
        self.platform()
            .list_devices()
            .map_err(e2s)?
            .into_iter()
            .find(|d| d.id == id)
            .ok_or_else(|| "That USB drive is no longer connected.".to_string())
    }

    fn report(&self, id: &str) -> Res<DriveReport> {
        self.inner.lock().unwrap().reports.get(id).cloned().ok_or_else(|| "Scan the drive first.".to_string())
    }
}

fn start_watcher(app: &AppHandle, platform: Arc<dyn Platform>) -> Watcher {
    let handle = app.clone();
    Watcher::spawn(platform, Duration::from_millis(1000), move |ev| {
        if let DeviceEvent::Disappeared { id } = &ev {
            if let Some(state) = handle.try_state::<AppState>() {
                state.inner.lock().unwrap().reports.remove(id);
            }
        }
        let _ = handle.emit("device-event", &ev);
    })
}

fn set_mode(app: &AppHandle, state: &AppState, mode: Mode) -> Res<()> {
    let platform: Arc<dyn Platform> = match &mode {
        Mode::Real => Arc::from(boothready_platform::native()),
        Mode::Demo(root) => {
            let available = root.join("available");
            if !available.exists() {
                init_demo(root).map_err(e2s)?;
            }
            Arc::new(boothready_platform::demo::DemoPlatform::new(root.join("usb")))
        }
    };
    let mut inner = state.inner.lock().unwrap();
    if let Some(w) = inner.watcher.take() {
        w.stop();
    }
    inner.reports.clear();
    inner.mode = mode;
    inner.platform = platform.clone();
    inner.watcher = Some(start_watcher(app, platform));
    Ok(())
}

// ---------------------------------------------------------------- app info

#[derive(Serialize)]
struct AppInfo {
    version: String,
    demo: bool,
    platform: String,
    rules_version: u64,
    rules_review_status: String,
    rules_review_note: Option<String>,
}

#[tauri::command]
fn app_info(state: State<'_, AppState>) -> AppInfo {
    let inner = state.inner.lock().unwrap();
    AppInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        demo: matches!(inner.mode, Mode::Demo(_)),
        platform: inner.platform.name().into(),
        rules_version: state.rules.version,
        rules_review_status: state.rules.review_status.clone(),
        rules_review_note: state.rules.review_note.clone(),
    }
}

#[tauri::command]
fn set_demo_mode(app: AppHandle, state: State<'_, AppState>, enabled: bool) -> Res<()> {
    let mode = if enabled {
        let root = app.path().app_data_dir().map_err(e2s)?.join("demo");
        Mode::Demo(root)
    } else {
        Mode::Real
    };
    set_mode(&app, &state, mode)
}

// ---------------------------------------------------------------- devices

#[tauri::command]
async fn list_devices(state: State<'_, AppState>) -> Res<Vec<DeviceCard>> {
    let platform = state.platform();
    let devices =
        tauri::async_runtime::spawn_blocking(move || platform.list_devices()).await.map_err(e2s)?.map_err(e2s)?;
    let inner = state.inner.lock().unwrap();
    Ok(devices
        .into_iter()
        .map(|d| {
            let key = media_key(&d);
            let known = inner.store.as_ref().and_then(|s| s.media(&key).ok().flatten());
            let identification = inner
                .identities
                .get(&key)
                .cloned()
                .unwrap_or_else(|| boothready_core::identify::identify(&d, &state.catalog));
            DeviceCard::new(d, identification, key, known)
        })
        .collect())
}

#[tauri::command]
async fn analyze(app: AppHandle, state: State<'_, AppState>, device_id: String) -> Res<DriveSummary> {
    let dev = state.device(&device_id)?;
    let backend = state.backend();
    let rules = state.rules.clone();
    let catalog = state.catalog.clone();
    let key = media_key(&dev);
    let confirmed = state.inner.lock().unwrap().identities.get(&key).cloned();
    let id2 = device_id.clone();
    let report = tauri::async_runtime::spawn_blocking(move || {
        // Raw inspection works in demo mode and when we happen to have
        // read access; otherwise the OS-reported layout is used.
        let raw = backend.open_read(&dev).ok().and_then(|f| boothready_core::media::inspect(f).ok());
        let mut last = std::time::Instant::now();
        analyze_drive(&dev, raw, &rules, &catalog, |p, _| {
            if last.elapsed() > Duration::from_millis(150) {
                let _ = app.emit(
                    "scan-progress",
                    ScanProgressEvent {
                        device_id: id2.clone(),
                        files: p.files_done,
                        bytes: p.bytes_done,
                        current: p.current.clone(),
                    },
                );
                last = std::time::Instant::now();
            }
        })
    })
    .await
    .map_err(e2s)?;
    let mut report = report;
    if let Some(c) = confirmed {
        report.identification = c;
    }
    let summary = DriveSummary::from_report(&report);
    let mut inner = state.inner.lock().unwrap();
    if let Some(store) = &inner.store {
        let _ = store.media_seen(&key, &report.identification.display_name, now_unix());
    }
    inner.reports.insert(device_id, report);
    Ok(summary)
}

#[tauri::command]
fn identity_candidates(state: State<'_, AppState>, device_id: String, query: Option<String>) -> Res<Vec<Candidate>> {
    let report = state.report(&device_id)?;
    let gb = report.identification.marketed_gb;
    let mut out: Vec<Candidate> = report.identification.candidates.clone();
    let extra: Vec<Candidate> = match query.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        Some(q) => state.catalog.search(q).into_iter().map(|p| dto::candidate(p, gb)).collect(),
        None => state
            .catalog
            .products
            .iter()
            .filter(|p| gb.is_some_and(|g| p.capacities_gb.contains(&g)))
            .map(|p| dto::candidate(p, gb))
            .collect(),
    };
    for c in extra {
        if !out.iter().any(|o| o.catalog_id == c.catalog_id)
            && Some(&c.catalog_id) != report.identification.catalog_id.as_ref()
        {
            out.push(c);
        }
    }
    Ok(out)
}

#[tauri::command]
fn confirm_identity(state: State<'_, AppState>, device_id: String, catalog_id: String) -> Res<Identification> {
    let product = state.catalog.product(&catalog_id).ok_or("unknown product")?.clone();
    let mut inner = state.inner.lock().unwrap();
    let report = inner.reports.get_mut(&device_id).ok_or("Scan the drive first.")?;
    let id = apply_user_choice(report.identification.clone(), &product);
    report.identification = id.clone();
    let key = media_key(&report.device);
    if let Some(store) = &inner.store {
        let _ = store.media_seen(&key, &id.display_name, now_unix());
        let _ = store.confirm_identity(&key, &catalog_id, &id.display_name);
    }
    inner.identities.insert(key, id.clone());
    Ok(id)
}

// ---------------------------------------------------------------- rules

#[tauri::command]
fn presets(state: State<'_, AppState>) -> Vec<Preset> {
    state.rules.presets.clone()
}

#[tauri::command]
fn hardware(state: State<'_, AppState>, query: Option<String>) -> Vec<HardwareCard> {
    let list = match query.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        Some(q) => state.rules.search(q),
        None => state.rules.devices.iter().collect(),
    };
    list.into_iter().map(HardwareCard::from).collect()
}

#[tauri::command]
fn assess(state: State<'_, AppState>, device_id: String, targets: Vec<String>) -> Res<DriveAssessment> {
    let report = state.report(&device_id)?;
    let devs = state.rules.devices_by_id(&targets);
    if devs.is_empty() {
        return Err("Pick at least one piece of equipment.".into());
    }
    Ok(assess_drive(&report.facts, &devs, &state.rules))
}

#[tauri::command]
async fn plan(
    state: State<'_, AppState>,
    targets: Vec<String>,
    redundancy: bool,
    device_id: Option<String>,
    essential_gb: Option<f64>,
) -> Res<KitPlan> {
    let platform = state.platform();
    let devices =
        tauri::async_runtime::spawn_blocking(move || platform.list_devices()).await.map_err(e2s)?.map_err(e2s)?;
    let library_bytes = device_id
        .as_ref()
        .and_then(|id| state.inner.lock().unwrap().reports.get(id).map(|r| r.content.used_bytes))
        .unwrap_or(0);
    let drives: Vec<DriveCandidate> = devices
        .iter()
        .filter(|d| boothready_core::privileged::eligibility(d).eligible)
        .map(|d| {
            let id = boothready_core::identify::identify(d, &state.catalog);
            DriveCandidate {
                id: d.id.clone(),
                name: id.display_name,
                vendor: id.manufacturer,
                capacity_bytes: d.size_bytes,
            }
        })
        .collect();
    let req = KitRequest {
        targets,
        redundancy,
        library_bytes,
        essential_bytes: essential_gb.map(|g| (g * 1e9) as u64),
        drives,
    };
    Ok(plan_kit(&req, &state.rules))
}

// ---------------------------------------------------------------- prepare

#[tauri::command]
fn confirmation(state: State<'_, AppState>, device_id: String) -> Res<ConfirmationDetails> {
    let report = state.report(&device_id)?;
    let dev = state.device(&device_id)?;
    let fp = DeviceFingerprint::of(&dev);
    if fp != report.fingerprint {
        return Err("This drive changed since it was scanned. Scan it again before erasing.".into());
    }
    Ok(ConfirmationDetails::new(&report, &fp))
}

#[tauri::command]
async fn prepare(
    app: AppHandle,
    state: State<'_, AppState>,
    device_id: String,
    token: String,
    filesystem: FilesystemKind,
    role: Option<Role>,
    targets: Vec<String>,
) -> Res<String> {
    let report = state.report(&device_id)?;
    let label = role.map(|r| r.volume_label().to_string()).unwrap_or_else(|| "BOOTHREADY".into());
    let req = HelperRequest::Prepare {
        expected: report.fingerprint.clone(),
        confirmation: token,
        scheme: PartitionScheme::Mbr,
        filesystem,
        label,
    };
    let demo = state.demo_root();
    let app2 = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || -> Res<String> {
        let mut out: Res<String> = Err("The helper didn't report a result.".into());
        let mut emit = |e: HelperEvent| match e {
            HelperEvent::Progress { step, detail } => {
                let _ = app2.emit("prepare-progress", PrepareProgress { step, detail });
            }
            HelperEvent::Done { detail } => out = Ok(detail),
            HelperEvent::Error { message, .. } => out = Err(message),
            _ => {}
        };
        match demo {
            Some(root) => boothready_helper::handle(&DemoBackend::new(root.join("usb")), req, &mut emit),
            None => elevate::run_helper(&req, &mut emit).map_err(|e| format!("Couldn't start the disk helper: {e}"))?,
        }
        out
    })
    .await
    .map_err(e2s)??;

    // Record the role on the fresh drive once the OS remounts it.
    let platform = state.platform();
    for _ in 0..40 {
        if let Some(root) = platform
            .list_devices()
            .ok()
            .and_then(|ds| ds.into_iter().find(|d| d.id == device_id))
            .and_then(|d| d.primary_mount().and_then(|v| v.mount_point.clone()))
        {
            let mut m = Manifest::new(role, targets.clone());
            m.scheme = Some(PartitionScheme::Mbr);
            m.filesystem = Some(filesystem);
            m.preparation = PreparationState::InProgress { step: "awaiting_export".into(), started_unix: now_unix() };
            let _ = write_manifest(&root, &m);
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    state.inner.lock().unwrap().reports.remove(&device_id);
    Ok(result)
}

#[tauri::command]
fn remove_apple_double(state: State<'_, AppState>, device_id: String) -> Res<u32> {
    let report = state.report(&device_id)?;
    let root = report.mount_point.ok_or("The drive isn't mounted.")?;
    let mut removed = 0;
    for e in boothready_core::library::apple_double_files(Path::new(&root)) {
        if std::fs::remove_file(&e).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

/// Cheap check for a finished export: modification times of the library
/// databases, without rescanning every track.
#[tauri::command]
fn export_status(state: State<'_, AppState>, device_id: String) -> Res<ExportStatus> {
    let dev = state.device(&device_id)?;
    let root = dev.primary_mount().and_then(|v| v.mount_point.clone()).ok_or("The drive isn't mounted.")?;
    let mut resolver = boothready_core::library::PathResolver::new(&root);
    let mtime = |r: &mut boothready_core::library::PathResolver, rel: &str| {
        r.resolve(rel)
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
    };
    Ok(ExportStatus {
        device_library: mtime(&mut resolver, boothready_core::library::DEVICE_LIBRARY_FILE),
        one_library: mtime(&mut resolver, boothready_core::library::ONE_LIBRARY_FILE),
        engine: mtime(&mut resolver, boothready_core::library::ENGINE_DB_FILE),
    })
}

#[tauri::command]
fn open_rekordbox() -> Res<()> {
    elevate::open_rekordbox().map_err(e2s)
}

// ---------------------------------------------------------------- verify / eject

#[tauri::command]
async fn verify(app: AppHandle, state: State<'_, AppState>, device_id: String, full: bool) -> Res<VerifyReport> {
    let dev = state.device(&device_id)?;
    let root = dev.primary_mount().and_then(|v| v.mount_point.clone()).ok_or("The drive isn't mounted.")?;
    let backend = state.backend();
    let cancel = state.cancel_verify.clone();
    cancel.store(false, Ordering::Relaxed);
    let id2 = device_id.clone();
    let report = tauri::async_runtime::spawn_blocking(move || {
        let layout = backend.open_read(&dev).ok().and_then(|f| boothready_core::media::inspect(f).ok());
        let libs = scan_libraries(&root);
        let manifest = read_manifest(&root);
        let expect = Expectations {
            scheme: manifest.as_ref().and_then(|m| m.scheme),
            filesystem: manifest.as_ref().and_then(|m| m.filesystem),
        };
        let mode = if full { VerifyMode::Full } else { VerifyMode::Quick };
        let req = VerifyRequest {
            root: &root,
            layout: layout.as_ref(),
            expect,
            libs: &libs,
            manifest: manifest.as_ref(),
            mode,
        };
        let report = verify_volume(&req, &cancel, |p| {
            let _ = app.emit("verify-progress", VerifyProgressEvent { device_id: id2.clone(), progress: p.clone() });
        });
        if !report.cancelled {
            let mut m = manifest.unwrap_or_else(|| Manifest::new(None, vec![]));
            if report.passed {
                m.preparation = PreparationState::Complete { finished_unix: report.completed_unix };
            }
            m.verification = Some(report.record());
            let _ = write_manifest(&root, &m);
        }
        report
    })
    .await
    .map_err(e2s)?;
    Ok(report)
}

#[tauri::command]
fn cancel_verify(state: State<'_, AppState>) {
    state.cancel_verify.store(true, Ordering::Relaxed);
}

#[tauri::command]
async fn eject(state: State<'_, AppState>, device_id: String) -> Res<EjectResult> {
    let platform = state.platform();
    let r = tauri::async_runtime::spawn_blocking(move || platform.eject(&device_id)).await.map_err(e2s)?;
    Ok(match r {
        Ok(()) => EjectResult { ok: true, busy_holder: None, message: "Safe to remove.".into() },
        Err(PlatformError::Busy { holder }) => EjectResult {
            ok: false,
            message: match &holder {
                Some(h) => format!("{h} is using this USB. Close it, then eject again."),
                None => "Another application is using this USB. Close it, then eject again.".into(),
            },
            busy_holder: holder,
        },
        Err(e) => EjectResult { ok: false, busy_holder: None, message: e.to_string() },
    })
}

// ---------------------------------------------------------------- store

#[tauri::command]
fn known_media(state: State<'_, AppState>) -> Vec<KnownMedia> {
    state.inner.lock().unwrap().store.as_ref().and_then(|s| s.known_media().ok()).unwrap_or_default()
}

#[tauri::command]
fn profiles(state: State<'_, AppState>) -> Vec<GigProfile> {
    state.inner.lock().unwrap().store.as_ref().and_then(|s| s.profiles().ok()).unwrap_or_default()
}

#[tauri::command]
fn save_profile(
    state: State<'_, AppState>,
    name: String,
    preset: Option<String>,
    targets: Vec<String>,
    redundancy: bool,
) -> Res<GigProfile> {
    let p = GigProfile {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        preset,
        targets,
        playlists: vec![],
        redundancy,
        updated_unix: now_unix(),
    };
    let inner = state.inner.lock().unwrap();
    inner.store.as_ref().ok_or("storage unavailable")?.save_profile(&p).map_err(e2s)?;
    Ok(p)
}

// ---------------------------------------------------------------- demo

fn init_demo(root: &Path) -> std::io::Result<()> {
    let available = root.join("available");
    std::fs::create_dir_all(&available)?;
    std::fs::create_dir_all(root.join("usb"))?;
    for s in boothready_core::demo::STICKS {
        if !available.join(s.name).exists() && !root.join("usb").join(s.name).exists() {
            boothready_core::demo::create_stick(&available, s)?;
        }
    }
    Ok(())
}

#[tauri::command]
fn demo_available(state: State<'_, AppState>) -> Res<Vec<DemoStickInfo>> {
    let root = state.demo_root().ok_or("Demo mode is off.")?;
    Ok(boothready_core::demo::STICKS
        .iter()
        .map(|s| DemoStickInfo {
            name: s.name.into(),
            title: format!("{} {}", s.maker, s.product),
            size_gb: s.size / 1_000_000_000,
            inserted: root.join("usb").join(s.name).exists(),
            description: s.description.into(),
        })
        .collect())
}

#[tauri::command]
fn demo_insert(state: State<'_, AppState>, name: String) -> Res<()> {
    let root = state.demo_root().ok_or("Demo mode is off.")?;
    std::fs::rename(root.join("available").join(&name), root.join("usb").join(&name)).map_err(e2s)?;
    let dir = root.join("usb").join(&name);
    let _ = std::fs::remove_file(dir.join(".ejected"));
    let _ = std::fs::remove_file(dir.join(".unmounted"));
    Ok(())
}

#[tauri::command]
fn demo_remove(state: State<'_, AppState>, name: String) -> Res<()> {
    let root = state.demo_root().ok_or("Demo mode is off.")?;
    std::fs::rename(root.join("usb").join(&name), root.join("available").join(&name)).map_err(e2s)
}

#[tauri::command]
fn demo_reset(app: AppHandle, state: State<'_, AppState>) -> Res<()> {
    let root = state.demo_root().ok_or("Demo mode is off.")?;
    let _ = std::fs::remove_dir_all(&root);
    set_mode(&app, &state, Mode::Demo(root))
}

/// Stand-in for the user finishing an export in rekordbox: writes a fresh
/// export with both databases. `legacy_safe` leaves out hi-res files, as a
/// Legacy Rescue export of essential playlists would.
#[tauri::command]
fn demo_simulate_export(state: State<'_, AppState>, device_id: String, legacy_safe: bool) -> Res<()> {
    let root = state.demo_root().ok_or("Demo mode is off.")?;
    let volume = root.join("usb").join(device_id.trim_start_matches("demo:")).join("volume");
    let content = if legacy_safe { DemoContent::BothLibraries } else { DemoContent::FullExport };
    boothready_core::demo::populate(&volume, content).map_err(e2s)
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let data = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data)?;
            let store = Store::open(&data.join("boothready.sqlite")).ok();
            let demo_env = std::env::var_os("BOOTHREADY_DEMO").map(PathBuf::from);
            let mode = match demo_env {
                Some(p) if p.as_os_str() == "1" => Mode::Demo(data.join("demo")),
                Some(p) => Mode::Demo(p),
                None => Mode::Real,
            };
            let state = AppState {
                inner: Mutex::new(Inner {
                    mode: Mode::Real,
                    platform: Arc::from(boothready_platform::native()),
                    watcher: None,
                    reports: HashMap::new(),
                    identities: HashMap::new(),
                    store,
                }),
                rules: Ruleset::builtin(),
                catalog: UsbCatalog::builtin(),
                cancel_verify: Arc::new(AtomicBool::new(false)),
            };
            app.manage(state);
            let handle = app.handle().clone();
            let st = app.state::<AppState>();
            set_mode(&handle, &st, mode).map_err(Box::<dyn std::error::Error>::from)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            set_demo_mode,
            list_devices,
            analyze,
            identity_candidates,
            confirm_identity,
            presets,
            hardware,
            assess,
            plan,
            confirmation,
            prepare,
            remove_apple_double,
            export_status,
            open_rekordbox,
            verify,
            cancel_verify,
            eject,
            known_media,
            profiles,
            save_profile,
            demo_available,
            demo_insert,
            demo_remove,
            demo_reset,
            demo_simulate_export,
        ])
        .run(tauri::generate_context!())
        .expect("error while running BoothReady");
}

/// Snapshot of real engine output for the browser-only UI mock
/// (`cargo run -p boothready-app --example mock_snapshot`). Runs the demo
/// flow through the same DTO code the commands use, so the mock can't drift
/// from what the app really returns.
pub fn mock_snapshot(root: &Path) -> serde_json::Value {
    use boothready_core::demo::{create_stick, populate, STICKS};
    use boothready_platform::demo::DemoPlatform;
    use serde_json::json;
    let rules = Ruleset::builtin();
    let catalog = UsbCatalog::builtin();
    let usb = root.join("usb");
    std::fs::create_dir_all(&usb).unwrap();
    for s in STICKS {
        create_stick(&usb, s).unwrap();
    }
    let platform = DemoPlatform::new(usb.clone());
    let backend = DemoBackend::new(usb.clone());
    let analyze = |id: &str| -> DriveReport {
        let dev = platform.list_devices().unwrap().into_iter().find(|d| d.id == id).unwrap();
        let raw = backend.open_read(&dev).ok().and_then(|f| boothready_core::media::inspect(f).ok());
        analyze_drive(&dev, raw, &rules, &catalog, |_, _| {})
    };
    let assessments = |r: &DriveReport| -> serde_json::Value {
        let mut m = serde_json::Map::new();
        for p in &rules.presets {
            m.insert(
                p.id.clone(),
                serde_json::to_value(assess_drive(&r.facts, &rules.devices_by_id(&p.devices), &rules)).unwrap(),
            );
        }
        serde_json::Value::Object(m)
    };
    let state = |r: &DriveReport| -> serde_json::Value {
        json!({
            "card": DeviceCard::new(r.device.clone(), r.identification.clone(), media_key(&r.device), None),
            "summary": DriveSummary::from_report(r),
            "assessments": assessments(r),
            "confirmation": ConfirmationDetails::new(r, &r.fingerprint),
            "candidates": r.identification.candidates,
        })
    };
    let mut drives = serde_json::Map::new();
    for s in STICKS {
        let id = format!("demo:{}", s.name);
        let initial = analyze(&id);
        // Rebuild as MBR + FAT32, then a fresh export: what the flow produces.
        let dev = initial.device.clone();
        let fp = DeviceFingerprint::of(&dev);
        let req = HelperRequest::Prepare {
            confirmation: fp.token(),
            expected: fp,
            scheme: PartitionScheme::Mbr,
            filesystem: FilesystemKind::Fat32,
            label: "BR_MAIN".into(),
        };
        boothready_helper::handle(&backend, req, &mut |_| {});
        let prepared = analyze(&id);
        let volume = usb.join(s.name).join("volume");
        populate(&volume, if s.name == "kingston-32" { DemoContent::BothLibraries } else { DemoContent::FullExport })
            .unwrap();
        let exported = analyze(&id);
        drives.insert(
            id,
            json!({ "initial": state(&initial), "prepared": state(&prepared), "exported": state(&exported) }),
        );
    }
    let club = rules.preset("unknown_club").unwrap();
    let plan = plan_kit(
        &KitRequest {
            targets: club.devices.clone(),
            redundancy: true,
            library_bytes: 94_000_000_000,
            essential_bytes: Some(20_000_000_000),
            drives: STICKS
                .iter()
                .map(|s| DriveCandidate {
                    id: format!("demo:{}", s.name),
                    name: format!("{} {}", s.maker, s.product),
                    vendor: Some(s.maker.into()),
                    capacity_bytes: s.size,
                })
                .collect(),
        },
        &rules,
    );
    json!({
        "rules": { "version": rules.version, "review_status": rules.review_status, "review_note": rules.review_note },
        "presets": rules.presets,
        "hardware": rules.devices.iter().map(HardwareCard::from).collect::<Vec<_>>(),
        "search": {
            "old nexus": rules.search("old nexus").into_iter().map(HardwareCard::from).collect::<Vec<_>>(),
            "denon": rules.search("denon").into_iter().map(HardwareCard::from).collect::<Vec<_>>(),
        },
        "demo_sticks": STICKS.iter().map(|s| json!({"name": s.name, "title": format!("{} {}", s.maker, s.product), "size_gb": s.size / 1_000_000_000, "description": s.description})).collect::<Vec<_>>(),
        "drives": drives,
        "plan_unknown_club": plan,
    })
}
