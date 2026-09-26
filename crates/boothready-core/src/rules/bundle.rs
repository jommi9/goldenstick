//! Signed compatibility-rule updates.
//!
//! Rules ship inside the app and can be replaced by a newer signed bundle
//! without an app update. A bundle is rejected unless it is signed by a key
//! the app trusts and carries a higher version than the rules in use, so an
//! attacker can neither forge rules nor roll them back.

use super::{RulesError, Ruleset};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

pub const BUNDLE_FORMAT: &str = "boothready-rules/1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RulesBundle {
    pub format: String,
    pub key_id: String,
    /// Base64 of the exact ruleset JSON bytes that were signed.
    pub payload: String,
    /// Base64 Ed25519 signature over the decoded payload.
    pub signature: String,
}

#[derive(Debug, Clone)]
pub struct TrustedKey {
    pub id: String,
    pub key: VerifyingKey,
}

#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    #[error("not a BoothReady rules bundle")]
    Format,
    #[error("signed by an unknown key '{0}'")]
    UnknownKey(String),
    #[error("signature does not match")]
    BadSignature,
    #[error("bundle version {got} is not newer than installed version {have}")]
    Rollback { got: u64, have: u64 },
    #[error(transparent)]
    Rules(#[from] RulesError),
}

pub fn verify_bundle(bytes: &[u8], trusted: &[TrustedKey], installed_version: u64) -> Result<Ruleset, BundleError> {
    let bundle: RulesBundle = serde_json::from_slice(bytes).map_err(|_| BundleError::Format)?;
    if bundle.format != BUNDLE_FORMAT {
        return Err(BundleError::Format);
    }
    let key =
        trusted.iter().find(|k| k.id == bundle.key_id).ok_or_else(|| BundleError::UnknownKey(bundle.key_id.clone()))?;
    let payload = B64.decode(&bundle.payload).map_err(|_| BundleError::Format)?;
    let sig_bytes = B64.decode(&bundle.signature).map_err(|_| BundleError::Format)?;
    let sig = Signature::from_slice(&sig_bytes).map_err(|_| BundleError::BadSignature)?;
    key.key.verify(&payload, &sig).map_err(|_| BundleError::BadSignature)?;
    // Only parse after the signature checks out.
    let text = std::str::from_utf8(&payload).map_err(|_| BundleError::Format)?;
    let rules = Ruleset::from_json(text)?;
    if rules.version <= installed_version {
        return Err(BundleError::Rollback { got: rules.version, have: installed_version });
    }
    Ok(rules)
}

/// Build a bundle. Used by the release tooling and tests.
pub fn sign_bundle(ruleset_json: &str, key_id: &str, key: &ed25519_dalek::SigningKey) -> RulesBundle {
    use ed25519_dalek::Signer;
    let sig = key.sign(ruleset_json.as_bytes());
    RulesBundle {
        format: BUNDLE_FORMAT.into(),
        key_id: key_id.into(),
        payload: B64.encode(ruleset_json.as_bytes()),
        signature: B64.encode(sig.to_bytes()),
    }
}
