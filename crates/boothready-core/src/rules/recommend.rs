use super::model::{Claim, DeviceProfile, Evidence, Support};
use boothready_model::{FilesystemKind, PartitionScheme};
use serde::{Deserialize, Serialize};

/// The layout BoothReady would build for a set of target devices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormatRecommendation {
    pub scheme: PartitionScheme,
    pub filesystem: FilesystemKind,
    /// Weakest evidence among the targets this layout covers.
    pub evidence: Evidence,
    /// Device IDs this layout is expected to work on.
    pub covers: Vec<String>,
    /// Device IDs no layout we can build is known to work on.
    pub not_covered: Vec<String>,
    /// Plain-language answer to "Why this format?".
    pub why: String,
}

/// Layouts BoothReady can create on both macOS and Windows.
const CANDIDATES: [(PartitionScheme, FilesystemKind); 2] =
    [(PartitionScheme::Mbr, FilesystemKind::Fat32), (PartitionScheme::Mbr, FilesystemKind::Exfat)];

fn combined(d: &DeviceProfile, scheme: PartitionScheme, fs: FilesystemKind) -> Option<Evidence> {
    let p: Claim = d.partition(scheme);
    let f: Claim = d.filesystem(fs);
    (p.support == Support::Supported && f.support == Support::Supported).then(|| p.evidence.min(f.evidence))
}

fn join_models(devs: &[&DeviceProfile]) -> String {
    let names: Vec<&str> = devs.iter().map(|d| d.model.as_str()).collect();
    match names.len() {
        0 => String::new(),
        1 => names[0].to_string(),
        n => format!("{} and {}", names[..n - 1].join(", "), names[n - 1]),
    }
}

pub fn recommend_format(targets: &[&DeviceProfile]) -> FormatRecommendation {
    let engine_only = !targets.is_empty() && targets.iter().all(|d| d.family == "engine");
    let mut best: Option<(usize, Evidence, usize, (PartitionScheme, FilesystemKind))> = None;
    for (i, &(scheme, fs)) in CANDIDATES.iter().enumerate() {
        let results: Vec<Option<Evidence>> = targets.iter().map(|d| combined(d, scheme, fs)).collect();
        let covered = results.iter().filter(|r| r.is_some()).count();
        let weakest = results.iter().flatten().min().copied().unwrap_or(Evidence::Unknown);
        // Engine DJ recommends exFAT; everyone else gets FAT32 first.
        let pref = if engine_only { i } else { CANDIDATES.len() - i };
        let key = (covered, weakest, pref, (scheme, fs));
        if best.as_ref().is_none_or(|b| (key.0, key.1, key.2) > (b.0, b.1, b.2)) {
            best = Some(key);
        }
    }
    let (_, evidence, _, (scheme, filesystem)) = best.expect("candidates are non-empty");
    let covers: Vec<String> =
        targets.iter().filter(|d| combined(d, scheme, filesystem).is_some()).map(|d| d.id.clone()).collect();
    let not_covered: Vec<String> = targets.iter().filter(|d| !covers.contains(&d.id)).map(|d| d.id.clone()).collect();
    let why = explain(targets, scheme, filesystem, &not_covered);
    FormatRecommendation { scheme, filesystem, evidence, covers, not_covered, why }
}

fn explain(targets: &[&DeviceProfile], scheme: PartitionScheme, fs: FilesystemKind, not_covered: &[String]) -> String {
    let mut parts = Vec::new();
    if fs == FilesystemKind::Fat32 {
        let no_exfat: Vec<&DeviceProfile> = targets
            .iter()
            .copied()
            .filter(|d| d.filesystem(FilesystemKind::Exfat).support != Support::Supported)
            .collect();
        let documented: Vec<&DeviceProfile> = no_exfat
            .iter()
            .copied()
            .filter(|d| {
                d.filesystem(FilesystemKind::Exfat).support == Support::Unsupported
                    && d.filesystem(FilesystemKind::Exfat).evidence.is_strong()
            })
            .collect();
        if !documented.is_empty() {
            let vendors: Vec<&str> = {
                let mut v: Vec<&str> = documented.iter().map(|d| d.manufacturer.as_str()).collect();
                v.dedup();
                v
            };
            parts.push(format!(
                "You selected equipment that includes the {}. {} documents FAT, FAT32 and HFS+ support for {}, while newer systems also read exFAT.",
                join_models(&documented),
                vendors.join(" and "),
                if documented.len() == 1 { "that player" } else { "those players" },
            ));
        } else if !no_exfat.is_empty() {
            parts.push(format!(
                "It isn't documented that the {} can read exFAT, so BoothReady doesn't rely on it.",
                join_models(&no_exfat)
            ));
        }
        parts.push("FAT32 gives the selected equipment a common filesystem.".into());
    } else if fs == FilesystemKind::Exfat {
        parts.push("Engine DJ recommends exFAT, and every selected player reads it.".into());
    }
    if scheme == PartitionScheme::Mbr {
        let no_gpt: Vec<&DeviceProfile> = targets
            .iter()
            .copied()
            .filter(|d| matches!(d.partition(PartitionScheme::Gpt).support, Support::Unsupported | Support::Unreliable))
            .collect();
        if !no_gpt.is_empty() {
            parts.push(format!(
                "The drive gets an MBR partition table, because the GUID partition map macOS uses by default isn't reliably recognised by the {}.",
                join_models(&no_gpt)
            ));
        }
    }
    if !not_covered.is_empty() {
        let missing: Vec<&DeviceProfile> = targets.iter().copied().filter(|d| not_covered.contains(&d.id)).collect();
        parts.push(format!(
            "BoothReady has no reliable data on which format the {} needs, so this drive isn't counted as ready for it.",
            join_models(&missing)
        ));
    }
    parts.join(" ")
}
