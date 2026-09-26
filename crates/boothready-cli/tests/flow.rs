//! The PRD §100 acceptance scenario, driven through the real binary on
//! simulated drives.

use std::process::Command;

fn br(demo: &std::path::Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_boothready")).arg("--demo").arg(demo).args(args).output().unwrap();
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

#[test]
#[cfg_attr(windows, ignore = "formats a 31 GB image, which isn't sparse on NTFS")]
fn acceptance_scenario() {
    let d = tempfile::tempdir().unwrap();
    let demo = d.path();
    assert!(br(demo, &["demo", "init"]).0);
    assert!(br(demo, &["demo", "insert", "sandisk-128"]).0);

    // USB A: GPT + exFAT + Device Library, unknown club.
    let (ok, out) = br(demo, &["--json", "check", "sandisk-128", "--preset", "unknown_club"]);
    assert!(ok, "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["assessment"]["overall"], "fix_needed");
    assert_eq!(v["drive"]["identification"]["display_name"], "SanDisk Ultra USB 3.0 128 GB");
    let fixes: Vec<&str> =
        v["assessment"]["fixes"].as_array().unwrap().iter().map(|f| f["kind"].as_str().unwrap()).collect();
    assert!(fixes.contains(&"export_rekordbox") && fixes.contains(&"reformat"), "{fixes:?}");

    // Insert the 32 GB Kingston and build the Legacy Rescue drive.
    assert!(br(demo, &["demo", "insert", "kingston-32"]).0);
    let (ok, out) = br(demo, &["prepare", "kingston-32", "--fs", "fat32", "--label", "BR_LEGACY", "--yes"]);
    assert!(ok, "{out}");
    let (ok, out) = br(demo, &["--json", "check", "kingston-32", "--targets", "cdj-2000,cdj-2000nxs"]);
    assert!(ok, "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["drive"]["layout"]["scheme"], "mbr");
    assert_eq!(v["drive"]["layout_source"], "raw");
    let checks = v["assessment"]["checks"].as_array().unwrap();
    assert_eq!(checks[0]["label"], "FAT32");
    assert_eq!(checks[0]["status"], "pass");

    // Verification of the (empty) prepared drive passes and is recorded.
    let vol = demo.join("usb/kingston-32/volume");
    let (ok, out) = br(demo, &["verify", vol.to_str().unwrap(), "--full", "--save"]);
    assert!(ok, "{out}");
    assert!(vol.join(".boothready/manifest.json").exists());

    // Refuses to erase without the typed confirmation.
    let out = Command::new(env!("CARGO_BIN_EXE_boothready"))
        .arg("--demo")
        .arg(demo)
        .args(["prepare", "sandisk-128"])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("Nothing was erased"));
}

#[test]
#[cfg(unix)]
fn make_image_refuses_devices() {
    let (ok, out) = br(std::path::Path::new("/nonexistent"), &["make-image", "/dev/null", "--size-gb", "1"]);
    assert!(!ok);
    assert!(out.contains("never writes to devices"), "{out}");
}
