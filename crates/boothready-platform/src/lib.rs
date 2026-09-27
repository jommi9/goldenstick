//! Physical device discovery for BoothReady.
//!
//! Each OS backend answers three questions: which storage devices exist
//! (with USB identity and volumes), how to unmount them, and how to eject
//! them safely. Everything identifies drives by physical device, never by
//! drive letter or mount point, because those change.

pub mod demo;
#[cfg(target_os = "linux")]
pub mod linux;
pub mod macos;
pub mod raw;
#[cfg(windows)]
pub mod windows;
mod windows_layout;

use boothready_model::{DeviceEvent, PhysicalDevice};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("device {0} not found")]
    NotFound(String),
    /// Something still has files open. `holder` names the process when the
    /// OS tells us (Windows eject veto, `lsof`/`fuser` on Unix).
    #[error("the drive is in use{}", holder.as_ref().map(|h| format!(" by {h}")).unwrap_or_default())]
    Busy { holder: Option<String> },
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("{0}")]
    Failed(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub trait Platform: Send + Sync {
    /// All storage devices, including internal ones (flagged `is_system`),
    /// so the UI can explain why a disk isn't offered.
    fn list_devices(&self) -> Result<Vec<PhysicalDevice>, PlatformError>;

    /// Unmount every volume on the device without powering it off.
    fn unmount(&self, device_id: &str) -> Result<(), PlatformError>;

    /// Flush, unmount and power off so the drive can be pulled.
    fn eject(&self, device_id: &str) -> Result<(), PlatformError>;

    fn name(&self) -> &'static str;
}

/// The backend for the OS we're running on.
pub fn native() -> Box<dyn Platform> {
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacPlatform)
    }
    #[cfg(windows)]
    {
        Box::new(windows::WindowsPlatform)
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LinuxPlatform::system())
    }
    #[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
    {
        Box::new(demo::DemoPlatform::new(std::env::temp_dir().join("boothready-demo")))
    }
}

/// Compare two snapshots and report what changed.
pub fn diff(before: &HashMap<String, PhysicalDevice>, after: &[PhysicalDevice]) -> Vec<DeviceEvent> {
    let mut events = Vec::new();
    for d in after {
        match before.get(&d.id) {
            None => events.push(DeviceEvent::Appeared { device: d.clone() }),
            Some(old) if old != d => events.push(DeviceEvent::Changed { device: d.clone() }),
            _ => {}
        }
    }
    for id in before.keys() {
        if !after.iter().any(|d| &d.id == id) {
            events.push(DeviceEvent::Disappeared { id: id.clone() });
        }
    }
    events
}

/// Polls the platform and reports device changes. A one-second interval
/// keeps the PRD's two-second detection target with room to spare; native
/// notifications can replace polling per OS without changing callers.
pub struct Watcher {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Watcher {
    pub fn spawn<F>(platform: Arc<dyn Platform>, interval: Duration, mut on_event: F) -> Watcher
    where
        F: FnMut(DeviceEvent) + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = thread::spawn(move || {
            let mut known: HashMap<String, PhysicalDevice> = HashMap::new();
            while !flag.load(Ordering::Relaxed) {
                if let Ok(devices) = platform.list_devices() {
                    for e in diff(&known, &devices) {
                        on_event(e);
                    }
                    known = devices.into_iter().map(|d| (d.id.clone(), d)).collect();
                }
                let mut slept = Duration::ZERO;
                while slept < interval && !flag.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(50));
                    slept += Duration::from_millis(50);
                }
            }
        });
        Watcher { stop, handle: Some(handle) }
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Free space and allocation unit of the filesystem holding a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Space {
    /// Bytes the current user can still write.
    pub free_bytes: u64,
    /// Every file occupies a whole number of these (the cluster size on FAT
    /// and exFAT).
    pub cluster_bytes: u64,
}

