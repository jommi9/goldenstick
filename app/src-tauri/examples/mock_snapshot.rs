//! Writes app/src/mock-data.json from real engine output.

fn main() {
    let dir = std::env::temp_dir().join(format!("boothready-mock-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let snapshot = boothready_app::mock_snapshot(&dir);
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/mock-data.json");
    std::fs::write(&out, serde_json::to_string(&snapshot).unwrap()).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    println!("wrote {}", out.display());
}
