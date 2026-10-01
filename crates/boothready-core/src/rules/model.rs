//! Typed compatibility data. Every claim carries both a support level and the
//! evidence behind it, so the UI can never show an inferred result as verified.

use crate::audio::{Codec, Container};
use crate::library::LibraryFormat;
use boothready_model::{FilesystemKind, PartitionScheme};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, HashMap};
use std::fmt;

/// How we know something. Ordered weakest to strongest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    Unknown,
    Inferred,
    Community,
    Vendor,
    Lab,
}

impl Evidence {
    /// Strong enough to show a green "Ready".
    pub fn is_strong(self) -> bool {
        matches!(self, Evidence::Vendor | Evidence::Lab)
    }

    pub fn label(self) -> &'static str {
        match self {
            Evidence::Unknown => "No reliable evidence",
            Evidence::Inferred => "Inferred",
            Evidence::Community => "Community reports",
            Evidence::Vendor => "Vendor documented",
            Evidence::Lab => "BoothReady lab verified",
        }
    }

    fn parse(s: &str) -> Option<Evidence> {
        Some(match s {
            "unknown" => Evidence::Unknown,
            "inferred" => Evidence::Inferred,
            "community" => Evidence::Community,
            "vendor" => Evidence::Vendor,
            "lab" => Evidence::Lab,
            _ => return None,
        })
    }

    fn key(self) -> &'static str {
        match self {
            Evidence::Unknown => "unknown",
            Evidence::Inferred => "inferred",
            Evidence::Community => "community",
            Evidence::Vendor => "vendor",
            Evidence::Lab => "lab",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Support {
    Supported,
    Unsupported,
    /// The vendor or field reports say it may or may not work.
    Unreliable,
    Unknown,
}

/// A support statement plus its evidence. Serialised as `"supported/vendor"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Claim {
    pub support: Support,
    pub evidence: Evidence,
}

impl Claim {
    pub const UNKNOWN: Claim = Claim { support: Support::Unknown, evidence: Evidence::Unknown };

    pub fn new(support: Support, evidence: Evidence) -> Claim {
        Claim { support, evidence }
    }
}

impl fmt::Display for Claim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self.support {
            Support::Supported => "supported",
            Support::Unsupported => "unsupported",
            Support::Unreliable => "unreliable",
            Support::Unknown => "unknown",
        };
        write!(f, "{s}/{}", self.evidence.key())
    }
}

impl std::str::FromStr for Claim {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (sup, ev) = s.split_once('/').ok_or_else(|| format!("claim '{s}' must look like 'supported/vendor'"))?;
        let support = match sup {
            "supported" => Support::Supported,
            "unsupported" => Support::Unsupported,
            "unreliable" => Support::Unreliable,
            "unknown" => Support::Unknown,
            _ => return Err(format!("unknown support level '{sup}'")),
        };
        let evidence = Evidence::parse(ev).ok_or_else(|| format!("unknown evidence level '{ev}'"))?;
        if support != Support::Unknown && evidence == Evidence::Unknown {
            return Err(format!("claim '{s}' asserts support without evidence"));
        }
        Ok(Claim { support, evidence })
    }
}

