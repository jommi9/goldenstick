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
use std::collections::HashSet;

pub const BUILTIN_RULESET: &str = include_str!("../../data/ruleset.json");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ruleset {
    pub schema: u32,
    /// Monotonic; signed updates must carry a higher number.
    pub version: u64,
    pub published: String,
    pub review_status: String,
    #[serde(default)]
    pub review_note: Option<String>,
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
        let r: Ruleset = serde_json::from_str(s)?;
        r.validate()?;
        Ok(r)
    }

    fn validate(&self) -> Result<(), RulesError> {
        if self.schema != 1 {
            return Err(RulesError::Invalid(format!("unsupported schema {}", self.schema)));
        }
        let mut ids = HashSet::new();
        for d in &self.devices {
            if !ids.insert(d.id.as_str()) {
                return Err(RulesError::Invalid(format!("duplicate device id {}", d.id)));
            }
            if d.audio.iter().any(|r| r.codecs.is_empty()) {
                return Err(RulesError::Invalid(format!("{}: audio rule with no codecs", d.id)));
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
