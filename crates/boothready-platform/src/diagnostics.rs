//! A diagnostics report: what BoothReady made of the machine's drives, next
//! to the raw operating-system output it was built from.
//!
//! When a real drive is misread, the report is enough to reproduce it. On
//! macOS [`replay`] rebuilds the device list from the recorded `diskutil` and
//! `ioreg` output on any OS, so a report becomes a test fixture. Reports hold
//! disk and volume names, sizes, partition layouts and USB IDs, never file
//! names or file contents.

use crate::Platform;
use boothready_model::PhysicalDevice;
use serde::{Deserialize, Serialize};

pub const FORMAT: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diagnostics {
    pub format: u32,
    pub boothready_version: String,
    pub created_unix: u64,
    pub os: String,
    pub arch: String,
    pub backend: String,
    /// What BoothReady made of the drives.
    pub devices: Vec<PhysicalDevice>,
    /// Why listing failed, if it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The raw output behind `devices`.
    pub captures: Vec<Capture>,
}

/// One piece of raw OS output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capture {
    /// The command and its arguments, or the API call and its target.
    pub source: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Binary output such as an IOCTL buffer, as lowercase hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Capture {
    pub fn text(source: &[&str], text: impl Into<String>) -> Capture {
        Capture { source: own(source), text: Some(text.into()), hex: None, error: None }
    }

    pub fn bytes(source: &[&str], bytes: &[u8]) -> Capture {
        Capture { source: own(source), text: None, hex: Some(hex(bytes)), error: None }
    }

    pub fn failed(source: &[&str], error: impl Into<String>) -> Capture {
        Capture { source: own(source), text: None, hex: None, error: Some(error.into()) }
    }

    /// Run a fixed program with fixed arguments, never through a shell, and
    /// record what it printed.
    #[cfg_attr(not(any(unix, windows)), allow(dead_code))]
    pub(crate) fn command(program: &str, args: &[&str]) -> Capture {
        let source: Vec<&str> = std::iter::once(program).chain(args.iter().copied()).collect();
        match std::process::Command::new(program).args(args).output() {
            Ok(o) if o.status.success() => Capture::text(&source, String::from_utf8_lossy(&o.stdout)),
            Ok(o) => Capture {
                error: Some(format!("exit {}: {}", o.status, String::from_utf8_lossy(&o.stderr).trim())),
                ..Capture::text(&source, String::from_utf8_lossy(&o.stdout))
            },
            Err(e) => Capture::failed(&source, e.to_string()),
        }
    }

    pub fn decoded(&self) -> Option<Vec<u8>> {
        match (&self.text, &self.hex) {
            (Some(t), _) => Some(t.as_bytes().to_vec()),
            (None, Some(h)) => unhex(h),
            _ => None,
        }
    }
}

impl Diagnostics {
    /// The output of the capture whose source is exactly `source`.
    pub fn output(&self, source: &[&str]) -> Option<Vec<u8>> {
        self.captures.iter().find(|c| c.source == source).and_then(Capture::decoded)
    }
}

fn own(source: &[&str]) -> Vec<String> {
    source.iter().map(|s| s.to_string()).collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// Collect a report. A demo backend has no OS output worth recording, so
/// its report holds only the simulated devices.
pub fn collect(platform: &dyn Platform, boothready_version: &str) -> Diagnostics {
    let (devices, error) = match platform.list_devices() {
        Ok(d) => (d, None),
        Err(e) => (Vec::new(), Some(e.to_string())),
    };
    let captures = if platform.name() == "demo" { Vec::new() } else { native_captures() };
    Diagnostics {
        format: FORMAT,
        boothready_version: boothready_version.to_string(),
        created_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        backend: platform.name().to_string(),
        devices,
        error,
        captures,
    }
}

fn native_captures() -> Vec<Capture> {
    #[cfg(target_os = "macos")]
    {
        crate::macos::captures()
    }
    #[cfg(windows)]
    {
        crate::windows::captures()
    }
    #[cfg(target_os = "linux")]
    {
        crate::linux::captures()
    }
    #[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
    {
        Vec::new()
    }
}

/// Rebuild the device list from a report's raw output, where the backend
/// allows it (macOS). `None` means the report can't be replayed.
pub fn replay(d: &Diagnostics) -> Option<Result<Vec<PhysicalDevice>, crate::PlatformError>> {
    match d.os.as_str() {
        "macos" => crate::macos::replay(d),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_round_trip_as_json() {
        let d = Diagnostics {
            format: FORMAT,
            boothready_version: "0.1.0".into(),
            created_unix: 1,
            os: "windows".into(),
            arch: "x86_64".into(),
            backend: "windows".into(),
            devices: Vec::new(),
            error: None,
            captures: vec![
                Capture::text(&["diskutil", "list"], "<plist/>"),
                Capture::bytes(&["IOCTL_DISK_GET_DRIVE_LAYOUT_EX", "PhysicalDrive1"], &[0, 1, 0xab, 0xff]),
                Capture::failed(&["powershell"], "not found"),
            ],
        };
        let back: Diagnostics = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(back, d);
        assert_eq!(back.output(&["IOCTL_DISK_GET_DRIVE_LAYOUT_EX", "PhysicalDrive1"]), Some(vec![0, 1, 0xab, 0xff]));
        assert_eq!(back.output(&["diskutil", "list"]), Some(b"<plist/>".to_vec()));
        assert_eq!(back.output(&["powershell"]), None);
        assert_eq!(unhex("abc"), None);
        assert_eq!(unhex("zz"), None);
    }

    #[test]
    fn demo_reports_hold_no_os_output() {
        let dir = tempfile::tempdir().unwrap();
        crate::demo::DemoPlatform::create_stick(dir.path(), "s", "SanDisk", "Ultra", 0x0781, 0x5581, 32_000_000_000)
            .unwrap();
        let d = collect(&crate::demo::DemoPlatform::new(dir.path().to_path_buf()), "0.1.0");
        assert_eq!((d.devices.len(), d.captures.len(), d.backend.as_str()), (1, 0, "demo"));
        assert!(replay(&d).is_none());
    }

    #[cfg(any(target_os = "macos", windows, target_os = "linux"))]
    #[test]
    fn native_reports_record_the_os_output() {
        let d = collect(crate::native().as_ref(), "0.1.0");
        assert!(!d.captures.is_empty());
        assert!(d.captures.iter().any(|c| c.text.is_some() || c.hex.is_some()), "{:#?}", d.captures);
        // On a Mac the recorded output rebuilds the same device list.
        #[cfg(target_os = "macos")]
        assert_eq!(replay(&d).unwrap().unwrap(), d.devices);
    }
}
