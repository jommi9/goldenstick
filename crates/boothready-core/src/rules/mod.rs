//! Data-driven compatibility rules.
//!
//! Nothing about specific hardware is hard-coded outside `data/ruleset.json`
//! (or a signed update replacing it).

mod assess;
pub mod bundle;
mod evaluate;
mod model;
mod recommend;

pub use assess::*;
pub use evaluate::{evaluate_audio, TrackVerdict};
pub use model::*;
pub use recommend::{recommend_format, FormatRecommendation};

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

pub const BUILTIN_RULESET: &str = include_str!("../../data/ruleset.json");

/// Version 2 moved sources from free text to a table of dated, linked
/// references that every vendor or community claim must cite.
pub const SCHEMA: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ruleset {
    pub schema: u32,
    /// Monotonic; signed updates must carry a higher number.
    pub version: u64,
    pub published: String,
    /// "seed" (unchecked), "desk_reviewed" (checked against documents) or
    /// "lab_verified".
    pub review_status: String,
    #[serde(default)]
    pub review_note: Option<String>,
    #[serde(default)]
    pub references: BTreeMap<String, Reference>,
    pub devices: Vec<DeviceProfile>,
    pub presets: Vec<Preset>,
    #[serde(default)]
    pub observations: Vec<Observation>,
}

#[derive(Debug, thiserror::Error)]
pub enum RulesError {
    #[error("rules file is not valid: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("rules file is inconsistent: {0}")]
    Invalid(String),
}

impl Ruleset {
    pub fn builtin() -> Ruleset {
        Ruleset::from_json(BUILTIN_RULESET).expect("built-in ruleset must be valid")
    }

    pub fn from_json(s: &str) -> Result<Ruleset, RulesError> {
        let mut r: Ruleset = serde_json::from_str(s)?;
        r.validate()?;
        for d in &mut r.devices {
            d.cited = d
                .sources
                .iter()
                .map(|(aspect, ids)| {
                    let line = ids.iter().map(|id| r.references[id].cite()).collect::<Vec<_>>().join("; ");
                    (aspect.clone(), line)
                })
                .collect();
        }
        Ok(r)
    }

    fn validate(&self) -> Result<(), RulesError> {
        let invalid = |msg: String| Err(RulesError::Invalid(msg));
        if self.schema != SCHEMA {
            return invalid(format!("unsupported schema {}", self.schema));
        }
        for (id, r) in &self.references {
            if !r.url.starts_with("https://") {
                return invalid(format!("reference {id} needs an https URL"));
            }
            if !matches!(r.kind, Evidence::Vendor | Evidence::Community | Evidence::Lab) {
                return invalid(format!("reference {id} must be vendor, community or lab evidence"));
            }
        }
        // A claim is only as strong as the document behind it: "vendor" needs
        // a manufacturer document for that aspect, "community" a forum report.
        let cites = |ids: &[String], kind: Evidence| ids.iter().any(|id| self.references[id].kind == kind);
        let mut ids = HashSet::new();
        for d in &self.devices {
            if !ids.insert(d.id.as_str()) {
                return invalid(format!("duplicate device id {}", d.id));
            }
            if d.audio.iter().any(|r| r.codecs.is_empty()) {
                return invalid(format!("{}: audio rule with no codecs", d.id));
            }
            let quirk_refs = d.quirks.iter().flat_map(|q| &q.sources);
            if let Some(id) =
                d.sources.values().flatten().chain(quirk_refs).find(|id| !self.references.contains_key(*id))
            {
                return invalid(format!("{} cites unknown reference {id}", d.id));
            }
            for (aspect, ev) in d.claimed_evidence() {
                if matches!(ev, Evidence::Vendor | Evidence::Community | Evidence::Lab)
                    && !cites(d.sources.get(aspect).map(Vec::as_slice).unwrap_or_default(), ev)
                {
                    return invalid(format!("{}: {aspect} claims {} evidence without citing any", d.id, ev.label()));
                }
            }
            for q in &d.quirks {
                if matches!(q.evidence, Evidence::Vendor | Evidence::Community | Evidence::Lab)
                    && !cites(&q.sources, q.evidence)
                {
                    return invalid(format!(
                        "{}: quirk {} claims {} evidence without citing any",
                        d.id,
                        q.id,
                        q.evidence.label()
                    ));
                }
            }
        }
        for p in &self.presets {
            for id in &p.devices {
                if !ids.contains(id.as_str()) {
                    return Err(RulesError::Invalid(format!("preset {} references unknown device {id}", p.id)));
                }
            }
        }
        for o in &self.observations {
            if !ids.contains(o.device.as_str()) {
                return Err(RulesError::Invalid(format!("observation references unknown device {}", o.device)));
            }
        }
        Ok(())
    }

    pub fn device(&self, id: &str) -> Option<&DeviceProfile> {
        self.devices.iter().find(|d| d.id == id)
    }

    pub fn preset(&self, id: &str) -> Option<&Preset> {
        self.presets.iter().find(|p| p.id == id)
    }

    /// Resolve device IDs, silently skipping unknown ones.
    pub fn devices_by_id<'a>(&'a self, ids: &[String]) -> Vec<&'a DeviceProfile> {
        ids.iter().filter_map(|id| self.device(id)).collect()
    }

    /// Fuzzy hardware search for people who know the gear by sight or
    /// nickname rather than model number ("old nexus", "big touch screen").
    pub fn search(&self, query: &str) -> Vec<&DeviceProfile> {
        let norm =
            |s: &str| s.to_lowercase().replace(['-', '_', '+'], " ").split_whitespace().collect::<Vec<_>>().join(" ");
        let compact = |s: &str| norm(s).replace(' ', "");
        let q = norm(query);
        if q.is_empty() {
            return self.devices.iter().collect();
        }
        let qc = compact(query);
        let tokens: Vec<&str> = q.split(' ').collect();
        let mut scored: Vec<(u32, &DeviceProfile)> = self
            .devices
            .iter()
            .filter_map(|d| {
                let mut score = 0u32;
                let model = compact(&d.model);
                if model == qc || d.aliases.iter().any(|a| compact(a) == qc) {
                    score += 100;
                } else if model.contains(&qc) || d.aliases.iter().any(|a| compact(a).contains(&qc)) {
                    score += 60;
                }
                for t in &d.search_terms {
                    let t = norm(t);
                    if t == q {
                        score += 50;
                    } else if tokens.iter().all(|tok| t.contains(tok)) {
                        score += 30;
                    }
                }
                if norm(&d.manufacturer).contains(&q) || d.family.contains(&q) {
                    score += 20;
                }
                (score > 0).then_some((score, d))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.released.cmp(&b.1.released)));
        scored.into_iter().map(|(_, d)| d).collect()
    }
}

#[cfg(test)]
mod tests;
