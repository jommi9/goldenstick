//! boothready-helper: the only BoothReady process that runs with elevated
//! rights. It accepts a single structured JSON request (from a file or
//! stdin), never a command line or a path to operate on, and reports
//! progress as JSON lines.
//!
//! Usage:
//!   boothready-helper --request <file> --response <file>
//!   boothready-helper --stdio          (one request per line, for development)

#![cfg_attr(test, allow(clippy::unwrap_used))]

use boothready_core::privileged::{HelperErrorCode, HelperEvent, HelperRequest};
use boothready_helper::{handle, NativeBackend};
use std::io::{BufRead, Write};

const MAX_REQUEST_BYTES: u64 = 64 * 1024;

fn parse(text: &str) -> Result<HelperRequest, HelperEvent> {
    serde_json::from_str(text)
        .map_err(|e| HelperEvent::Error { code: HelperErrorCode::BadRequest, message: format!("bad request: {e}") })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let backend = NativeBackend::new();
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();

    if args.iter().any(|a| a == "--stdio") {
        let stdin = std::io::stdin();
        let mut out = std::io::stdout();
        for line in stdin.lock().lines().map_while(Result::ok) {
            if line.trim().is_empty() {
                continue;
            }
            let mut emit = |e: HelperEvent| {
                let _ = writeln!(out, "{}", serde_json::to_string(&e).unwrap_or_default());
                let _ = out.flush();
            };
            match parse(&line) {
                Ok(req) => handle(&backend, req, &mut emit),
                Err(e) => emit(e),
            }
        }
        return;
    }

    let (Some(req_path), Some(resp_path)) = (arg("--request"), arg("--response")) else {
        eprintln!("usage: boothready-helper --request <file> --response <file> | --stdio");
        std::process::exit(2);
    };
    let mut resp = match open_response(&resp_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("cannot open response file: {e}");
            std::process::exit(2);
        }
    };
    let mut emit = |e: HelperEvent| {
        let _ = writeln!(resp, "{}", serde_json::to_string(&e).unwrap_or_default());
        let _ = resp.flush();
    };
    let text = match std::fs::metadata(&req_path) {
        Ok(m) if m.len() <= MAX_REQUEST_BYTES => std::fs::read_to_string(&req_path).unwrap_or_default(),
        _ => String::new(),
    };
    match parse(&text) {
        Ok(req) => handle(&backend, req, &mut emit),
        Err(e) => emit(e),
    }
}

fn open_response(path: &str) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}
