# Architecture

## Layers

```
 app/ (Tauri 2 + TypeScript)          boothready CLI
          │  commands, events               │
          ▼                                 ▼
 ┌────────────────────── boothready-core ──────────────────────┐
 │ media      partition map + filesystem from raw sectors       │
 │ audio      header probes (WAV, AIFF, FLAC, MP3, MP4, Ogg)    │
 │ library    rekordbox PDB reader, OneLibrary, Engine, Serato  │
 │ rules      device profiles, evidence, assessment, "why"      │
 │ planner    Main / Legacy Rescue / Backup, capacity fitting   │
 │ format     MBR + FAT32 builder (fatfs)                       │
 │ verify     quick/full read-back, manifest, fingerprints      │
 │ identify   USB catalog matching with confidence              │
 │ state      PRD §64 state machine                             │
 │ store      SQLite: known drives, profiles, history           │
 └──────────────────────────────────────────────────────────────┘
          │                                 │
 boothready-platform                  boothready-helper (elevated)
 list, watch, unmount, eject          structured requests only
```

The core has no OS dependencies. Everything that touches a real device goes through `boothready-platform`, and everything that writes to a whole device goes through `boothready-helper`.

## Reading a drive

1. The platform layer reports physical devices with USB identity (VID, PID, serial, bMaxPower) and volumes, keyed by physical device rather than drive letter or mount point.
2. `identify` maps descriptors to a catalog entry and reports Exact, Strong, Probable or Unknown. The picture shows a product family and is never used as proof of the controller inside.
3. The partition layout comes from a raw read when the process may read the device. Otherwise it comes from what the OS reports, which lacks the dirty bit and the MBR type byte, and the UI says which source it used. Every backend marks EFI system partitions from their partition type, because macOS puts a FAT32 one in front of the data partition on every GPT drive and a layout built from the OS's volume list would otherwise judge an exFAT stick as FAT32.
4. `library::scan_libraries` finds rekordbox, Engine DJ and Serato data, resolving paths case-insensitively the way FAT and exFAT players do. It never writes to the drive, and Engine databases are opened with SQLite's `immutable=1`.
5. `scan::scan_audio` probes every audio file and streams progress. The parsers bound every length against the real file size and are mutation-fuzzed.
6. `rules::assess_drive` turns all of that into one result per device and layer.

## Compatibility rules

`data/ruleset.json` is the only place hardware knowledge lives. A claim reads like `"supported/vendor"`, meaning a support level from supported, unsupported, unreliable and unknown combined with an evidence level from vendor, lab, community, inferred and unknown. A claim that asserts support with unknown evidence fails to load.

Each layer contributes a verdict, and the device verdict is the worst of them:

| Verdict | Meaning |
|---|---|
| Ready | Every layer passes on vendor or lab evidence |
| Expected to work | Passes, but some evidence is inferred or community-reported |
| Partly ready | Some tracks won't play, or the export is out of date |
| Not enough data | A layer has no reliable data for this device |
| At risk | The vendor warns it may not work (GPT on a CDJ-3000, power draw, dirty filesystem) |
| Fix needed | Documented not to work |

An untested USB model doesn't block Ready, because the format decides compatibility, but the physical column always shows that the exact stick hasn't been tested. OneLibrary databases are encrypted, so their presence can be checked but their contents can't, and any device that relies on them tops out at Expected to work.

Rule updates arrive as signed bundles (`rules::bundle`). Ed25519 signatures are checked against keys compiled into the app before the payload is even parsed, and a bundle whose version isn't higher than the installed one is rejected.

## Erasing a drive safely

This follows PRD §25.

- `privileged::eligibility` only offers removable USB devices of 2 TB or less that don't host a system mount.
- The confirmation screen shows the drive's picture, capacity, volume label, the last four characters of its serial and what's on it, and the button names the drive.
- The UI sends the helper a `DeviceFingerprint` (device ID, size, serial, VID/PID, volume IDs) and a token derived from it. The helper re-enumerates devices itself and refuses if anything differs, which covers a user swapping sticks between confirming and erasing and a drive reformatted elsewhere in the meantime.
- Requests are an enum with no field that carries a path, a command or free text, apart from a volume label restricted to `[A-Z0-9_-]{1,11}`.
- Raw access is exclusive: `O_EXCL` on Linux fails if anything is mounted, macOS writes to `/dev/rdiskN` after `diskutil unmountDisk`, and Windows locks and dismounts every volume and holds those locks while writing.
- After formatting, the helper re-reads the device and fails loudly unless it finds MBR, the requested filesystem and the requested label.
- A manifest in `.boothready/` is written as "in progress" before the export step and only marked complete after a passing verification, so a stick pulled halfway through is never shown as ready.

