//! BoothReady core engine.
//!
//! Everything here is platform-independent and side-effect free except where
//! a function takes an explicit path or handle to operate on. OS-specific
//! device discovery lives in `boothready-platform`; privileged disk writes
//! are executed by `boothready-helper`.

pub mod io;
pub mod audio;
pub mod library;
pub mod media;

#[cfg(test)]
pub(crate) mod testutil;