/// How much room is left on the volume mounted at `path`.
pub fn space(path: &std::path::Path) -> std::io::Result<Space> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes())?;
        // SAFETY: statvfs is plain data; zeroed is a valid initial value.
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: `c` is a valid NUL-terminated path and `st` is writable.
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        #[allow(clippy::unnecessary_cast)] // the field widths differ by OS
        Ok(Space { free_bytes: st.f_bavail as u64 * st.f_frsize as u64, cluster_bytes: st.f_frsize as u64 })
    }
    #[cfg(windows)]
    {
        windows::space(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "free space isn't available on this OS"))
    }
}

/// Run a fixed program with fixed arguments. Never goes through a shell.
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn run(program: &str, args: &[&str]) -> Result<String, PlatformError> {
    let out = std::process::Command::new(program).args(args).output()?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let lower = err.to_lowercase();
        if lower.contains("busy") || lower.contains("in use") || lower.contains("dissent") {
            Err(PlatformError::Busy { holder: None })
        } else if lower.contains("permission") || lower.contains("not permitted") {
            Err(PlatformError::PermissionDenied(err))
        } else {
            Err(PlatformError::Failed(if err.is_empty() { format!("{program} failed") } else { err }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn watcher_reports_insert_and_removal() {
        let dir = tempfile::tempdir().unwrap();
        let p: Arc<dyn Platform> = Arc::new(demo::DemoPlatform::new(dir.path().to_path_buf()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let w = Watcher::spawn(p, Duration::from_millis(100), move |e| sink.lock().unwrap().push(e));
        // Wait for each event with a generous timeout rather than a fixed
        // sleep, so slow CI runners don't turn this into a flake.
        let wait_for = |pred: &dyn Fn(&DeviceEvent) -> bool| {
            let start = std::time::Instant::now();
            while start.elapsed() < Duration::from_secs(10) {
                if events.lock().unwrap().iter().any(pred) {
                    return true;
                }
                thread::sleep(Duration::from_millis(20));
            }
            false
        };
        demo::DemoPlatform::create_stick(dir.path(), "stick1", "SanDisk", "Ultra", 0x0781, 0x5581, 32_000_000_000)
            .unwrap();
        assert!(wait_for(&|e| matches!(e, DeviceEvent::Appeared { .. })), "no Appeared event");
        std::fs::remove_dir_all(dir.path().join("stick1")).unwrap();
        assert!(wait_for(&|e| matches!(e, DeviceEvent::Disappeared { .. })), "no Disappeared event");
        w.stop();
        let ev = events.lock().unwrap();
        assert!(matches!(ev.first(), Some(DeviceEvent::Appeared { .. })), "{ev:?}");
        assert!(matches!(ev.last(), Some(DeviceEvent::Disappeared { .. })), "{ev:?}");
    }

    #[test]
    fn space_reports_the_volume_under_a_path() {
        let dir = tempfile::tempdir().unwrap();
        let s = space(dir.path()).unwrap();
        assert!(s.free_bytes > 0 && s.cluster_bytes >= 512, "{s:?}");
        assert!(space(&dir.path().join("missing")).is_err());
    }

    /// Runs the real backend against the machine's own disks. Every Windows
    /// and macOS machine has a system disk, so this catches what the parser
    /// tests can't, like an IOCTL that needs more access than enumeration
    /// asks for, or a filesystem diskutil doesn't name.
    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn native_backend_describes_the_system_disk() {
        let devices = native().list_devices().unwrap();
        let sys = devices.iter().find(|d| d.is_system).unwrap_or_else(|| panic!("no system disk in {devices:#?}"));
        assert!(sys.size_bytes > 0, "{sys:#?}");
        assert!(sys.partition_scheme.is_some(), "{sys:#?}");
        assert!(sys.volumes.iter().any(|v| v.filesystem.is_some()), "{sys:#?}");
        // A Mac's startup disk holds APFS containers, whose volume names and
        // mount points come from the synthesized disks diskutil reports.
        #[cfg(target_os = "macos")]
        assert!(sys.volumes.iter().any(|v| v.label.is_some() && v.mount_point.is_some()), "{sys:#?}");
    }
}
