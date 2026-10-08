//! Drive the real helper binary over its stdio protocol.

#![allow(clippy::unwrap_used)]

use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn stdio_protocol() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_boothready-helper"))
        .arg("--stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(stdin, r#"{{"op":"hello","protocol":1}}"#).unwrap();
        writeln!(stdin, r#"{{"op":"hello","protocol":99}}"#).unwrap();
        writeln!(stdin, r#"{{"op":"shell","cmd":"id"}}"#).unwrap();
        writeln!(stdin, "not json").unwrap();
        writeln!(stdin, r#"{{"op":"list_devices"}}"#).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    let lines: Vec<serde_json::Value> =
        String::from_utf8_lossy(&out.stdout).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 5, "{lines:?}");
    assert_eq!(lines[0]["kind"], "hello");
    assert_eq!(lines[1]["code"], "bad_request");
    assert_eq!(lines[2]["code"], "bad_request");
    assert_eq!(lines[3]["code"], "bad_request");
    assert!(lines[4]["kind"] == "devices" || lines[4]["kind"] == "error");
}

#[test]
fn request_file_mode_rejects_oversized_requests() {
    let d = tempfile::tempdir().unwrap();
    let req = d.path().join("req.json");
    let resp = d.path().join("resp.jsonl");
    std::fs::write(&req, format!(r#"{{"op":"hello","protocol":1,"pad":"{}"}}"#, "x".repeat(70_000))).unwrap();
    let st = Command::new(env!("CARGO_BIN_EXE_boothready-helper"))
        .args(["--request", req.to_str().unwrap(), "--response", resp.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(st.success());
    let text = std::fs::read_to_string(&resp).unwrap();
    assert!(text.contains("bad_request"), "{text}");
    std::fs::write(&req, r#"{"op":"hello","protocol":1}"#).unwrap();
    Command::new(env!("CARGO_BIN_EXE_boothready-helper"))
        .args(["--request", req.to_str().unwrap(), "--response", resp.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(std::fs::read_to_string(&resp).unwrap().contains("\"hello\""));
}