impl Serialize for Claim {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Claim {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioRule {
    pub codecs: Vec<Codec>,
    #[serde(default)]
    pub containers: Option<Vec<Container>>,
    #[serde(default)]
    pub sample_rates: Option<Vec<u32>>,
    #[serde(default)]
    pub bit_depths: Option<Vec<u16>>,
    pub evidence: Evidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowerClaim {
    pub value: u32,
    pub evidence: Evidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quirk {
    pub id: String,
    pub severity: Severity,
    pub evidence: Evidence,
    pub text: String,
    /// Reference IDs backing a vendor or community quirk.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    /// Only mention the quirk for drives with one of these filesystems.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub when_filesystem: Vec<FilesystemKind>,
    /// Only mention the quirk for drives with one of these partition schemes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub when_scheme: Vec<PartitionScheme>,
}

impl Quirk {
    pub fn applies(&self, scheme: PartitionScheme, filesystem: Option<FilesystemKind>) -> bool {
        (self.when_scheme.is_empty() || self.when_scheme.contains(&scheme))
            && (self.when_filesystem.is_empty() || filesystem.is_some_and(|f| self.when_filesystem.contains(&f)))
    }
}

/// A document behind one or more claims. Kept at ruleset level because one
/// vendor notice often covers several players.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    pub title: String,
    pub url: String,
    /// What kind of evidence the document is: `vendor` for manufacturer
    /// manuals, FAQs and notices, `community` for forum reports.
    pub kind: Evidence,
    /// When the page was last read, as YYYY-MM-DD.
    pub accessed: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Reference {
    /// "Title (host)", short enough for one line under a verdict.
    pub fn cite(&self) -> String {
        let host = self.url.split("://").nth(1).unwrap_or(&self.url).split('/').next().unwrap_or_default();
        format!("{} ({host})", self.title)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    Player,
    Standalone,
    Controller,
    Software,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceProfile {
    pub id: String,
    pub manufacturer: String,
    pub model: String,
    /// Ecosystem: "alphatheta", "engine", "mixxx".
    pub family: String,
    pub kind: DeviceKind,
    pub released: u16,
    /// Older hardware the planner serves with a separate Legacy Rescue drive.
    pub legacy: bool,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub search_terms: Vec<String>,
    pub partition_tables: HashMap<PartitionScheme, Claim>,
    pub filesystems: HashMap<FilesystemKind, Claim>,
    pub library_formats: BTreeMap<LibraryFormat, Claim>,
    pub folder_browsing: Claim,
    pub audio: Vec<AudioRule>,
    /// When set, a codec that matches no rule is documented as unsupported.
    pub audio_list_exhaustive: Option<Evidence>,
    pub usb_power_ma: Option<PowerClaim>,
    #[serde(default)]
    pub quirks: Vec<Quirk>,
    /// Reference IDs per aspect ("filesystems", "audio", ...).
    #[serde(default)]
    pub sources: BTreeMap<String, Vec<String>>,
    pub last_reviewed: Option<String>,
    /// `sources` resolved to readable citations when the ruleset loads.
    #[serde(skip)]
    pub(crate) cited: BTreeMap<String, String>,
}

impl DeviceProfile {
    pub fn name(&self) -> String {
        format!("{} {}", self.manufacturer, self.model)
    }

    pub fn partition(&self, s: PartitionScheme) -> Claim {
        self.partition_tables.get(&s).copied().unwrap_or(Claim::UNKNOWN)
    }

    pub fn filesystem(&self, f: FilesystemKind) -> Claim {
        self.filesystems.get(&f).copied().unwrap_or(Claim::UNKNOWN)
    }

    pub fn library(&self, f: LibraryFormat) -> Claim {
        self.library_formats.get(&f).copied().unwrap_or(Claim::UNKNOWN)
    }

    /// Library formats this device is documented or inferred to read.
    pub fn readable_libraries(&self) -> Vec<(LibraryFormat, Claim)> {
        self.library_formats.iter().filter(|(_, c)| c.support == Support::Supported).map(|(f, c)| (*f, *c)).collect()
    }

    /// The documents behind an aspect, as one line of citations.
    pub fn source(&self, aspect: &str) -> Option<&str> {
        self.cited.get(aspect).map(String::as_str)
    }

    /// Every (aspect, evidence) pair this profile claims, for checking that
    /// each vendor or community claim cites a document of that kind.
    pub(crate) fn claimed_evidence(&self) -> Vec<(&'static str, Evidence)> {
        let mut out = Vec::new();
        out.extend(self.partition_tables.values().map(|c| ("partition_tables", c.evidence)));
        out.extend(self.filesystems.values().map(|c| ("filesystems", c.evidence)));
        out.extend(self.library_formats.values().map(|c| ("library_formats", c.evidence)));
        out.push(("folder_browsing", self.folder_browsing.evidence));
        out.extend(self.audio.iter().map(|r| ("audio", r.evidence)));
        out.extend(self.audio_list_exhaustive.map(|e| ("audio", e)));
        out.extend(self.usb_power_ma.map(|p| ("usb_power_ma", p.evidence)));
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preset {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub devices: Vec<String>,
    /// Plan an independent backup drive.
    pub redundancy: bool,
}

/// A physical test of one USB model on one player.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub usb_model: String,
    pub device: String,
    pub firmware: Option<String>,
    pub filesystem: FilesystemKind,
    pub scheme: PartitionScheme,
    pub worked: bool,
    pub evidence: Evidence,
    pub tested_on: String,
    pub notes: Option<String>,
}