The helper is built without SQLite and without the demo platform. Demo and test builds run the same request handler in-process against disk images, so the erase path that ships is the one the tests exercise.

Elevation is per operation for now: `pkexec` on Linux, `osascript ... with administrator privileges` on macOS and `ShellExecuteEx` with `runas` on Windows. A release should install the helper as a privileged service (SMAppService on macOS, a Windows service) and keep the same request protocol.

## Formatting

FAT32 is built in pure Rust so that Windows' 32 GB FAT32 limit doesn't apply. The MBR layout starts at 1 MiB, the first and last MiB are zeroed so no GPT header survives, and cluster sizes follow Microsoft's defaults but shrink on small volumes so they stay FAT32. `fatfs` writes the filesystem, and BoothReady fills in the hidden-sectors field and the FSInfo free count the way Windows does. Tests check the output with `fsck.fat`, `sfdisk`, `sgdisk` and mtools, including a 64 GB volume, and through an alignment-checking device.

exFAT has no mature pure-Rust formatter, so it uses `diskutil eraseDisk` on macOS, a generated `diskpart` script on Windows and `mkfs.exfat` on Linux.

## Verification

Quick mode checks the layout and libraries and reads an even spread of 24 audio files. Full mode reads every audio file end to end and compares BLAKE3 hashes when the manifest has them. Progress is reported in bytes, and an estimate of time remaining appears only after 5 seconds and 5 % of the work. The result is stored with a content fingerprint built from every file's path, size and modification time, and any later change invalidates it.

## The desktop app

The Tauri commands in `app/src-tauri/src/lib.rs` are thin wrappers that run engine code on blocking threads and stream progress as events. The UI in `app/src` is plain TypeScript with no framework. In a browser without Tauri it loads `mock-data.json`, which `cargo run -p boothready-app --example mock_snapshot` generates by running the real engine over the demo drives, so the mock can't drift from the real DTOs. `npm run screenshots` walks the full two-USB flow in headless Chromium.

## What still needs verifying

- **Rules.** `ruleset.json` is seed data. Every `vendor` claim needs checking against current manuals and firmware notes, and the OneLibrary file name (`PIONEER/rekordbox/exportLibrary.db`) needs confirming against a rekordbox 7.2.x export. No claim is lab verified.
- **Hardware.** The macOS and Windows backends parse sample `diskutil`, `ioreg` and IOCTL output in tests, and CI runs them against each runner's own disks, but they haven't enumerated, erased or ejected a real USB stick. That needs doing with a set of sticks before any public build.
- **APFS sticks on macOS.** An APFS partition is recognised from its partition type, but its volumes live on a synthesized container disk that the backend doesn't read yet, so the app can't show an APFS stick's volume name or files on the erase confirmation.
- **FAT32 output on players.** `fatfs` uses 8 reserved sectors and doesn't align the data region to clusters the way Windows does. The result is valid and fsck-clean, but it has to be tried on CDJs from each generation.
- **Engine DJ paths.** Track paths in `m.db` are resolved against the database folder, the `Engine Library` folder and the volume root, because the exact base isn't documented. It should be checked against a real Engine DJ 4.x export.
- **Product images.** The app draws illustrations. Licensed photography (PRD §96) is still open.
- **Legal review** of reading vendor database formats and of the dependency licences (PRD §95). The helper and the CLI link only permissively licensed crates (MIT, Apache-2.0, BSD, ISC, Zlib, CC0), plus SQLite, which is public domain. Through Tauri the desktop app also links `cssparser`, `selectors`, `dtoa-short` and `option-ext`, which are MPL-2.0. That licence is file-level copyleft, so it allows a proprietary app but obliges publishing any changes to those files, and it belongs in the review. rekordcrate (MPL-2.0) is only a dev-dependency, used to cross-check the PDB reader.
