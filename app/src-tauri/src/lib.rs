//! Tauri shell around the BoothReady engine.
//!
//! Commands are thin: they look up state, run engine code on a blocking
//! thread, and return plain data. Long operations stream progress as events
//! so the UI stays responsive (PRD §70).

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod dto;
mod elevate;
mod export;
mod library;

use boothready_core::copy::{plan_copy, run_copy, CopyPlan, CopyProblem, CopyReport, CopyTarget};
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
use library::{LibraryPage, LibrarySource, LibraryState, TrackQuery};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
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
    cancel_copy: Arc<AtomicBool>,
    /// Loaded on first use and dropped when the mode changes, so demo mode
    /// never shows or alters the user's real library.
    library: Mutex<Option<LibraryState>>,
    cancel_library: Arc<AtomicBool>,
}

type Res<T> = Result<T, String>;

fn e2s<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

impl AppState {
    /// The app state. A thread that panicked while holding the lock leaves
    /// plain data behind (mode, reports, identities), which every command
    /// can still use, so a poisoned lock is recovered rather than taking
    /// the whole app down with it.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn platform(&self) -> Arc<dyn Platform> {
        self.lock().platform.clone()
    }

    fn backend(&self) -> Box<dyn Backend> {
        match &self.lock().mode {
            Mode::Demo(root) => Box::new(DemoBackend::new(root.join("usb"))),
            Mode::Real => Box::new(NativeBackend::new()),
        }
    }

    fn demo_root(&self) -> Option<PathBuf> {
        match &self.lock().mode {
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

    /// The name a drive is shown with: the user's confirmed identity, or
    /// the best catalog match.
    fn display_name(&self, dev: &PhysicalDevice) -> String {
        let confirmed = self.lock().identities.get(&media_key(dev)).cloned();
        confirmed.unwrap_or_else(|| boothready_core::identify::identify(dev, &self.catalog)).display_name
    }

    /// Run `f` on the library for the current mode, loading it if needed.
    fn with_library<T>(&self, app: &AppHandle, f: impl FnOnce(&mut LibraryState) -> T) -> Res<T> {
        let demo_root = self.demo_root();
        let mut guard = self.library.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let library = match &mut *guard {
            Some(library) => library,
            empty => empty.insert(match demo_root {
                Some(root) => LibraryState::demo(&root)?,
                None => LibraryState::load(app.path().app_data_dir().map_err(e2s)?.join("library")),
            }),
        };
        Ok(f(library))
    }

    fn report(&self, id: &str) -> Res<DriveReport> {
        self.lock().reports.get(id).cloned().ok_or_else(|| "Scan the drive first.".to_string())
    }
}

fn start_watcher(app: &AppHandle, platform: Arc<dyn Platform>) -> Watcher {
    let handle = app.clone();
    Watcher::spawn(platform, Duration::from_millis(1000), move |ev| {
        if let DeviceEvent::Disappeared { id } = &ev {
            if let Some(state) = handle.try_state::<AppState>() {
                state.lock().reports.remove(id);
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
    state.cancel_library.store(true, Ordering::Relaxed);
    *state.library.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    let mut inner = state.lock();
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
    let inner = state.lock();
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
    let inner = state.lock();
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
    let confirmed = state.lock().identities.get(&key).cloned();
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
                        total: p.files_total,
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
    let mut inner = state.lock();
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
    let mut inner = state.lock();
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

/// Save a diagnostics report to the Downloads folder and return its path.
#[tauri::command]
async fn save_diagnostics(app: AppHandle, state: State<'_, AppState>) -> Res<String> {
    let platform = state.platform();
    let report = tauri::async_runtime::spawn_blocking(move || {
        boothready_platform::diagnostics::collect(platform.as_ref(), env!("CARGO_PKG_VERSION"))
    })
    .await
    .map_err(e2s)?;
    let dir = app.path().download_dir().or_else(|_| app.path().home_dir()).map_err(e2s)?;
    let path = dir.join(format!("boothready-diagnostics-{}-{}.json", report.os, report.created_unix));
    std::fs::write(&path, serde_json::to_vec_pretty(&report).map_err(e2s)?).map_err(e2s)?;
    Ok(path.display().to_string())
}

// ------------------------------------------------------------- test lab

fn test_lab_dir(app: &AppHandle, kind: &str) -> Res<PathBuf> {
    let dir =
        app.path().download_dir().or_else(|_| app.path().home_dir()).map_err(e2s)?.join("BoothReady").join("test-lab");
    boothready_testlab::create_report_directory(&dir, kind).map_err(e2s)
}

fn safe_test_label(label: Option<String>) -> String {
    let raw = label.unwrap_or_else(|| "baseline".into());
    let mut out: String =
        raw.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    if out.is_empty() {
        out = "baseline".into();
    }
    out.truncate(48);
    out
}

#[tauri::command]
fn test_lab_scenarios() -> Vec<boothready_testlab::ScenarioInfo> {
    boothready_testlab::scenarios()
}

#[tauri::command]
async fn test_lab_virtual(
    app: AppHandle,
    state: State<'_, AppState>,
    scenario: Option<String>,
    keep_fixtures: bool,
) -> Res<boothready_testlab::VirtualSuite> {
    let root = test_lab_dir(&app, "virtual")?.join("suite");
    let rules = state.rules.clone();
    let catalog = state.catalog.clone();
    tauri::async_runtime::spawn_blocking(move || {
        boothready_testlab::run_virtual_suite(scenario.as_deref(), keep_fixtures, Some(root), &rules, &catalog)
            .map_err(e2s)
    })
    .await
    .map_err(e2s)?
}

#[tauri::command]
async fn test_lab_save_diagnostics(app: AppHandle, state: State<'_, AppState>, label: Option<String>) -> Res<String> {
    let platform = state.platform();
    let report = tauri::async_runtime::spawn_blocking(move || {
        boothready_platform::diagnostics::collect(platform.as_ref(), env!("CARGO_PKG_VERSION"))
    })
    .await
    .map_err(e2s)?;
    let dir = test_lab_dir(&app, "diagnostics")?;
    let path = dir.join(format!("diagnostics-{}-{}.json", safe_test_label(label), report.created_unix));
    std::fs::write(&path, serde_json::to_vec_pretty(&report).map_err(e2s)?).map_err(e2s)?;
    Ok(path.display().to_string())
}

#[tauri::command]
async fn test_lab_real(
    app: AppHandle,
    state: State<'_, AppState>,
    device_id: String,
    targets: Vec<String>,
) -> Res<TestLabRealResult> {
    if state.demo_root().is_some() {
        return Err("Turn off Demo mode before reading a real USB. This test is read-only and never prepares, copies or ejects the drive.".into());
    }
    let out = test_lab_dir(&app, "real")?;
    let platform = state.platform();
    let rules = state.rules.clone();
    let catalog = state.catalog.clone();
    tauri::async_runtime::spawn_blocking(move || -> Res<TestLabRealResult> {
        let before = boothready_platform::diagnostics::collect(platform.as_ref(), env!("CARGO_PKG_VERSION"));
        let before_path = out.join("diagnostics-before.json");
        std::fs::write(&before_path, serde_json::to_vec_pretty(&before).map_err(e2s)?).map_err(e2s)?;

        let dev = platform
            .list_devices()
            .map_err(e2s)?
            .into_iter()
            .find(|d| d.id == device_id)
            .ok_or_else(|| format!("No exact device ID '{device_id}'. Refresh devices and try again."))?;
        if dev.is_system {
            return Err(format!("Refusing to test system device {}.", dev.id));
        }
        if dev.bus != boothready_model::BusType::Usb {
            return Err(format!("Refusing to test non-USB device {}.", dev.id));
        }
        let backend = NativeBackend::new();
        let raw = backend.open_read(&dev).ok().and_then(|f| boothready_core::media::inspect(f).ok());
        let drive = analyze_drive(&dev, raw, &rules, &catalog, |_, _| {});
        let target_ids = if targets.is_empty() {
            rules.preset("unknown_club").map(|p| p.devices.clone()).unwrap_or_default()
        } else {
            targets
        };
        let target_devices = rules.devices_by_id(&target_ids);
        if target_devices.is_empty() {
            return Err("Pick at least one target device before running the real USB test.".into());
        }
        let assessment = assess_drive(&drive.facts, &target_devices, &rules);
        let after = boothready_platform::diagnostics::collect(platform.as_ref(), env!("CARGO_PKG_VERSION"));
        let after_path = out.join("diagnostics-after.json");
        std::fs::write(&after_path, serde_json::to_vec_pretty(&after).map_err(e2s)?).map_err(e2s)?;
        let device = TestLabDevice {
            id: dev.id.clone(),
            display_name: drive.identification.display_name.clone(),
            model: dev.storage_model.clone().or_else(|| dev.usb.as_ref().and_then(|u| u.product.clone())),
            size_bytes: dev.size_bytes,
            is_usb: dev.bus == boothready_model::BusType::Usb,
            is_system: dev.is_system,
            mount_point: dev.primary_mount().and_then(|v| v.mount_point.as_ref().map(|p| p.display().to_string())),
        };
        let read_issues = boothready_testlab::read_issues(&drive);
        let result = TestLabRealResult {
            kind: "real_read_only",
            read_ok: read_issues.is_empty(),
            read_issues,
            device,
            summary: DriveSummary::from_report(&drive),
            assessment,
            diagnostics_before: before_path.display().to_string(),
            diagnostics_after: after_path.display().to_string(),
            report: out.join("report.json").display().to_string(),
        };
        std::fs::write(out.join("report.json"), serde_json::to_vec_pretty(&result).map_err(e2s)?).map_err(e2s)?;
        Ok(result)
    })
    .await
    .map_err(e2s)?
}

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
    let library_bytes =
        device_id.as_ref().and_then(|id| state.lock().reports.get(id).map(|r| r.content.used_bytes)).unwrap_or(0);
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
    state.lock().reports.remove(&device_id);
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
        device_library: boothready_core::library::device_library_written_unix(&mut resolver),
        one_library: boothready_core::library::one_library_written_unix(&mut resolver),
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

// ---------------------------------------------------------------- copy

fn mounted_root(dev: &PhysicalDevice) -> Res<(PathBuf, Option<FilesystemKind>)> {
    let vol = dev.primary_mount().ok_or("The drive isn't mounted.")?;
    Ok((vol.mount_point.clone().ok_or("The drive isn't mounted.")?, vol.filesystem))
}

fn copy_plan(src: &Path, dst: &Path, filesystem: Option<FilesystemKind>) -> Res<CopyPlan> {
    let space = boothready_platform::space(dst).map_err(|e| format!("Couldn't read the free space: {e}"))?;
    let target = CopyTarget { filesystem, free_bytes: space.free_bytes, cluster_bytes: space.cluster_bytes };
    plan_copy(src, dst, &target).map_err(e2s)
}

/// A dry run of copying a finished Device Library export onto `dst_root`.
fn export_copy_source(
    made: &export::LocalExport,
    dst_root: &Path,
    filesystem: Option<FilesystemKind>,
) -> Res<CopySource> {
    copy_plan(Path::new(&made.path), dst_root, filesystem).map(|plan| CopySource {
        device_id: format!("export:{}", made.name),
        display_name: made.name.clone(),
        role: None,
        files: plan.files.len() as u32,
        bytes: plan.files.iter().map(|f| f.size).sum(),
        problems: plan.problems.iter().map(|p| p.describe()).collect(),
        needs_erase: plan.problems.iter().any(|p| matches!(p, CopyProblem::DestinationNotEmpty { .. })),
        export: true,
    })
}

/// A dry run of copying `dev` onto `dst_root`, if `dev` is worth copying
/// from: it holds a DJ library and a passing verification that still
/// matches what's on it.
fn copy_source(
    dev: &PhysicalDevice,
    display_name: String,
    dst_root: &Path,
    filesystem: Option<FilesystemKind>,
) -> Option<Res<CopySource>> {
    let (root, _) = mounted_root(dev).ok()?;
    let m = read_manifest(&root)?;
    let verified = m
        .verification
        .as_ref()
        .is_some_and(|v| v.passed && v.content_fingerprint == boothready_core::manifest::content_fingerprint(&root));
    if !verified || scan_libraries(&root).formats.is_empty() {
        return None;
    }
    Some(copy_plan(&root, dst_root, filesystem).map(|plan| CopySource {
        device_id: dev.id.clone(),
        display_name,
        role: m.role,
        files: plan.files.len() as u32,
        bytes: plan.files.iter().map(|f| f.size).sum(),
        problems: plan.problems.iter().map(|p| p.describe()).collect(),
        needs_erase: plan.problems.iter().any(|p| matches!(p, CopyProblem::DestinationNotEmpty { .. })),
        export: false,
    }))
}

/// What could be copied onto `device_id`, each with a dry run of the copy:
/// the Device Library exports built on this computer, newest first, then
/// the connected drives that hold a verified library.
#[tauri::command]
async fn copy_sources(app: AppHandle, state: State<'_, AppState>, device_id: String) -> Res<Vec<CopySource>> {
    let devices = state.platform().list_devices().map_err(e2s)?;
    let dst = devices.iter().find(|d| d.id == device_id).ok_or("That USB drive is no longer connected.")?;
    let (dst_root, filesystem) = mounted_root(dst)?;
    let candidates: Vec<(PhysicalDevice, String)> = devices
        .iter()
        .filter(|d| d.id != device_id && d.primary_mount().is_some())
        .map(|d| (d.clone(), state.display_name(d)))
        .collect();
    let exports = export::list_exports(&export::exports_root(&app, &state)?);
    tauri::async_runtime::spawn_blocking(move || -> Res<Vec<CopySource>> {
        let mut out: Vec<CopySource> =
            exports.iter().filter_map(|made| export_copy_source(made, &dst_root, filesystem).ok()).collect();
        for (dev, name) in candidates {
            if let Some(source) = copy_source(&dev, name, &dst_root, filesystem) {
                out.push(source?);
            }
        }
        Ok(out)
    })
    .await
    .map_err(e2s)?
}

/// Copy one drive's contents, or a Device Library export built on this
/// computer (`from_id` of `export:<name>`), onto a drive, streaming
/// `copy-progress`.
#[tauri::command]
async fn copy_drive(app: AppHandle, state: State<'_, AppState>, from_id: String, to_id: String) -> Res<CopyReport> {
    let src = match from_id.strip_prefix("export:") {
        Some(name) => export::export_path(&app, &state, name)?,
        None => mounted_root(&state.device(&from_id)?)?.0,
    };
    let (dst, filesystem) = mounted_root(&state.device(&to_id)?)?;
    let cancel = state.cancel_copy.clone();
    cancel.store(false, Ordering::Relaxed);
    let id2 = to_id.clone();
    let report = tauri::async_runtime::spawn_blocking(move || {
        let plan = copy_plan(&src, &dst, filesystem)?;
        run_copy(&plan, &src, &dst, &cancel, |p| {
            let _ = app.emit("copy-progress", VerifyProgressEvent { device_id: id2.clone(), progress: p.clone() });
        })
        .map_err(e2s)
    })
    .await
    .map_err(e2s)??;
    state.lock().reports.remove(&to_id);
    Ok(report)
}

#[tauri::command]
fn cancel_copy(state: State<'_, AppState>) {
    state.cancel_copy.store(true, Ordering::Relaxed);
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
    state.lock().store.as_ref().and_then(|s| s.known_media().ok()).unwrap_or_default()
}

#[tauri::command]
fn profiles(state: State<'_, AppState>) -> Vec<GigProfile> {
    state.lock().store.as_ref().and_then(|s| s.profiles().ok()).unwrap_or_default()
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
    let inner = state.lock();
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

// ---------------------------------------------------------------- music library

#[derive(Clone, Serialize)]
struct LibraryScanEvent {
    source_id: String,
    files: u32,
    bytes: u64,
    cloud_only: u32,
}

#[tauri::command]
fn library_sources(app: AppHandle, state: State<'_, AppState>, targets: Vec<String>) -> Res<Vec<LibrarySource>> {
    let players = state.rules.devices_by_id(&targets);
    state.with_library(&app, |library| library.list(&players))
}

/// Ask the user for a folder with the system's own dialog.
#[tauri::command]
async fn library_pick_folder(app: AppHandle) -> Res<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog().file().set_title("Choose a music folder").blocking_pick_folder()
    })
    .await
    .map_err(e2s)?;
    Ok(picked.and_then(|folder| folder.into_path().ok()).map(|path| path.display().to_string()))
}

/// Where a folder is stored. Demo folders sit in a temporary directory, so
/// only their cloud-style path is meaningful there.
fn locate_source(path: &Path, platform: &dyn Platform, demo: bool) -> library::Location {
    if demo {
        library::locate(path, &[]).or_computer()
    } else {
        library::locate(path, &platform.list_devices().unwrap_or_default())
    }
}

#[tauri::command]
async fn library_add(app: AppHandle, state: State<'_, AppState>, path: String) -> Res<String> {
    let folder = library::resolve_folder(Path::new(path.trim()))?;
    let demo = state.demo_root().is_some();
    let platform = state.platform();
    let probe = folder.clone();
    let location = tauri::async_runtime::spawn_blocking(move || locate_source(&probe, platform.as_ref(), demo))
        .await
        .map_err(e2s)?;
    state.with_library(&app, |library| library.add(folder, location, now_unix()))?
}

/// Forget a source. The folder and its files are left alone.
#[tauri::command]
fn library_remove(app: AppHandle, state: State<'_, AppState>, id: String) -> Res<()> {
    state.with_library(&app, |library| library.remove(&id))
}

/// Scan one source. Cloud-only files are listed from their metadata and
/// never opened, so a scan downloads nothing.
#[tauri::command]
async fn library_scan(app: AppHandle, state: State<'_, AppState>, id: String) -> Res<()> {
    let demo = state.demo_root().is_some();
    let record = state.with_library(&app, |library| {
        if library.is_busy() {
            return Err("Another scan or collection is still running.".to_string());
        }
        let record = library.record(&id)?;
        library.set_scanning(&id, true);
        Ok(record)
    })??;
    let cancel = state.cancel_library.clone();
    cancel.store(false, Ordering::Relaxed);
    let platform = state.platform();
    let emitter = app.clone();
    let source_id = id.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let location = locate_source(&record.path, platform.as_ref(), demo);
        let mut last = std::time::Instant::now();
        let progress = |p: &boothready_core::sources::SourceProgress| {
            if p.files == 1 || last.elapsed() >= Duration::from_millis(120) {
                last = std::time::Instant::now();
                let event = LibraryScanEvent {
                    source_id: source_id.clone(),
                    files: p.files,
                    bytes: p.bytes,
                    cloud_only: p.cloud_only,
                };
                let _ = emitter.emit("library-scan-progress", event);
            }
        };
        let scan = if demo {
            let placeholder = boothready_core::sources::listed_placeholder(&record.path);
            boothready_core::sources::scan_source(&record.path, &placeholder, &cancel, progress)
        } else {
            let placeholder = boothready_core::sources::os_placeholder;
            boothready_core::sources::scan_source(&record.path, &placeholder, &cancel, progress)
        };
        scan.map(|mut scan| {
            if demo {
                library::demo_sizes(&mut scan);
            }
            (scan, location)
        })
    })
    .await;
    state.with_library(&app, |library| library.set_scanning(&id, false))?;
    let (scan, location) = outcome.map_err(e2s)?.map_err(|e| format!("The folder couldn't be read: {e}"))?;
    if scan.cancelled {
        return Err("Scan stopped. The earlier results were kept.".to_string());
    }
    state.with_library(&app, |library| library.store_scan(&id, scan, location, now_unix()))
}

#[tauri::command]
fn library_cancel_scan(state: State<'_, AppState>) {
    state.cancel_library.store(true, Ordering::Relaxed);
}

/// One page of a source's tracks. `filter` is a track state or "all".
#[derive(serde::Deserialize)]
struct TrackRequest {
    id: String,
    targets: Vec<String>,
    #[serde(flatten)]
    query: TrackQuery,
}

#[tauri::command]
fn library_tracks(app: AppHandle, state: State<'_, AppState>, request: TrackRequest) -> Res<LibraryPage> {
    let players = state.rules.devices_by_id(&request.targets);
    let mut query = request.query;
    query.limit = query.limit.clamp(1, 500);
    state.with_library(&app, |library| library.tracks(&request.id, &players, &query))?
}

/// Which tracks to collect: a source and the filter and search shown on it.
#[derive(serde::Deserialize)]
struct CollectRequest {
    id: String,
    targets: Vec<String>,
    filter: String,
    query: String,
    /// A folder inside the source, or empty for all of it.
    #[serde(default)]
    folder: String,
    /// The collection's name. Empty asks for the suggested one.
    name: String,
}

#[derive(Serialize)]
struct CollectPlan {
    tracks: u32,
    bytes: u64,
    cloud_only: u32,
    cloud_only_bytes: u64,
    /// Matching tracks left out because they are damaged or not audio.
    unreadable: u32,
    name: String,
    destination: String,
    /// A folder with that name is already there. The same selection resumes it.
    exists: bool,
    needed_bytes: u64,
    free_bytes: Option<u64>,
}

#[derive(Clone, Serialize)]
struct LibraryCollectEvent {
    source_id: String,
    phase: boothready_core::staging::StagePhase,
    files_done: u32,
    files_total: u32,
    bytes_done: u64,
    bytes_total: u64,
}

/// Where collections are kept: a visible folder in the user's Music folder,
/// or inside the demo folder in demo mode.
fn collections_root(app: &AppHandle, state: &AppState) -> Res<PathBuf> {
    match state.demo_root() {
        Some(root) => Ok(root.join("Collections")),
        None => Ok(app.path().audio_dir().or_else(|_| app.path().home_dir()).map_err(e2s)?.join("BoothReady")),
    }
}

/// The nearest folder above `path` that exists, for a free-space lookup.
fn existing_ancestor(path: &Path) -> &Path {
    let mut at = path;
    while !at.exists() {
        match at.parent() {
            Some(parent) => at = parent,
            None => break,
        }
    }
    at
}

fn collect_plan(
    app: &AppHandle,
    state: &AppState,
    request: &CollectRequest,
) -> Res<(CollectPlan, library::Selection, PathBuf)> {
    let players = state.rules.devices_by_id(&request.targets);
    let selection = state.with_library(app, |library| {
        library.selection(&request.id, &players, &request.filter, &request.query, &request.folder)
    })??;
    let source_name =
        selection.source.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Music".into());
    let typed = if request.name.trim().is_empty() {
        format!("{source_name} {}", library::date_of(now_unix()))
    } else {
        request.name.clone()
    };
    let name = library::collection_folder_name(&typed)?;
    let destination = collections_root(app, state)?.join(&name);
    let space = boothready_platform::space(existing_ancestor(&destination)).ok();
    let needed_bytes = library::space_needed(
        selection.bytes,
        selection.paths.len() as u64,
        space.map(|s| s.cluster_bytes).unwrap_or(4096),
    );
    let plan = CollectPlan {
        tracks: selection.paths.len() as u32,
        bytes: selection.bytes,
        cloud_only: selection.cloud_only,
        cloud_only_bytes: selection.cloud_only_bytes,
        unreadable: selection.unreadable,
        name,
        destination: destination.display().to_string(),
        exists: destination.exists(),
        needed_bytes,
        free_bytes: space.map(|s| s.free_bytes),
    };
    Ok((plan, selection, destination))
}

/// Allow the window to stream one track and return its file path, which
/// the UI turns into an asset URL. Only audio files inside a source are
/// allowed, one at a time as they are played.
#[tauri::command]
fn library_play(app: AppHandle, state: State<'_, AppState>, id: String, path: String) -> Res<String> {
    let file = state.with_library(&app, |library| library.playable_path(&id, &path))??;
    app.asset_protocol_scope().allow_file(&file).map_err(e2s)?;
    Ok(file.display().to_string())
}

/// One track's analysis for the list and the player: from the cache the
/// exports share, or analysed now when `analyze` is set. The window asks with
/// `analyze` only for tracks already on this computer, so nothing is downloaded.
#[tauri::command]
async fn library_analysis(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    path: String,
    analyze: bool,
) -> Res<Option<boothready_core::export::analysis_view::AnalysisView>> {
    let file = state.with_library(&app, |library| library.playable_path(&id, &path))??;
    let cache = boothready_core::export::analysis_cache::AnalysisCache::new(export::analysis_cache_dir(&app, &state)?);
    tauri::async_runtime::spawn_blocking(move || {
        boothready_core::export::analysis_view::cached_or_analyzed(&cache, &file, analyze)
    })
    .await
    .map_err(e2s)?
}

/// Compare the music library with the rekordbox library on a scanned stick:
/// which playlists match, differ, or exist on one side only, and which stick
/// tracks have no copy in the library.
#[tauri::command]
fn stick_compare(
    app: AppHandle,
    state: State<'_, AppState>,
    device_id: String,
) -> Res<Option<boothready_core::library::compare::Comparison>> {
    use boothready_core::library::compare::{compare, StickFile, StickPlaylist};
    let report = state.lock().reports.get(&device_id).cloned().ok_or("Scan the stick first.")?;
    let Some(dl) = report.libraries.rekordbox.as_ref().and_then(|rb| rb.device_library.as_ref()) else {
        return Ok(None);
    };
    let stick: Vec<StickFile> = dl
        .tracks
        .iter()
        .map(|t| StickFile {
            id: t.id,
            title: t.title.clone(),
            artist: t.artist.clone(),
            path: t.path.clone(),
            size: t.db_file_size,
        })
        .collect();
    let playlists: Vec<StickPlaylist> = dl
        .playlists
        .iter()
        .filter(|p| !p.is_folder)
        .map(|p| StickPlaylist { name: p.name.clone(), track_ids: p.track_ids.clone() })
        .collect();
    let files = state.with_library(&app, |library| library.compare_files())?;
    Ok(Some(compare(&files, &stick, &playlists)))
}

/// Copy tracks from a stick into a folder of a music source: one stick
/// playlist's tracks when `playlist` names one, or else every stick track that
/// has no copy in the library. The files come from the stick, so nothing is
/// downloaded, and the cloud client uploads them in the background. A file
/// already in the folder with the same size is skipped; one with another size
/// is kept beside it under a new name. Nothing is overwritten or deleted.
/// Returns how many files were copied.
#[tauri::command]
async fn stick_to_library(
    app: AppHandle,
    state: State<'_, AppState>,
    device_id: String,
    source_id: String,
    folder: String,
    playlist: Option<String>,
) -> Res<u32> {
    use std::path::Component;
    if folder.is_empty() || !Path::new(&folder).components().all(|c| matches!(c, Component::Normal(_))) {
        return Err("That isn't a folder inside the music source.".to_string());
    }
    let report = state.lock().reports.get(&device_id).cloned().ok_or("Scan the stick first.")?;
    let dev = state.device(&device_id)?;
    let root = dev.primary_mount().and_then(|v| v.mount_point.clone()).ok_or("The stick isn't mounted.")?;
    let dl = report
        .libraries
        .rekordbox
        .as_ref()
        .and_then(|rb| rb.device_library.clone())
        .ok_or("This stick has no rekordbox library.")?;
    let source_root = state.with_library(&app, |library| library.record(&source_id).map(|r| r.path))??;
    let wanted: Vec<String> = match &playlist {
        Some(name) => {
            let p = dl
                .playlists
                .iter()
                .find(|p| !p.is_folder && &p.name == name)
                .ok_or("That playlist isn't on the stick any more.")?;
            p.track_ids.iter().filter_map(|id| dl.tracks.iter().find(|t| t.id == *id)).map(|t| t.path.clone()).collect()
        }
        None => {
            use boothready_core::library::compare::{compare, StickFile, StickPlaylist};
            let stick: Vec<StickFile> = dl
                .tracks
                .iter()
                .map(|t| StickFile {
                    id: t.id,
                    title: t.title.clone(),
                    artist: t.artist.clone(),
                    path: t.path.clone(),
                    size: t.db_file_size,
                })
                .collect();
            let lists: Vec<StickPlaylist> = dl
                .playlists
                .iter()
                .filter(|p| !p.is_folder)
                .map(|p| StickPlaylist { name: p.name.clone(), track_ids: p.track_ids.clone() })
                .collect();
            let files = state.with_library(&app, |library| library.compare_files())?;
            compare(&files, &stick, &lists).orphans.into_iter().map(|o| o.path).collect()
        }
    };
    tauri::async_runtime::spawn_blocking(move || library::copy_into(&root, &wanted, &source_root.join(&folder)))
        .await
        .map_err(e2s)?
}

/// The folders inside a source, for the browser's tree.
#[tauri::command]
fn library_folders(app: AppHandle, state: State<'_, AppState>, id: String) -> Res<Vec<library::FolderNode>> {
    state.with_library(&app, |library| library.folders(&id))?
}

/// What collecting the shown tracks would copy, and where to.
#[tauri::command]
fn library_collect_plan(app: AppHandle, state: State<'_, AppState>, request: CollectRequest) -> Res<CollectPlan> {
    collect_plan(&app, &state, &request).map(|(plan, _, _)| plan)
}

/// Copy the shown tracks into a new verified collection on this computer and
/// add it to the library. Reading a cloud-only track makes the cloud client
/// download it. Returns the new source's id.
#[tauri::command]
async fn library_collect(app: AppHandle, state: State<'_, AppState>, request: CollectRequest) -> Res<String> {
    let (plan, selection, destination) = collect_plan(&app, &state, &request)?;
    if plan.tracks == 0 {
        return Err("There are no readable tracks to collect.".to_string());
    }
    if plan.free_bytes.is_some_and(|free| free < plan.needed_bytes) {
        return Err("There isn't enough free space on this computer for these tracks.".to_string());
    }
    state.with_library(&app, |library| {
        if library.is_busy() {
            return Err("Another scan or collection is still running.".to_string());
        }
        library.set_collecting(true);
        Ok(())
    })??;
    let demo = state.demo_root().is_some();
    let cancel = state.cancel_library.clone();
    cancel.store(false, Ordering::Relaxed);
    let emitter = app.clone();
    let source_id = request.id.clone();
    let bytes_total = plan.bytes;
    let job_selection = selection.clone();
    let job_destination = destination.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let mut last = std::time::Instant::now();
        let mut phase = None;
        let mut progress = |p: &boothready_core::staging::StageProgress| {
            if phase != Some(p.phase) || last.elapsed() >= Duration::from_millis(120) {
                phase = Some(p.phase);
                last = std::time::Instant::now();
                let event = LibraryCollectEvent {
                    source_id: source_id.clone(),
                    phase: p.phase,
                    files_done: p.files_done,
                    files_total: p.files_total,
                    bytes_done: p.bytes_done,
                    bytes_total,
                };
                let _ = emitter.emit("library-collect-progress", event);
            }
        };
        library::run_collect(&job_selection, &job_destination, &cancel, &mut progress).map(|(made, mut scan)| {
            if demo {
                library::demo_sizes(&mut scan);
            }
            (made, scan)
        })
    })
    .await;
    state.with_library(&app, |library| library.set_collecting(false))?;
    let (made, scan) = outcome.map_err(e2s)??;
    let origin =
        selection.source.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Music".into());
    let root = destination.canonicalize().unwrap_or(destination);
    let reference = library::CollectionRef { name: plan.name, root, origin, revision: made.revision };
    state.with_library(&app, |library| library.add_collection(reference, scan, library::Location::Computer, now_unix()))
}

