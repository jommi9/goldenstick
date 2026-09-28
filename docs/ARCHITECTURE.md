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
 │ format     MBR + FAT32 builder (mkfs.fat geometry)           │
 │ copy       drive-to-drive copy, resumable, hash journal      │
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

Evidence has to point at something. The ruleset holds a table of references, each with a title, an https link, the date it was read and whether it's a vendor document or a community report, and every device lists the references behind each aspect (partition tables, filesystems, library formats, folder browsing, audio, USB power). A ruleset where a vendor claim cites no vendor document for that aspect, or a community claim cites no forum report, fails to load, and so does a signed update with the same gap. Claims no document covers are marked inferred, and the UI says so. `boothready rules device <id>` prints a profile with the documents it cites.

Quirks are notes that ride along with a device verdict, like the CDJ-3000 needing firmware 1.20 for exFAT. A quirk can be limited to drives with a given filesystem or partition scheme, so an exFAT warning never shows up for a FAT32 stick.

The current ruleset was checked on 27 September 2026 against AlphaTheta's help center articles, product pages and notices, Denon DJ and Engine DJ support articles and user guides, the Mixxx manual, and threads on the Pioneer DJ and Engine DJ community forums. Where a document confirms FAT32 support it describes MBR-partitioned sticks and names the GUID partition map as the exception, so it also backs the MBR claim.

Each layer contributes a verdict, and the device verdict is the worst of them:

