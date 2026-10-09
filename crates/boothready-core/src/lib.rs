//! BoothReady core engine.
//!
//! Everything here is platform-independent and side-effect free except where
//! a function takes an explicit path or handle to operate on. OS-specific
//! device discovery lives in `boothready-platform`; privileged disk writes
//! are executed by `boothready-helper`.

#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod audio;
pub mod copy;
#[cfg(feature = "fixtures")]
pub mod demo;
pub mod drive;
pub mod format;
pub mod identify;
pub mod io;
pub mod library;
pub mod manifest;
pub mod media;
pub mod planner;
pub mod privileged;
pub mod rules;
pub mod scan;
pub mod state;
#[cfg(feature = "sqlite")]
pub mod store;
pub mod verify;

#[cfg(test)]
pub(crate) mod testutil;
