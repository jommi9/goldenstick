//! Helpers shared by tests that build real disk images with system tools.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::{Command, Stdio};

pub fn have_tool(name: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {name} || test -x /usr/sbin/{name} || test -x /sbin/{name}")])
        .stdout(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn resolve(name: &str) -> String {
    for dir in ["/usr/sbin", "/sbin", "/usr/local/sbin"] {
        let p = format!("{dir}/{name}");
        if Path::new(&p).exists() {
            return p;
        }
    }
    name.to_string()
}

pub fn run_tool(name: &str, args: &[&str], stdin: Option<&str>) -> String {
    let mut child = Command::new(resolve(name))
        .args(args)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to run {name}: {e}"));
    if let Some(input) = stdin {
        child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{name} {args:?} failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn sparse_file(path: &Path, len: u64) {
    let f = File::create(path).unwrap();
    f.set_len(len).unwrap();
}

/// Copy a formatted partition image into a disk image at `offset`, skipping
/// all-zero blocks so sparse images stay sparse.
pub fn splice_partition(disk: &Path, part: &Path, offset: u64) {
    let mut src = File::open(part).unwrap();
    let mut dst = OpenOptions::new().write(true).open(disk).unwrap();
    let mut buf = vec![0u8; 1 << 20];
    let mut pos = 0u64;
    loop {
        let n = src.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        if buf[..n].iter().any(|&b| b != 0) {
            dst.seek(SeekFrom::Start(offset + pos)).unwrap();
            dst.write_all(&buf[..n]).unwrap();
        }
        pos += n as u64;
    }
}
