//! Export providers: complete player libraries built from local audio.
//!
//! `docs/EXPORT-PROVIDERS.md` describes the contract. The first provider is
//! the rekordbox Device Library writer in [`device_library`], which produces
//! a `device_library_tree` artifact: `export.pdb`, the analysis files under
//! `PIONEER/USBANLZ` and the audio under `Contents`. With
//! `ExportRequest::one_library` it adds `exportLibrary.db` from the same rows,
//! which makes it a `dual_library_tree`. Providers only ever
//! write into the local folder they are given; copying a finished tree onto
//! a USB stays with the existing copy and verification flow.

#[cfg(feature = "device-library-writer")]
pub mod analysis_cache;
#[cfg(feature = "device-library-writer")]
pub mod device_library;
#[cfg(feature = "device-library-writer")]
pub mod playlists;

#[cfg(all(test, feature = "device-library-writer"))]
mod tests;
