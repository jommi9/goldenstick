//! The application state machine (PRD §64). Every failure state has a
//! defined recovery, and illegal jumps (e.g. straight to "Ready to eject"
//! without verifying) are rejected.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AppState {
    WaitingForMedia,
    MediaDetected,
    IdentifyingMedia,
    ScanningMedia,
    AssessmentReady,
    TargetSelection,
    PreparationPlan,
    AwaitingConfirmation,
    AwaitingPrivilege,
    Partitioning,
    Formatting,
    Copying,
    LibraryExportRequired,
    WaitingForExternalExport,
    ValidatingDatabase,
    VerifyingFiles,
    ReadyToEject,
    Complete,
    // Failures
    UnsupportedMedia,
    MediaRemoved,
    FormatFailed,
    CopyFailed,
    DatabaseInvalid,
    ValidationFailed,
    InsufficientSpace,
    PermissionDenied,
    UnknownError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recovery {
    pub label: &'static str,
    pub next: AppState,
}

use AppState::*;

impl AppState {
    pub const ALL: [AppState; 27] = [
        WaitingForMedia,
        MediaDetected,
        IdentifyingMedia,
        ScanningMedia,
        AssessmentReady,
        TargetSelection,
        PreparationPlan,
        AwaitingConfirmation,
        AwaitingPrivilege,
        Partitioning,
        Formatting,
        Copying,
        LibraryExportRequired,
        WaitingForExternalExport,
        ValidatingDatabase,
        VerifyingFiles,
        ReadyToEject,
        Complete,
        UnsupportedMedia,
        MediaRemoved,
        FormatFailed,
        CopyFailed,
        DatabaseInvalid,
        ValidationFailed,
        InsufficientSpace,
        PermissionDenied,
        UnknownError,
    ];

    pub fn is_failure(self) -> bool {
        matches!(
            self,
            UnsupportedMedia
                | MediaRemoved
                | FormatFailed
                | CopyFailed
                | DatabaseInvalid
                | ValidationFailed
                | InsufficientSpace
                | PermissionDenied
                | UnknownError
        )
    }

    /// States during which the drive is being modified. Pulling the drive
    /// here leaves it incomplete, and the UI must say so.
    pub fn is_writing(self) -> bool {
        matches!(self, Partitioning | Formatting | Copying)
    }

    pub fn recovery(self) -> Option<Recovery> {
        let (label, next) = match self {
            UnsupportedMedia => ("Try a different USB", WaitingForMedia),
            MediaRemoved => ("Reinsert the USB", WaitingForMedia),
            FormatFailed => ("Try preparing again", PreparationPlan),
            CopyFailed => ("Resume copying", PreparationPlan),
            DatabaseInvalid => ("Export the library again", LibraryExportRequired),
            ValidationFailed => ("Repair and verify again", PreparationPlan),
            InsufficientSpace => ("Choose fewer playlists or a bigger USB", PreparationPlan),
            PermissionDenied => ("Allow access and try again", AwaitingPrivilege),
            UnknownError => ("Start over", WaitingForMedia),
            _ => return None,
        };
        Some(Recovery { label, next })
    }

    fn allowed(self) -> &'static [AppState] {
        match self {
            WaitingForMedia => &[MediaDetected],
            MediaDetected => &[IdentifyingMedia, UnsupportedMedia],
            IdentifyingMedia => &[ScanningMedia, UnknownError],
            ScanningMedia => &[AssessmentReady, UnsupportedMedia, UnknownError],
            AssessmentReady => &[TargetSelection, PreparationPlan, VerifyingFiles, ReadyToEject, WaitingForMedia],
            TargetSelection => &[PreparationPlan, AssessmentReady],
            PreparationPlan => &[
                AwaitingConfirmation,
                Copying,
                LibraryExportRequired,
                ValidatingDatabase,
                VerifyingFiles,
                TargetSelection,
                InsufficientSpace,
            ],
            AwaitingConfirmation => &[AwaitingPrivilege, PreparationPlan],
            AwaitingPrivilege => &[Partitioning, PermissionDenied, PreparationPlan],
            Partitioning => &[Formatting, FormatFailed],
            Formatting => &[Copying, LibraryExportRequired, ValidatingDatabase, FormatFailed],
            Copying => &[LibraryExportRequired, ValidatingDatabase, CopyFailed, InsufficientSpace],
            LibraryExportRequired => &[WaitingForExternalExport, PreparationPlan],
            WaitingForExternalExport => &[ValidatingDatabase, LibraryExportRequired],
            ValidatingDatabase => &[VerifyingFiles, DatabaseInvalid, LibraryExportRequired],
            VerifyingFiles => &[ReadyToEject, ValidationFailed, AssessmentReady],
            ReadyToEject => &[Complete, VerifyingFiles],
            Complete => &[WaitingForMedia, MediaDetected],
            _ => &[],
        }
    }

    pub fn can_transition(self, to: AppState) -> bool {
        if to == MediaRemoved {
            // A drive can vanish from any state where one is inserted.
            return !matches!(self, WaitingForMedia | MediaRemoved | Complete | UnknownError);
        }
        if to == UnknownError {
            return self != UnknownError;
        }
        if let Some(r) = self.recovery() {
            if r.next == to {
                return true;
            }
        }
        self.allowed().contains(&to)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("can't go from {from:?} to {to:?}")]
