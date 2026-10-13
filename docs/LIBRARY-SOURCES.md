# Library sources

## Scanning a source

The Library screen and `boothready library scan <folder>` list the audio in a folder without changing it.

```sh
boothready library scan /absolute/music-folder --preset unknown_club
```

A scan walks the folder, skips symlinks and operating system litter, and reads the header of each audio file that is on this computer. A file the cloud client has not downloaded is recognised from its metadata and is never opened. It is counted with its name and size, and its format stays unchecked. Listing a folder the cloud client has not expanded yet can make the client fetch that folder's file list.

Each track gets one state for the selected players: ready, still in the cloud, blocked by at least one player, no data for at least one player, or unreadable. The text output prints counts only. `--json` prints every track, which includes private file names.

The app saves the source list and each source's last scan under `library/` in its own data folder. Removing a source deletes those records and leaves the folder alone.

A scan and staging are separate steps. Scanning reads headers and downloads nothing. Staging, described below, reads every byte, which makes a cloud client download the files.

# Local library staging

`boothready library stage` creates a verified local collection from a selected directory. The source can be a local folder or a Google Drive for desktop folder that the user already accesses through their operating system.

```sh
boothready library stage /absolute/source-folder /absolute/existing-parent/collection --kind local
boothready library stage /absolute/drive-folder /absolute/existing-parent/collection --kind google-drive-desktop
boothready library verify /absolute/existing-parent/collection
```

The source kind records provenance. BoothReady does not sign into Google Drive, discover accounts or call its API. Reading a cloud placeholder may cause the installed Drive client to download its contents. A failed read aborts staging. These commands have been tested with generated directories; live Drive hydration remains unverified.

## Storage boundary

The CLI requires the destination to belong to a known system-volume mount reported by the platform backend, and refuses USB or removable devices. On Unix it compares filesystem device IDs to reject an unknown nested mount, and accepts APFS firmlink paths when the known mount has the same filesystem device ID. Windows rejects network path prefixes and all reparse points, including junctions. This also excludes Windows cloud placeholders until safe hydration handling is implemented. This conservative policy can reject a valid local folder when platform enumeration is incomplete. The destination's parent must already exist, while the collection folder must be new or contain the identical staging plan from a previous run.

Before copying, the CLI checks free space against the entire collection, rounded to filesystem allocation units, plus an allowance for directory and manifest data. This conservative check can refuse a resume even when most files already exist. Other processes can still consume space after this check.

The core API accepts paths, so other callers must enforce this storage boundary before calling it. Ordinary files in a local collection are written without raw disk access. USB export remains a separate operation with its existing confirmation requirements.

## Collection interface

`boothready_core::staging` exposes `inventory`, `stage` and `verify_collection`. `SourceKind` distinguishes `Local` from `GoogleDriveDesktop`. A `Collection` has schema version 1, source kind, the absolute source path, a content-derived revision and ordered `SourceFile` records. Each record contains a relative path, byte count and BLAKE3 hash. The revision hashes the JSON serialization of those records, so it is a local snapshot identifier rather than a Google Drive revision or a portable cross-platform identity.

A collection directory contains `collection.json`, staged content under `files/`, and `complete.json` after verification succeeds. `.work/` holds temporary copies. Every source file is fully read when the plan is built, every copied file is hashed from the destination, and the source inventory is checked again before completion. All regular files in the selected folder are included. Audio analysis and DJ database generation belong to later stages.

`verify_collection` checks both manifest records and rehashes the staged tree without accessing the original source. An exporter must call it before using the collection because a completion marker cannot detect later file changes by itself. Pass the relative records and revision to an exporter with a separate local `files/` root. The private source path has no role in export providers.

## Resume and failure behavior

Rerunning the stage command resumes the same immutable source snapshot, rehashing completed files before skipping them. Changes to the source require a fresh collection directory. Existing destination files are never overwritten, and a corrupt completed file causes an error. Publication uses a hard link from a fully synced temporary file, which fails if the destination already exists. Filesystems that cannot create hard links are unsupported.

Cancellation is checked during copying. A stopped process can leave temporary files in `.work/`, which subsequent runs ignore while preserving them. Such files can consume space until the user removes the abandoned collection. A process stopped while writing a manifest can leave a corrupt manifest; recovery uses a fresh destination because BoothReady will not overwrite it. Resume does not repair corrupted files or continue within an unfinished file.

Symbolic links and parent path components are rejected, including links in the selected roots. Use a resolved path when the operating system exposes a convenience symlink such as `/tmp` on macOS. Source and destination trees cannot overlap. Run one staging operation per destination and keep both directory trees and mounts stable during the operation. This first version does not provide transactional filesystem snapshots or protection against another process replacing directories while a copy is running.

## Privacy

Collection manifests contain private absolute paths and file names. Staged files and their manifests belong in a private local directory outside the repository. `--json` emits that same private metadata. Tests use synthetic content only; no personal library, Drive account or real USB is required.
