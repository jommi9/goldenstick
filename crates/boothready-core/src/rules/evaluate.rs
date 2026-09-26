use super::model::{DeviceProfile, Evidence};
use crate::audio::{AudioInfo, Codec};
use serde::{Deserialize, Serialize};

/// Whether one track will play on one device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum TrackVerdict {
    Supported { evidence: Evidence },
    Unsupported { reason: String, evidence: Evidence },
    Unknown { reason: String },
}

impl TrackVerdict {
    pub fn is_supported(&self) -> bool {
        matches!(self, TrackVerdict::Supported { .. })
    }
}

fn khz(rate: u32) -> String {
    if rate % 1000 == 0 {
        format!("{} kHz", rate / 1000)
    } else {
        format!("{:.1} kHz", rate as f64 / 1000.0)
    }
}

pub fn evaluate_audio(device: &DeviceProfile, info: &AudioInfo) -> TrackVerdict {
    if info.codec == Codec::Protected {
        return TrackVerdict::Unsupported {
            reason: "DRM-protected file; only Apple's own software can play it".into(),
            evidence: Evidence::Inferred,
        };
    }
    let rules: Vec<_> = device
        .audio
        .iter()
        .filter(|r| r.codecs.contains(&info.codec))
        .filter(|r| r.containers.as_ref().is_none_or(|c| c.contains(&info.container)))
        .collect();
    if rules.is_empty() {
        return match device.audio_list_exhaustive {
            Some(ev) => TrackVerdict::Unsupported {
                reason: format!("{} files are not supported", info.codec.label()),
                evidence: ev,
            },
            None => TrackVerdict::Unknown {
                reason: format!("no compatibility data for {} on the {}", info.codec.label(), device.model),
            },
        };
    }
    let mut first_failure: Option<(String, Evidence)> = None;
    let mut unknown: Option<String> = None;
    for r in &rules {
        let rate_ok = match (&r.sample_rates, info.sample_rate) {
            (None, _) => Some(true),
            (Some(list), Some(rate)) => Some(list.contains(&rate)),
            (Some(_), None) => None,
        };
        let depth_ok = match (&r.bit_depths, info.bit_depth) {
            (None, _) | (_, None) => true,
            (Some(list), Some(d)) => list.contains(&d),
        };
        match rate_ok {
            Some(true) if depth_ok => return TrackVerdict::Supported { evidence: r.evidence },
            None => unknown = Some("sample rate could not be read".into()),
            Some(rate_ok) => {
                if first_failure.is_none() {
                    let reason = if !rate_ok {
                        let max = r.sample_rates.as_ref().and_then(|l| l.iter().max()).copied().unwrap_or(0);
                        let rate = info.sample_rate.unwrap_or(0);
                        if rate > max {
                            format!("{} is above the {}'s limit of {}", khz(rate), device.model, khz(max))
                        } else {
                            format!("{} sample rate is not supported", khz(rate))
                        }
                    } else {
                        let allowed: Vec<String> = r.bit_depths.iter().flatten().map(|d| d.to_string()).collect();
                        format!(
                            "{}-bit {} is not supported ({}-bit only)",
                            info.bit_depth.unwrap_or(0),
                            info.codec.label(),
                            allowed.join("/")
                        )
                    };
                    first_failure = Some((reason, r.evidence));
                }
            }
        }
    }
    if let Some(reason) = unknown {
        return TrackVerdict::Unknown { reason };
    }
    let (reason, evidence) = first_failure.expect("at least one rule was evaluated");
    TrackVerdict::Unsupported { reason, evidence }
}