pub struct InvalidTransition {
    pub from: AppState,
    pub to: AppState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Machine {
    state: AppState,
    /// Set if the drive vanished mid-write; the next insert must not be
    /// treated as ready.
    interrupted_write: bool,
}

impl Default for Machine {
    fn default() -> Self {
        Machine { state: WaitingForMedia, interrupted_write: false }
    }
}

impl Machine {
    pub fn state(&self) -> AppState {
        self.state
    }

    pub fn interrupted_write(&self) -> bool {
        self.interrupted_write
    }

    pub fn go(&mut self, to: AppState) -> Result<AppState, InvalidTransition> {
        if !self.state.can_transition(to) {
            return Err(InvalidTransition { from: self.state, to });
        }
        if to == MediaRemoved && self.state.is_writing() {
            self.interrupted_write = true;
        }
        if to == Complete {
            self.interrupted_write = false;
        }
        self.state = to;
        Ok(to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_failure_has_a_recovery() {
        for s in AppState::ALL {
            assert_eq!(s.is_failure(), s.recovery().is_some(), "{s:?}");
            if let Some(r) = s.recovery() {
                assert!(s.can_transition(r.next), "{s:?} -> {:?}", r.next);
            }
        }
    }

    #[test]
    fn every_state_is_reachable_and_can_move_on() {
        use std::collections::HashSet;
        let mut seen: HashSet<AppState> = HashSet::from([WaitingForMedia]);
        let mut frontier = vec![WaitingForMedia];
        while let Some(s) = frontier.pop() {
            for t in AppState::ALL {
                if s.can_transition(t) && seen.insert(t) {
                    frontier.push(t);
                }
            }
        }
        assert_eq!(seen.len(), AppState::ALL.len());
        for s in AppState::ALL {
            assert!(AppState::ALL.iter().any(|t| s.can_transition(*t)), "{s:?} is a dead end");
        }
    }

    #[test]
    fn happy_path_and_guard_rails() {
        let mut m = Machine::default();
        for s in [
            MediaDetected,
            IdentifyingMedia,
            ScanningMedia,
            AssessmentReady,
            TargetSelection,
            PreparationPlan,
            AwaitingConfirmation,
            AwaitingPrivilege,
            Partitioning,
            Formatting,
            LibraryExportRequired,
            WaitingForExternalExport,
            ValidatingDatabase,
            VerifyingFiles,
            ReadyToEject,
            Complete,
        ] {
            m.go(s).unwrap();
        }
        // Can't claim ready without verifying, or erase without confirming.
        let mut m = Machine::default();
        m.go(MediaDetected).unwrap();
        assert!(m.go(ReadyToEject).is_err());
        let mut m = Machine { state: PreparationPlan, interrupted_write: false };
        assert!(m.go(Partitioning).is_err());
        assert!(m.go(AwaitingConfirmation).is_ok());
    }

    #[test]
    fn pulling_the_drive_mid_write_is_remembered() {
        let mut m = Machine { state: Copying, interrupted_write: false };
        m.go(MediaRemoved).unwrap();
        assert!(m.interrupted_write());
        m.go(WaitingForMedia).unwrap();
        assert!(m.interrupted_write());
    }
}
