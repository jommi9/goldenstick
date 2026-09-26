//! Turning USB descriptors into a product people recognise.
//!
//! A VID/PID pair is usually shared by a whole product family and several
//! hardware revisions, so identification reports *how sure* it is, and a
//! matching picture is never treated as proof of what's inside.

use boothready_model::PhysicalDevice;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const BUILTIN_CATALOG: &str = include_str!("../data/usb_catalog.json");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogRevision {
    pub bcd_device: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogProduct {
    pub id: String,
    pub manufacturer: String,
    pub name: String,
    pub vendor_id: String,
    pub product_ids: Vec<String>,
    pub descriptor_patterns: Vec<String>,
    pub capacities_gb: Vec<u32>,
    pub connector: String,
    pub usb: String,
    pub image: String,
    pub color: String,
    #[serde(default)]
    pub revisions: Vec<CatalogRevision>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsbCatalog {
    pub schema: u32,
    pub vendors: HashMap<String, String>,
    pub products: Vec<CatalogProduct>,
}

impl UsbCatalog {
    pub fn builtin() -> UsbCatalog {
        serde_json::from_str(BUILTIN_CATALOG).expect("built-in USB catalog must be valid")
    }

    pub fn product(&self, id: &str) -> Option<&CatalogProduct> {
        self.products.iter().find(|p| p.id == id)
    }

    /// Free-text search for the "Search model" fallback.
    pub fn search(&self, q: &str) -> Vec<&CatalogProduct> {
        let q = q.to_lowercase();
        let tokens: Vec<&str> = q.split_whitespace().collect();
        self.products
            .iter()
            .filter(|p| {
                let hay = format!("{} {} {}", p.manufacturer, p.name, p.descriptor_patterns.join(" ")).to_lowercase();
                !tokens.is_empty() && tokens.iter().all(|t| hay.contains(t))
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Unknown,
    Probable,
    Strong,
    Exact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub catalog_id: String,
    pub display_name: String,
    pub image: String,
    pub color: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identification {
    pub confidence: Confidence,
    pub catalog_id: Option<String>,
    /// "SanDisk Ultra USB 3.0 32 GB"
    pub display_name: String,
    pub manufacturer: Option<String>,
    pub marketed_gb: Option<u32>,
    pub image: String,
    pub color: String,
    /// Other products that look plausible, for "Looks different".
    pub candidates: Vec<Candidate>,
    /// Plain-language statement of what we can and can't tell.
    pub explanation: String,
    pub confirmed_by_user: bool,
}

/// Map a raw byte count to the capacity printed on the packaging. Flash
/// drives expose roughly 92-100 % of their marketed decimal gigabytes.
pub fn marketed_gb(bytes: u64) -> Option<u32> {
    const SIZES: [u32; 12] = [1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1000, 2000];
    let gb = bytes as f64 / 1e9;
    SIZES.iter().copied().find(|&s| gb <= s as f64 * 1.001 && gb >= s as f64 * 0.88).or_else(|| {
        // Some drives are sold in binary sizes (e.g. 1024 GB).
        SIZES.iter().copied().find(|&s| {
            let bin = s as f64 * 1.073_741_824;
            gb <= bin * 1.001 && gb >= bin * 0.88
        })
    })
}

fn hex4(v: u16) -> String {
    format!("{v:04x}")
}

fn candidate(p: &CatalogProduct, gb: Option<u32>) -> Candidate {
    Candidate {
        catalog_id: p.id.clone(),
        display_name: name_with_size(&p.manufacturer, &p.name, gb),
        image: p.image.clone(),
        color: p.color.clone(),
    }
}

fn name_with_size(manufacturer: &str, name: &str, gb: Option<u32>) -> String {
    let size = match gb {
        Some(g) if g >= 1000 => format!(" {} TB", g / 1000),
        Some(g) => format!(" {g} GB"),
        None => String::new(),
    };
    format!("{manufacturer} {name}{size}")
}

pub fn identify(dev: &PhysicalDevice, catalog: &UsbCatalog) -> Identification {
    let gb = marketed_gb(dev.size_bytes);
    let usb = dev.usb.clone().unwrap_or_default();
    let descriptor = format!(
        "{} {} {}",
        usb.manufacturer.clone().unwrap_or_default(),
        usb.product.clone().unwrap_or_default(),
        dev.storage_model.clone().unwrap_or_default()
    )
    .to_lowercase();
    let vid = usb.vendor_id.map(hex4);
    let pid = usb.product_id.map(hex4);
    let vendor_name = vid.as_ref().and_then(|v| catalog.vendors.get(v)).cloned();

    let by_id: Vec<&CatalogProduct> = catalog
        .products
        .iter()
        .filter(|p| Some(&p.vendor_id) == vid.as_ref() && pid.as_ref().is_some_and(|pid| p.product_ids.contains(pid)))
        .collect();
    let size_ok = |p: &CatalogProduct| gb.is_some_and(|g| p.capacities_gb.contains(&g));
    let desc_score = |p: &CatalogProduct| {
        p.descriptor_patterns
            .iter()
            .filter(|pat| descriptor.contains(pat.as_str()))
            .map(|pat| pat.len())
            .max()
            .unwrap_or(0)
    };

    let mut ranked: Vec<&CatalogProduct> = by_id.clone();
    ranked.sort_by_key(|p| std::cmp::Reverse((desc_score(p), size_ok(p))));
    let fallback_name = {
        let raw = dev.raw_display_name();
        let generic = raw == "Unknown USB drive"
            || raw.to_lowercase().contains("mass storage")
            || raw.to_lowercase().contains("usb disk");
        match (&vendor_name, generic) {
            (Some(v), true) => name_with_size(v, "USB drive", gb),
            _ => name_with_size("", &raw, gb).trim().to_string(),
        }
    };

    if let Some(best) = ranked.first().copied() {
        let desc = desc_score(best) > 0;
        let size = size_ok(best);
        let unique = ranked.len() == 1 || desc_score(ranked[1]) < desc_score(best);
        let revision = usb.bcd_device.map(hex4).and_then(|b| best.revisions.iter().find(|r| r.bcd_device == b));
        let confidence = if unique && desc && size && revision.is_some() {
            Confidence::Exact
        } else if unique && (desc || size) {
            Confidence::Strong
        } else {
            Confidence::Probable
        };
        let explanation = match confidence {
            Confidence::Exact => format!(
                "Matched the exact product and hardware revision ({}).",
                revision.map(|r| r.label.as_str()).unwrap_or("")
            ),
            Confidence::Strong => {
                "We can identify the product family but not guarantee the internal controller revision.".to_string()
            }
            _ => format!(
                "Several {} products share this hardware ID. Pick the one that looks like yours.",
                best.manufacturer
            ),
        };
        let others: Vec<Candidate> = ranked.iter().skip(1).map(|p| candidate(p, gb)).collect();
        return Identification {
            confidence,
            catalog_id: Some(best.id.clone()),
            display_name: name_with_size(&best.manufacturer, &best.name, gb),
            manufacturer: Some(best.manufacturer.clone()),
            marketed_gb: gb,
            image: best.image.clone(),
            color: best.color.clone(),
            candidates: others,
            explanation,
            confirmed_by_user: false,
        };
    }

    // No ID match: try manufacturer + capacity.
    let maker = vendor_name.clone().or_else(|| usb.manufacturer.clone());
    let same_maker: Vec<&CatalogProduct> = catalog
        .products
        .iter()
        .filter(|p| maker.as_deref().is_some_and(|m| m.to_lowercase().contains(&p.manufacturer.to_lowercase())))
        .filter(|p| size_ok(p))
        .collect();
    if !same_maker.is_empty() {
        return Identification {
            confidence: Confidence::Probable,
            catalog_id: None,
            display_name: fallback_name,
            manufacturer: maker,
            marketed_gb: gb,
            image: "stick-generic".into(),
            color: "#6b7280".into(),
            candidates: same_maker.iter().map(|p| candidate(p, gb)).collect(),
            explanation: "The manufacturer and size match a few products we know. Pick the one that looks like yours."
                .into(),
            confirmed_by_user: false,
        };
    }
    Identification {
        confidence: Confidence::Unknown,
        catalog_id: None,
        display_name: fallback_name,
        manufacturer: maker,
        marketed_gb: gb,
        image: "stick-generic".into(),
        color: "#6b7280".into(),
        candidates: vec![],
        explanation: "This drive reports generic information, so we can't tell which product it is. That doesn't affect the compatibility check.".into(),
        confirmed_by_user: false,
    }
}

/// Apply a user's "that's the one" choice. The choice becomes a signal, not
/// proof: confidence doesn't go up, but the name and picture follow the user.
pub fn apply_user_choice(mut id: Identification, product: &CatalogProduct) -> Identification {
    id.display_name = name_with_size(&product.manufacturer, &product.name, id.marketed_gb);
    id.catalog_id = Some(product.id.clone());
    id.manufacturer = Some(product.manufacturer.clone());
    id.image = product.image.clone();
    id.color = product.color.clone();
    id.confirmed_by_user = true;
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    use boothready_model::{BusType, UsbDescriptor};

    fn dev(vid: u16, pid: u16, product: &str, bytes: u64) -> PhysicalDevice {
        PhysicalDevice {
            id: "d".into(),
            os_path: "/dev/sdz".into(),
            bus: BusType::Usb,
            removable: true,
            is_system: false,
            size_bytes: bytes,
            logical_sector_size: 512,
            storage_vendor: None,
            storage_model: None,
            storage_revision: None,
            usb: Some(UsbDescriptor {
                vendor_id: Some(vid),
                product_id: Some(pid),
                manufacturer: Some(" USB".into()),
                product: Some(product.into()),
                ..Default::default()
            }),
            partition_scheme: None,
            volumes: vec![],
        }
    }

    #[test]
    fn marketed_sizes() {
        assert_eq!(marketed_gb(30_752_000_000), Some(32));
        assert_eq!(marketed_gb(31_914_983_424), Some(32));
        assert_eq!(marketed_gb(61_530_000_000), Some(64));
        assert_eq!(marketed_gb(123_060_000_000), Some(128));
        assert_eq!(marketed_gb(15_376_000_000), Some(16));
        assert_eq!(marketed_gb(12_000_000_000), None);
    }

    #[test]
    fn sandisk_ultra_is_strong() {
        let c = UsbCatalog::builtin();
        let id = identify(&dev(0x0781, 0x5581, "Ultra", 30_752_000_000), &c);
        assert_eq!(id.confidence, Confidence::Strong);
        assert_eq!(id.display_name, "SanDisk Ultra USB 3.0 32 GB");
        assert!(id.explanation.contains("controller revision"));
    }

    #[test]
    fn shared_pid_needs_the_descriptor() {
        let c = UsbCatalog::builtin();
        // Samsung BAR Plus and FIT Plus share 090c:1000.
        let fit = identify(&dev(0x090c, 0x1000, "Flash Drive FIT", 64_000_000_000), &c);
        assert_eq!(fit.catalog_id.as_deref(), Some("samsung-fit-plus"));
        assert_eq!(fit.confidence, Confidence::Strong);
        let vague = identify(&dev(0x090c, 0x1000, "Flash Drive", 64_000_000_000), &c);
        assert_eq!(vague.confidence, Confidence::Probable);
        assert!(!vague.candidates.is_empty());
    }

    #[test]
    fn unknown_and_user_choice() {
        let c = UsbCatalog::builtin();
        let id = identify(&dev(0x1234, 0x5678, "Mass Storage", 8_000_000_000), &c);
        assert_eq!(id.confidence, Confidence::Unknown);
        let chosen = apply_user_choice(id, c.product("kingston-datatraveler").unwrap());
        assert!(chosen.confirmed_by_user);
        assert_eq!(chosen.confidence, Confidence::Unknown);
        assert_eq!(chosen.display_name, "Kingston DataTraveler 8 GB");
        assert_eq!(c.search("kingston").len(), 1);
    }
}