| Verdict | Meaning |
|---|---|
| Ready | Every layer passes on vendor or lab evidence |
| Expected to work | Passes, but some evidence is inferred or community-reported |
| Partly ready | Some tracks won't play, or the export is out of date |
| Not enough data | A layer has no reliable data for this device |
| At risk | The vendor warns it may not work (power draw, a dirty filesystem, a partition that isn't first) |
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

Elevation is per operation for now: `pkexec` on Linux, `osascript ... with administrator privileges` on macOS and `ShellExecuteEx` with `runas` on Windows. The installers ship the helper next to the app's executable, which is where the app looks for it, and the Windows installer installs per machine so the helper sits in a folder only administrators can change. A release should install the helper as a privileged service (SMAppService on macOS, a Windows service) and keep the same request protocol.

## Formatting

FAT32 is built in pure Rust so that Windows' 32 GB FAT32 limit doesn't apply. The MBR layout starts at 1 MiB, the first and last MiB are zeroed so no GPT header survives, and cluster sizes follow Microsoft's defaults but shrink on small volumes so they stay FAT32. `format::format_fat32` writes the volume itself from Microsoft's FAT specification, with the geometry mkfs.fat uses: 32 reserved sectors, or one cluster when clusters are bigger, and FATs rounded up to whole clusters, so the FATs and the data region all start on cluster boundaries. The boot sector carries the hidden-sector count and has its backup in sector 6, FSInfo holds a correct free count, and the label is in both the boot sector and the root directory. Tests compare the geometry with mkfs.fat's for volumes from 100 MB to 256 GB and check the output with `fsck.fat`, `sfdisk`, `sgdisk` and mtools, including a 64 GB volume, and through an alignment-checking device.

exFAT has no mature pure-Rust formatter, so it uses `diskutil eraseDisk` on macOS, a generated `diskpart` script on Windows and `mkfs.exfat` on Linux.

## Copying between drives

`copy` puts one drive's contents onto another, which is how a kit's Backup gets filled from a verified Main without a second export. rekordbox and Engine DJ store track paths relative to the volume root, so copying the whole tree, including the `PIONEER/USBANLZ` analysis files, gives a working export on the second drive. BoothReady never writes a partial DJ database (PRD §77), so when the tree doesn't fit, the answer is another export from the DJ software.

Before anything is written, the plan checks free space in whole clusters, FAT32's 4 GB file limit, names FAT, exFAT or Windows can't store, and that the destination holds nothing this copy didn't put there, so two libraries never get mixed. Each file is written under a temporary name, given the source's modification time and renamed into place, and a journal of finished files with their BLAKE3 hashes is flushed every few seconds. An interrupted copy resumes from the journal (PRD §61), running the same copy again copies nothing (PRD §71), and files an earlier run wrote that the source no longer has are removed. When the copy finishes, the hashes go into the destination's manifest, and full verification reads every file back against them.

The app offers a copy on the export step when another connected drive holds a DJ library and a passing verification that still matches its contents, and only between drives with compatible jobs: a Legacy Rescue needs its own conservative export.

## Verification

Quick mode checks the layout and libraries and reads an even spread of 24 audio files. Full mode reads every audio file end to end and compares BLAKE3 hashes when the manifest has them. Progress is reported in bytes, and an estimate of time remaining appears only after 5 seconds and 5 % of the work. The result is stored with a content fingerprint built from every file's path, size and modification time, and any later change invalidates it.

## The desktop app

The Tauri commands in `app/src-tauri/src/lib.rs` are thin wrappers that run engine code on blocking threads and stream progress as events. The UI in `app/src` is plain TypeScript with no framework. In a browser without Tauri it loads `mock-data.json`, which `cargo run -p boothready-app --example mock_snapshot` generates by running the real engine over the demo drives, so the mock can't drift from the real DTOs. `npm run screenshots` walks the full two-USB flow in headless Chromium.

## What still needs verifying

- **Rules.** The ruleset cites a document for every vendor and community claim, but the documents were read through web search excerpts because this project's build environment couldn't open AlphaTheta's, Denon's or Reddit's sites directly. The links should be opened and the claims re-read against the full pages, starting with the per-model audio details. Reddit hasn't been searched at all. [`RULES-REVIEW.md`](RULES-REVIEW.md) describes that pass, with a script that fetches every cited page and the relevant Reddit threads on a machine with open internet. The CDJ-900NXS, CDJ-2000NXS and XDJ-700 audio lists, the XDJ-XZ's ALAC support and the new CDJ-1500X and XDJ-AN audio specs are still inferred, and so is GPT behaviour on most players. No claim is lab verified.
- **Hardware.** The macOS and Windows backends parse sample `diskutil`, `ioreg` and IOCTL output in tests, and CI runs them against each runner's own disks, but they haven't enumerated, erased or ejected a real USB stick. [`FIELD-TEST.md`](FIELD-TEST.md) is the checklist for doing that. A diagnostics report (`boothready_platform::diagnostics`) records the raw `diskutil`/`ioreg` output on a Mac, and the IOCTL buffers, volume extents, USB instance IDs and PowerShell's disk view on Windows. A Mac report replays through the same code as the live backend on any OS, so a misread stick becomes a test fixture.
- **APFS sticks on macOS.** The volumes of an APFS partition live on a synthesized container disk, which the backend maps back to the partition from `diskutil list -plist virtual`. That's tested against sample output and against the CI runner's startup disk, but not yet against an APFS-formatted USB stick.
- **FAT32 output on players.** The layout matches mkfs.fat's and passes `fsck.fat`, but it has to be tried on CDJs from each generation.
- **Engine DJ paths.** Track paths in `m.db` are resolved against the database folder, the `Engine Library` folder and the volume root, because the exact base isn't documented. It should be checked against a real Engine DJ 4.x export.
- **Product images.** The app draws illustrations. Licensed photography (PRD §96) is still open.
- **Legal review** of reading vendor database formats and of the dependency licences (PRD §95). The helper and the CLI link only permissively licensed crates (MIT, Apache-2.0, BSD, ISC, Zlib, CC0), plus SQLite, which is public domain. Through Tauri the desktop app also links `cssparser`, `selectors`, `dtoa-short` and `option-ext`, which are MPL-2.0. That licence is file-level copyleft, so it allows a proprietary app but obliges publishing any changes to those files, and it belongs in the review. rekordcrate (MPL-2.0) is only a dev-dependency, used to cross-check the PDB reader.
