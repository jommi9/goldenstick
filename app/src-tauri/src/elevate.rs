//! Launching the privileged helper with the OS's own elevation prompt.
//!
//! The request travels as a JSON file the helper reads; progress comes back
//! as JSON lines the helper appends to a response file, which we tail. No
//! command line built here ever contains user-controlled text: the helper
//! path is ours and the two file paths are random names in our temp dir.
//!
//! Production builds should replace the prompt-per-operation approach with
//! an installed privileged service (SMAppService on macOS, a service or
//! scheduled task on Windows). The request/response protocol stays the same.

use boothready_core::privileged::{HelperEvent, HelperRequest};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn helper_path() -> io::Result<PathBuf> {
    if let Some(p) = std::env::var_os("BOOTHREADY_HELPER") {
        return Ok(PathBuf::from(p));
    }
    let exe = std::env::current_exe()?;
    let dir = exe.parent().ok_or_else(|| io::Error::other("no exe dir"))?;
    let name = if cfg!(windows) { "boothready-helper.exe" } else { "boothready-helper" };
    for candidate in [dir.join(name), dir.join("../Resources").join(name)] {
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "boothready-helper not found next to the app (build it with `cargo build -p boothready-helper`)",
    ))
}

fn temp_pair() -> (PathBuf, PathBuf) {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let dir = std::env::temp_dir();
    (dir.join(format!("br-{id}-req.json")), dir.join(format!("br-{id}-resp.jsonl")))
}

/// Run one request through the elevated helper, forwarding its events.
pub fn run_helper(req: &HelperRequest, emit: &mut dyn FnMut(HelperEvent)) -> io::Result<()> {
    let helper = helper_path()?;
    let (req_path, resp_path) = temp_pair();
    std::fs::write(&req_path, serde_json::to_vec(req)?)?;
    std::fs::write(&resp_path, b"")?;
    let mut child = spawn_elevated(&helper, &req_path, &resp_path)?;
    let mut seen = 0usize;
    loop {
        let finished = child.try_wait()?;
        let text = std::fs::read_to_string(&resp_path).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        for line in &lines[seen.min(lines.len())..] {
            if let Ok(e) = serde_json::from_str::<HelperEvent>(line) {
                emit(e);
            }
        }
        seen = lines.len();
        if finished.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = std::fs::remove_file(&req_path);
    let _ = std::fs::remove_file(&resp_path);
    if seen == 0 {
        emit(HelperEvent::Error {
            code: boothready_core::privileged::HelperErrorCode::PermissionDenied,
            message: "Administrator permission was not granted. Nothing was changed.".into(),
        });
    }
    Ok(())
}

trait Waitable {
    fn try_wait(&mut self) -> io::Result<Option<()>>;
}

#[cfg(not(windows))]
struct Proc(std::process::Child);

#[cfg(not(windows))]
impl Waitable for Proc {
    fn try_wait(&mut self) -> io::Result<Option<()>> {
        Ok(self.0.try_wait()?.map(|_| ()))
    }
}

#[cfg(target_os = "macos")]
fn spawn_elevated(helper: &Path, req: &Path, resp: &Path) -> io::Result<Box<dyn Waitable>> {
    // AppleScript's `quoted form of` handles quoting for /bin/sh.
    let script = format!(
        "do shell script (quoted form of {:?}) & \" --request \" & (quoted form of {:?}) & \" --response \" & (quoted form of {:?}) with administrator privileges with prompt \"BoothReady needs permission to prepare your USB drive.\"",
        helper.to_string_lossy(),
        req.to_string_lossy(),
        resp.to_string_lossy()
    );
    let child = std::process::Command::new("/usr/bin/osascript").args(["-e", &script]).spawn()?;
    Ok(Box::new(Proc(child)))
}

#[cfg(target_os = "linux")]
fn spawn_elevated(helper: &Path, req: &Path, resp: &Path) -> io::Result<Box<dyn Waitable>> {
    let child = std::process::Command::new("pkexec")
        .arg(helper)
        .arg("--request")
        .arg(req)
        .arg("--response")
        .arg(resp)
        .spawn()?;
    Ok(Box::new(Proc(child)))
}

#[cfg(windows)]
fn spawn_elevated(helper: &Path, req: &Path, resp: &Path) -> io::Result<Box<dyn Waitable>> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::WaitForSingleObject;
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

    struct Elevated(HANDLE);
    impl Waitable for Elevated {
        fn try_wait(&mut self) -> io::Result<Option<()>> {
            // SAFETY: waiting on a process handle we own.
            let r = unsafe { WaitForSingleObject(self.0, 0) };
            Ok((r == WAIT_OBJECT_0).then_some(()))
        }
    }
    impl Drop for Elevated {
        fn drop(&mut self) {
            // SAFETY: closing the handle ShellExecuteExW returned.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
    let file = HSTRING::from(helper.as_os_str());
    let params = HSTRING::from(format!("--request \"{}\" --response \"{}\"", req.display(), resp.display()));
    let verb = HSTRING::from("runas");
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY: all strings outlive the call; `info` is fully initialised.
    unsafe { ShellExecuteExW(&mut info) }
        .map_err(|e| io::Error::new(io::ErrorKind::PermissionDenied, e.to_string()))?;
    Ok(Box::new(Elevated(info.hProcess)))
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn spawn_elevated(_: &Path, _: &Path, _: &Path) -> io::Result<Box<dyn Waitable>> {
    Err(io::Error::other("elevation is not supported on this OS"))
}

/// Bring rekordbox to the front so the user can export.
pub fn open_rekordbox() -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let ok = std::process::Command::new("/usr/bin/open").args(["-a", "rekordbox"]).status()?.success()
            || std::process::Command::new("/usr/bin/open")
                .args(["-b", "com.pioneerdj.rekordboxdj"])
                .status()?
                .success();
        return if ok { Ok(()) } else { Err(io::Error::new(io::ErrorKind::NotFound, "rekordbox isn't installed")) };
    }
    #[cfg(windows)]
    {
        let pf =
            std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Program Files"));
        let base = pf.join("Pioneer");
        if let Ok(rd) = std::fs::read_dir(&base) {
            let mut dirs: Vec<PathBuf> =
                rd.flatten().map(|e| e.path()).filter(|p| p.to_string_lossy().contains("rekordbox")).collect();
            dirs.sort();
            if let Some(exe) = dirs.last().map(|d| d.join("rekordbox.exe")).filter(|p| p.exists()) {
                std::process::Command::new(exe).spawn()?;
                return Ok(());
            }
        }
        return Err(io::Error::new(io::ErrorKind::NotFound, "rekordbox isn't installed"));
    }
    #[allow(unreachable_code)]
    Err(io::Error::new(io::ErrorKind::Unsupported, "rekordbox isn't available on this OS"))
}