#[tauri::command]
fn library_cancel_collect(state: State<'_, AppState>) {
    state.cancel_library.store(true, Ordering::Relaxed);
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
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
                cancel_copy: Arc::new(AtomicBool::new(false)),
                library: Mutex::new(None),
                cancel_library: Arc::new(AtomicBool::new(false)),
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
            copy_sources,
            copy_drive,
            cancel_copy,
            eject,
            known_media,
            profiles,
            save_profile,
            demo_available,
            demo_insert,
            demo_remove,
            demo_reset,
            demo_simulate_export,
            save_diagnostics,
            test_lab_scenarios,
            test_lab_virtual,
            test_lab_save_diagnostics,
            test_lab_real,
            library_sources,
            library_pick_folder,
            library_add,
            library_remove,
            library_scan,
            library_cancel_scan,
            library_tracks,
            library_folders,
            library_play,
            library_analysis,
            stick_compare,
            stick_to_library,
            library_collect_plan,
            library_collect,
            library_cancel_collect,
            export::library_export_plan,
            export::library_export,
            export::library_cancel_export,
            export::library_exports,
            export::library_export_remove,
        ])
        .run(tauri::generate_context!())
        .expect("error while running BoothReady");
}

/// Snapshot of real engine output for the browser-only UI mock
/// (`cargo run -p boothready-app --example mock_snapshot`). Runs the demo
/// flow through the same DTO code the commands use, so the mock can't drift
/// from what the app really returns.
pub fn mock_snapshot(root: &Path) -> Res<serde_json::Value> {
    use boothready_core::demo::{create_stick, populate, STICKS};
    use boothready_platform::demo::DemoPlatform;
    use serde_json::json;
    let rules = Ruleset::builtin();
    let catalog = UsbCatalog::builtin();
    let usb = root.join("usb");
    std::fs::create_dir_all(&usb).map_err(e2s)?;
    for s in STICKS {
        create_stick(&usb, s).map_err(e2s)?;
    }
    let platform = DemoPlatform::new(usb.clone());
    let backend = DemoBackend::new(usb.clone());
    let analyze = |id: &str| -> Res<DriveReport> {
        let dev = platform
            .list_devices()
            .map_err(e2s)?
            .into_iter()
            .find(|d| d.id == id)
            .ok_or_else(|| format!("demo drive {id} is missing"))?;
        let raw = backend.open_read(&dev).ok().and_then(|f| boothready_core::media::inspect(f).ok());
        Ok(analyze_drive(&dev, raw, &rules, &catalog, |_, _| {}))
    };
    let assessments = |r: &DriveReport| -> Res<serde_json::Value> {
        let mut m = serde_json::Map::new();
        for p in &rules.presets {
            m.insert(
                p.id.clone(),
                serde_json::to_value(assess_drive(&r.facts, &rules.devices_by_id(&p.devices), &rules)).map_err(e2s)?,
            );
        }
        Ok(serde_json::Value::Object(m))
    };
    let state = |r: &DriveReport| -> Res<serde_json::Value> {
        Ok(json!({
            "card": DeviceCard::new(r.device.clone(), r.identification.clone(), media_key(&r.device), None),
            "summary": DriveSummary::from_report(r),
            "assessments": assessments(r)?,
            "confirmation": ConfirmationDetails::new(r, &r.fingerprint),
            "candidates": r.identification.candidates,
        }))
    };
    let mut drives = serde_json::Map::new();
    for s in STICKS {
        let id = format!("demo:{}", s.name);
        let initial = analyze(&id)?;
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
        let prepared = analyze(&id)?;
        let volume = usb.join(s.name).join("volume");
        populate(&volume, if s.name == "kingston-32" { DemoContent::BothLibraries } else { DemoContent::FullExport })
            .map_err(e2s)?;
        let exported = analyze(&id)?;
        drives.insert(
            id,
            json!({ "initial": state(&initial)?, "prepared": state(&prepared)?, "exported": state(&exported)? }),
        );
    }
    let club = rules.preset("unknown_club").ok_or("the built-in ruleset has no unknown_club preset")?;
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
    // The simulated library, once per preset, with the temporary folder
    // replaced by a home folder so the snapshot is the same on every run.
    let library_root = root.canonicalize().map_err(e2s)?;
    let mut library = LibraryState::demo(&library_root)?;
    let shown_root = library_root.display().to_string();
    // Collect the whole cloud folder with the real engine, so the mock can
    // show what those tracks look like once they are on the computer.
    let picked = library.selection("demo-cloud", &[], "all", "", "")?;
    let collected_root = library_root.join("Collections").join("Promos 2026 2026-09-21");
    let (made, mut collected_scan) =
        library::run_collect(&picked, &collected_root, &AtomicBool::new(false), &mut |_| {})?;
    library::demo_sizes(&mut collected_scan);
    let collected_id = library.add_collection(
        library::CollectionRef {
            name: "Promos 2026 2026-09-21".into(),
            root: collected_root,
            origin: "Promos 2026".into(),
            revision: made.revision,
        },
        collected_scan,
        library::Location::Computer,
        1_790_000_000,
    );
    let mut library_by_preset = serde_json::Map::new();
    for p in &rules.presets {
        let players = rules.devices_by_id(&p.devices);
        let mut sources = library.list(&players);
        let mut tracks = serde_json::Map::new();
        let mut collected = serde_json::Value::Null;
        for source in &mut sources {
            source.path = source
                .path
                .replace(&format!("{shown_root}/Volumes"), "/Volumes")
                .replace(&format!("{shown_root}/Collections"), "/Users/dj/Music/BoothReady")
                .replace(&shown_root, "/Users/dj");
            let page = library.tracks(&source.id, &players, &TrackQuery { limit: 500, ..TrackQuery::default() })?;
            if source.id == collected_id {
                source.id = "demo-collected".into();
                collected = json!({ "source": source, "tracks": page.tracks });
            } else {
                tracks.insert(source.id.clone(), serde_json::to_value(page.tracks).map_err(e2s)?);
            }
        }
        sources.retain(|s| s.id != "demo-collected");
        library_by_preset.insert(p.id.clone(), json!({ "sources": sources, "tracks": tracks, "collected": collected }));
    }
    Ok(json!({
        "library": library_by_preset,
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
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use boothready_core::demo::{create_stick, STICKS};
    use boothready_platform::demo::DemoPlatform;

    fn demo_devices(root: &Path) -> Vec<PhysicalDevice> {
        DemoPlatform::new(root.to_path_buf()).list_devices().unwrap()
    }

    fn verify_and_record(root: &Path) {
        let libs = scan_libraries(root);
        let exp = Expectations { scheme: None, filesystem: None };
        let req =
            VerifyRequest { root, layout: None, expect: exp, libs: &libs, manifest: None, mode: VerifyMode::Full };
        let report = verify_volume(&req, &AtomicBool::new(false), |_| {});
        assert!(report.passed, "{:?}", report.failures);
        let mut m = Manifest::new(Some(Role::Main), vec![]);
        m.verification = Some(report.record());
        write_manifest(root, &m).unwrap();
    }

    /// Only a drive with a DJ library and a passing verification that still
    /// matches its contents is offered as something to copy from.
    #[test]
    fn copy_sources_are_verified_and_unchanged() {
        let usb = tempfile::tempdir().unwrap();
        let stick = |name: &str| STICKS.iter().find(|s| s.name == name).unwrap();
        create_stick(usb.path(), stick("samsung-64")).unwrap();
        create_stick(usb.path(), stick("kingston-32")).unwrap();
        let devices = demo_devices(usb.path());
        let find = |name: &str| devices.iter().find(|d| d.id == format!("demo:{name}")).unwrap();
        let (src, dst) = (find("samsung-64"), find("kingston-32"));
        let (src_root, _) = mounted_root(src).unwrap();
        let (dst_root, fs) = mounted_root(dst).unwrap();

        assert!(copy_source(src, "Samsung".into(), &dst_root, fs).is_none(), "unverified");
        assert!(copy_source(dst, "Kingston".into(), &src_root, None).is_none(), "no library");

        verify_and_record(&src_root);
        let offer = copy_source(src, "Samsung".into(), &dst_root, fs).unwrap().unwrap();
        assert!(offer.problems.is_empty() && !offer.needs_erase, "{:?}", offer.problems);
        assert_eq!(offer.role, Some(Role::Main));
        assert!(offer.files > 0 && offer.bytes > 0);

        std::fs::write(src_root.join("Contents/new.mp3"), b"added after verification").unwrap();
        assert!(copy_source(src, "Samsung".into(), &dst_root, fs).is_none(), "changed since verification");
    }

    #[test]
    fn a_destination_with_other_files_needs_an_erase() {
        let usb = tempfile::tempdir().unwrap();
        let stick = |name: &str| STICKS.iter().find(|s| s.name == name).unwrap();
        create_stick(usb.path(), stick("samsung-64")).unwrap();
        create_stick(usb.path(), stick("sandisk-128")).unwrap();
        let devices = demo_devices(usb.path());
        let find = |name: &str| devices.iter().find(|d| d.id == format!("demo:{name}")).unwrap();
        let (src_root, _) = mounted_root(find("samsung-64")).unwrap();
        let (dst_root, fs) = mounted_root(find("sandisk-128")).unwrap();
        verify_and_record(&src_root);
        let offer = copy_source(find("samsung-64"), "Samsung".into(), &dst_root, fs).unwrap().unwrap();
        assert!(offer.needs_erase, "{:?}", offer.problems);
    }
}
