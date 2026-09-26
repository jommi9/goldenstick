//! Human-readable terminal output. Every status symbol has a word next to
//! it, so nothing depends on colour or emoji rendering alone.

use boothready_core::drive::{DriveReport, LayoutSource};
use boothready_core::identify::{identify, UsbCatalog};
use boothready_core::media::MediaLayout;
use boothready_core::planner::{DriveCandidate, KitPlan, GB};
use boothready_core::privileged::eligibility;
use boothready_core::rules::{DriveAssessment, LayerStatus, Verdict};
use boothready_core::verify::VerifyReport;
use boothready_model::PhysicalDevice;

fn gb(b: u64) -> String {
    format!("{:.1} GB", b as f64 / GB as f64)
}

fn mark(s: LayerStatus) -> &'static str {
    match s {
        LayerStatus::Pass => "[ok]  ",
        LayerStatus::Warn => "[warn]",
        LayerStatus::Fail => "[fail]",
        LayerStatus::Unknown => "[?]   ",
        LayerStatus::Info => "[--]  ",
    }
}

fn short(s: LayerStatus) -> &'static str {
    match s {
        LayerStatus::Pass => "ok",
        LayerStatus::Warn => "warn",
        LayerStatus::Fail => "FAIL",
        LayerStatus::Unknown => "?",
        LayerStatus::Info => "-",
    }
}

pub fn devices(devs: &[PhysicalDevice], catalog: &UsbCatalog) {
    if devs.is_empty() {
        println!("No storage devices found. Plug in a USB.");
        return;
    }
    for d in devs {
        let id = identify(d, catalog);
        let e = eligibility(d);
        let vol = d.volumes.first();
        println!(
            "{:<16} {:<34} {:>9}  {:<5} {:<6} {}",
            d.id,
            id.display_name,
            gb(d.size_bytes),
            d.partition_scheme.map(|s| s.label()).unwrap_or("?"),
            vol.and_then(|v| v.filesystem).map(|f| f.label()).unwrap_or("?"),
            if e.eligible { "can prepare".to_string() } else { format!("protected: {}", e.reasons.join(" ")) }
        );
    }
}

pub fn layout(l: &MediaLayout) {
    println!("{} disk, {} ({}-byte sectors)", l.scheme.label(), gb(l.size_bytes), l.sector_size);
    for p in &l.partitions {
        let fs = p.filesystem.as_ref();
        println!(
            "  #{} at {:>8} MiB, {:>9}  {:<6} {:<12} {}{}",
            p.entry.index,
            p.entry.start_bytes / (1 << 20),
            gb(p.entry.size_bytes),
            fs.map(|f| f.kind.label()).unwrap_or("none"),
            fs.and_then(|f| f.label.clone()).unwrap_or_default(),
            fs.and_then(|f| f.cluster_size).map(|c| format!("{} KB clusters", c / 1024)).unwrap_or_default(),
            if fs.and_then(|f| f.dirty) == Some(true) { "  (not ejected safely)" } else { "" }
        );
    }
    for w in &l.warnings {
        println!("  note: {w}");
    }
}

pub fn report(r: &DriveReport) {
    let id = &r.identification;
    println!();
    println!("{}", id.display_name);
    println!("  We think this is: {} (identification: {:?})", id.display_name, id.confidence);
    println!("  {}", id.explanation);
    println!();
    println!("  {}", r.headline);
    println!(
        "  Layout: {} + {}{}",
        r.layout.scheme.label(),
        r.layout.primary_filesystem().map(|f| f.label()).unwrap_or("no filesystem"),
        if r.layout_source == LayoutSource::Os { " (as reported by the OS)" } else { "" }
    );
    if let Some(m) = &r.mount_point {
        println!("  Mounted at {m}: {} files, {} used", r.content.files, gb(r.content.used_bytes));
    }
    if r.interrupted {
        println!("  WARNING: a BoothReady preparation was interrupted on this drive. Don't use it for a gig yet.");
    }
    if r.verified {
        println!("  Verified by BoothReady and unchanged since.");
    }
}

pub fn assessment(a: &DriveAssessment) {
    println!();
    for c in &a.checks {
        println!("  {} {}", mark(c.status), c.label);
    }
    println!(
        "  {} tracks scanned, {} compatible with all selected equipment, {} need attention",
        a.tracks.scanned, a.tracks.compatible_everywhere, a.tracks.need_attention
    );
    println!();
    println!("  Overall: {}. {}", a.overall.label(), a.headline);
    println!();
    println!(
        "  {:<22} {:<7} {:<9} {:<10} {:<7} {:<7} Overall",
        "Hardware", "USB", "Partition", "Filesystem", "Library", "Audio"
    );
    for d in &a.devices {
        let s: Vec<&str> = d.layers.iter().map(|l| short(l.status)).collect();
        println!("  {:<22} {:<7} {:<9} {:<10} {:<7} {:<7} {}", d.model, s[0], s[1], s[2], s[3], s[4], d.summary);
    }
    let problems: Vec<_> = a.devices.iter().filter(|d| d.verdict >= Verdict::Partial).collect();
    if !problems.is_empty() {
        println!();
        for d in problems {
            for l in d.layers.iter().filter(|l| matches!(l.status, LayerStatus::Fail | LayerStatus::Warn)) {
                println!("  {}: {}", d.model, l.detail);
            }
        }
    }
    if !a.fixes.is_empty() {
        println!();
        println!("  Recommended:");
        for f in &a.fixes {
            println!("   - {}{}: {}", f.title, if f.destructive { " (erases the drive)" } else { "" }, f.detail);
        }
    }
    println!();
}

pub fn plan(p: &KitPlan, drives: &[DriveCandidate]) {
    for (i, r) in p.roles.iter().enumerate() {
        let drive = r.assigned_drive.as_ref().and_then(|id| drives.iter().find(|d| &d.id == id));
        println!(
            "USB {} - {} ({} + {}, label {})",
            i + 1,
            r.role.label(),
            r.format.scheme.label(),
            r.format.filesystem.label(),
            r.volume_label
        );
        match drive {
            Some(d) => println!("  Use: {} ({})", d.name, gb(d.capacity_bytes)),
            None => println!("  Needs a drive of at least {}", gb(r.min_capacity_bytes)),
        }
        for line in &r.purpose {
            println!("  - {line}");
        }
        println!("  Why: {}", r.why);
        for n in &r.drive_notes {
            println!("  Note: {n}");
        }
        println!();
    }
    for n in &p.notes {
        println!("Note: {n}");
    }
    if !p.not_covered.is_empty() {
        println!("Not covered by known-good data: {}", p.not_covered.join(", "));
    }
}

pub fn verification(r: &VerifyReport) {
    for c in &r.checks {
        println!("  {} {}: {}", if c.passed { "[ok]  " } else { "[fail]" }, c.name, c.detail);
    }
    for f in r.failures.iter().take(20) {
        println!("    {}: {}", f.path, f.problem);
    }
    println!(
        "{}",
        if r.passed {
            "Verification passed."
        } else if r.cancelled {
            "Verification cancelled."
        } else {
            "Verification FAILED. Don't rely on this drive."
        }
    );
}
