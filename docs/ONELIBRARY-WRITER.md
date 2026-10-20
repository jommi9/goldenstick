# OneLibrary writer

Status: research and proposed design, written 2026-10-19. No writer exists in BoothReady yet, no export was written to a USB for this work, and no player has read a OneLibrary database written by BoothReady or by the throwaway spike described below.

BoothReady's Device Library writer (`crates/boothready-core/src/export/device_library.rs`) produces `export.pdb`, the ANLZ files and the audio. BoothReady's library check passes that tree for the CDJ-2000, CDJ-2000NXS2, CDJ-3000 and XDJ-XZ and blocks it for the CDJ-3000X, CDJ-1500X and XDJ-AZ, because those players read `PIONEER/rekordbox/exportLibrary.db` (OneLibrary, formerly Device Library Plus) instead. This document records what is known about that database and proposes how BoothReady could write one next to `export.pdb` in the same export.

## Conclusion

A OneLibrary writer is feasible, and the shorter route is a port of djbooth's MIT-licensed writer, the same source the Device Library writer was ported from. A clean-room writer is also feasible because every fact the writer needs, from the schema down to the typical value of each track column, can be read from rekordbox's own output, and those facts were confirmed independently against two OneLibrary databases that rekordbox wrote to Jommi's sticks. The decryption key is the one exception, since it can only come from a published source such as pyrekordbox or djbooth (both MIT).

A throwaway Rust spike, kept outside the repository, opened both rekordbox databases with `rusqlite`'s bundled SQLCipher, wrote a new encrypted database with the same schema, and had that database read back by a second, independent SQLCipher implementation. The pieces BoothReady already has, namely track metadata, analysis files, the audio layout and the ANLZ path hash, carry over unchanged: the path hash in `pdb_writer.rs` reproduces rekordbox's OneLibrary analysis path for all 559 tracks in the two databases.

The work splits into three pull requests of moderate size, listed under [Work](#work), of which the first swaps the workspace's SQLite for SQLCipher and adds a OneLibrary reader that BoothReady's library check can use on its own. A player test on each of the three target models remains the only way to establish that they accept the result.

## Sources

Commits are the ones inspected on 2026-10-19. A later commit needs a new review.

| Source | Commit | License | What it offers | Use in BoothReady |
|---|---|---|---|---|
| [gakonst/djbooth](https://github.com/gakonst/djbooth/tree/5222611a9cbcc8b0de3dbec9f6da347dcda98595/packages/rekordbox-ts/src/devicelib-plus) `packages/rekordbox-ts/src/devicelib-plus` | `5222611a9cbcc8b0de3dbec9f6da347dcda98595` | MIT (`packages/rekordbox-ts/LICENSE`, copyright Dylan Jones and Georgios Konstantopoulos) | `export.ts`: `writeOneLibraryPlaylist`, which creates the 22-table schema, inserts the fixed rows, appends a folder, a playlist and its tracks, and reopens the file for verification. `database.ts`: the obfuscated key and a reader. | Port, as with the Device Library writer. These files are identical at `452c46f`, the commit `UPSTREAM.md` pins. |
| [dylanljones/pyrekordbox](https://github.com/dylanljones/pyrekordbox/tree/2dcb57f2ea907c51fa15850f5433ec09ff3223ff/pyrekordbox/onelibrary) `pyrekordbox/onelibrary` | `2dcb57f2ea907c51fa15850f5433ec09ff3223ff` | MIT | The `OneLibrary` reader (renamed from `DeviceLibraryPlus` in commit `6badf7e365` on 2026-10-08), SQLAlchemy models of every table, `add_*` helpers for single rows, and the obfuscated key as `BLOB` in `database.py`. It does not create a database from nothing. | Reference for column meanings and enum values, and an independent reader for verification. |
| [Holzhaus/rekordcrate](https://github.com/Holzhaus/rekordcrate/tree/14d54eded7b0d2f6fe67c7871533dca35a73e5fa) | `14d54eded7b0d2f6fe67c7871533dca35a73e5fa` | MPL-2.0 | Readers for PDB, ANLZ, settings files and XML. No module opens `exportLibrary.db`. | Keeps verifying `export.pdb` and the ANLZ files; it cannot verify OneLibrary. |
| [Deep-Symmetry/crate-digger](https://github.com/Deep-Symmetry/crate-digger/tree/7c9d5358d8a3ce663f566380f2dc79ab1393201e) | `7c9d5358d8a3ce663f566380f2dc79ab1393201e` | EPL-2.0 (MPL-2.0 and LGPL-3.0 as secondary licenses) | `Archivist.java` copies `exportLibrary.db` as an opaque file when it archives an export. | None. |
| [chrisle/rbxport](https://github.com/chrisle/rbxport/tree/7fb4dad3f88a2cc0cecec0e718b4dde408fd7448) `crates/rbl-onelibrary` | `7fb4dad3f88a2cc0cecec0e718b4dde408fd7448` | GPL-2.0-or-later | A Rust crate described as reading and writing `exportLibrary.db`, built on `rusqlite` with `bundled-sqlcipher-vendored-openssl`. | None. BoothReady's workspace license is `UNLICENSED`, so GPL code cannot be copied or linked. Only the manifest, `LICENSING.md` and README were read. |
| [morizkraemer/fourfour](https://github.com/morizkraemer/fourfour/tree/d188f284c315693a146f99e920d7ae36f0dacd38) `pioneer-usb-writer` | `d188f284c315693a146f99e920d7ae36f0dacd38` | Unresolved: no license file and no `license` field at this commit, as at the earlier commit recorded in `EXPORT-PROVIDERS.md` | Its manifest depends on `rusqlite` with `bundled-sqlcipher` and its tree has `src/writer/onelibrary.rs`. | None until the license is resolved. The writer source was not read. |
| `libsqlite3-sys` 0.35.0 (the version in `Cargo.lock`) | crates.io | MIT for the crate; the bundled SQLCipher 4.6.1 community edition is under Zetetic's BSD-style license | Feature `bundled-sqlcipher` compiles SQLCipher in place of SQLite. | The proposed encryption layer. |
| `better-sqlite3-multiple-ciphers` 12.11.1 (SQLite3MultipleCiphers 3.53.2) | npm, as installed in djbooth's checkout | MIT | A separate implementation of the SQLCipher 4 format, used by djbooth. | Independent reader for development checks. |

djbooth's own porting log (`packages/rekordbox-ts/PORTING.md`) lists the OneLibrary writer as "hardware validation pending". Its 2026-08-02 entry describes a physical FAT32 export whose OneLibrary passed `quick_check` and held the expected playlist order, which is a check of the stick's files without a player reading them. The top-level README names the XDJ-AZ among the players its media works with, without a record that ties a OneLibrary written by djbooth to that player. Treat both as upstream claims.

## The format

### File and encryption

The database is `PIONEER/rekordbox/exportLibrary.db`. It is a SQLCipher 4 database: both rekordbox files opened with SQLCipher's version 4 defaults and the key, and with nothing else set. Those defaults are 4096-byte pages, PBKDF2-HMAC-SHA512 key derivation with 256,000 iterations, AES-256 page encryption and an HMAC-SHA512 per page. A SQLCipher file begins with a random 16-byte salt instead of `SQLite format 3`, which is the property `scan_rekordbox` already uses to report `encrypted`. djbooth opens the same files with SQLite3MultipleCiphers set to `cipher='sqlcipher'` and `legacy=4`, which selects the same parameters.

One of the two rekordbox databases was in rollback-journal mode with no side files. The other was in WAL mode with `exportLibrary.db-wal` and `exportLibrary.db-shm` beside it. `one_library_written_unix` already counts the WAL when it dates the library.

### Key

Every OneLibrary database uses the same passphrase. It is 64 characters long and begins `r8gd`; both of Jommi's sticks open with it, and their `property.createdDate` values are 2024-06-18 and 2025-12-25. pyrekordbox and djbooth publish it obfuscated: the stored string is base85 text that decodes to bytes which, after an XOR with a fixed 16-byte ASCII string and zlib decompression, give the key. This document does not reproduce the key. The writer should carry it the way upstream does, as the obfuscated string plus the decoding function, so that a search of the repository for the plain key finds nothing, and it should check the `r8gd` prefix after decoding as djbooth and pyrekordbox do.

If AlphaTheta changes the key in a later rekordbox or firmware, databases written with the old key would stop opening on players that expect the new one. The verification section below includes a check against a fresh rekordbox export for that reason.

### Schema

The schema has 22 tables and 4 indexes. The `CREATE` statements in djbooth's `export.ts` match the `sqlite_master` entries of both rekordbox databases statement for statement, so the writer can create the schema from that text.

| Table | Purpose |
|---|---|
| `content` | One row per track: title, BPM times 100, length in seconds, artist, album, genre, label and key IDs, the volume-absolute audio path and file name, file size and type, bit rate, bit depth, sample rate, the analysis path and bookkeeping fields. |
| `artist`, `album`, `genre`, `label`, `key`, `color`, `image` | Lookup tables referenced from `content` and `album`. |
| `playlist`, `playlist_content` | The playlist tree (`attribute` 0 for a playlist, 1 for a folder, 4 for a smart playlist; `playlist_id_parent` 0 at the root) and ordered membership by `sequenceNo`. |
| `cue`, `hotCueBankList`, `hotCueBankList_cue` | Memory cues, hot cues, loops and hot cue banks. |
| `history`, `history_content` | Play history the players write back. |
| `myTag`, `myTag_content` | My Tag columns and tags. |
| `menuItem`, `category`, `sort` | The player's browse menu and sort order. |
| `property` | One row: device name, database version, track count, creation date, background colour and My Tag master ID. |
| `recommendedLike` | Track pairings. |

### Fixed rows

In both rekordbox databases, `menuItem` (27 rows), `category` (22 rows), `sort` (17 rows) and `color` (8 rows) hold exactly the values djbooth inserts. The menu names are wrapped in U+FFFA and U+FFFB, for example `￺GENRE￻`.

Two tables differ from djbooth:

- `myTag`: rekordbox writes four parent rows (`Genre`, `Components`, `Situation` and `Untitled Column`, IDs 1 to 4, `attribute` 1) and 24 default tags under them with large IDs. djbooth writes a single `Tags` row.
- `property`: rekordbox wrote `dbVersion` as `'10000'` in the 2024 database and as `'1000'` in the 2025 one, both as text, and `myTagMasterDBID` as the same large integer in both. djbooth writes `'10000'` and `1`.

### Track rows

Values observed in the 559 track rows of the two rekordbox databases, all MP3 files:

| Column | rekordbox | djbooth |
|---|---|---|
| `path` | `/Contents/...`, the same path `export.pdb` stores | same |
| `analysisDataFilePath` | `/PIONEER/USBANLZ/Pxxx/xxxxxxxx/ANLZ0000.DAT` | same, from the same hash |
| `dateAdded`, `dateCreated` | `YYYY-MM-DD` text | `YYYY-MM-DD` text |
| `titleForSearch`, `nameForSearch` | `NULL` | `NULL` |
| `artist_id_lyricist`, `color_id` | `0` | `0` |
| `isHotCueAutoLoadOn`, `isKuvoDeliverStatusOn` | `1` | `1` |
| `hasModified` | `0` | `0` |
| `contentLink` | `788224` in every row | `NULL` |
| `masterDbId` | one large integer, the same in both databases | from rekordbox's `master.db`, else `NULL` |
| `masterContentId` | the track's ID in rekordbox's `master.db` | same, else `NULL` |
| `analysedBits` | `41` in 553 rows, `105` in 6 | `105` unless given |
| `analysisDataUpdateCount`, `informationUpdateCount` | `NULL` in 553 rows, `1` in 6 | `2` unless given |
| `cueUpdateCount` | `NULL` in 553 rows, empty text in 6 | empty text |
| `image_id` | set; `image.path` is `/PIONEER/Artwork/NNNNN/xN.jpg` | `NULL` |
| `fileType` | `1` (MP3) | `1` MP3, `4` M4A, `5` FLAC, `11` WAV, `12` AIFF |

The `fileType` values for the other formats are the ones `pdb_writer.rs` already writes to `export.pdb`; djbooth and pyrekordbox use the same numbers for OneLibrary, but no rekordbox OneLibrary with those formats has been inspected.

The analysis path check: a transcription of `usb_analysis_path` from `pdb_writer.rs` produced each row's `analysisDataFilePath` from its `path` in 559 of 559 rows, and every referenced audio and `ANLZ0000.DAT` file existed in the stick backups. The analysis files on those sticks are `.DAT`, `.EXT` and `.2EX`, the three files BoothReady already writes, so OneLibrary needs no further analysis output.

Neither rekordbox database contains a playlist, so the playlist rows rest on djbooth's writer and pyrekordbox's models, not on rekordbox output that was inspected.

## Proposed design

### Build

Change the workspace's `rusqlite` feature from `bundled` to `bundled-sqlcipher`. Cargo unifies features across a build, so this replaces SQLite with SQLCipher everywhere `rusqlite` is used: the Engine DJ reader in `library/engine.rs`, the store in `store.rs`, the CLI and the desktop app. A connection without a key behaves as plain SQLite, so those callers need no change, but the engine moves from SQLite 3.50.2 to the 3.46.1 that SQLCipher 4.6.1 is built on. The existing tests show whether that matters.

How SQLCipher gets its cryptography depends on the platform, per `libsqlite3-sys`'s build script:

- macOS uses CommonCrypto from the Security framework, with no OpenSSL. The spike binary linked only `Security`, `CoreFoundation` and `libSystem`.
- Windows has no default; the build stops unless `OPENSSL_DIR` is set or the feature `bundled-sqlcipher-vendored-openssl` builds OpenSSL from source, which needs Perl.
- Linux links the system's `libcrypto`, or the vendored OpenSSL with the same feature.

The proposal is `bundled-sqlcipher` everywhere, with `bundled-sqlcipher-vendored-openssl` added for Windows and Linux through a `[target.'cfg(not(target_os = "macos"))'.dependencies]` entry for `libsqlite3-sys` in `boothready-core`. The vendored OpenSSL is Apache-2.0 and adds to the build time on those platforms; both need a line in the third-party notices together with SQLCipher's license.

If the OpenSSL build turns out to be a problem on the Windows runners or in the release workflow, a fallback is to write a plain SQLite file with `rusqlite`'s `bundled` SQLite and encrypt its pages in Rust with the RustCrypto `aes`, `cbc`, `hmac`, `sha2` and `pbkdf2` crates, following Zetetic's published SQLCipher 4 page format. That fallback keeps OpenSSL out of the build at the cost of owning the page format code, and it needs SQLite to reserve 80 bytes at the end of every page for the IV and HMAC.

### Modules

- `library/onelibrary.rs`: the obfuscated key and its decoding, `open_one_library(path)` (read-only, keyed) and a reader that returns tracks and playlists in the same shapes `pdb.rs` returns them. `scan_rekordbox` can then fill `OneLibraryReport` with counts and paths, which takes OneLibrary from presence-only (level 0) to read and validated, as the Device Library already is.
- `library/onelibrary_writer.rs`: the port of djbooth's `export.ts`, behind a feature `onelibrary-writer` in the same way `device-library-writer` guards `pdb_writer.rs`. It takes a list of tracks and playlists and builds the whole database in one transaction into a path it is given. It does not append, matching the regeneration rule in `EXPORT-PROVIDERS.md`.

The writer's local changes against djbooth would be recorded in `UPSTREAM.md`:

- One call builds the whole database, where djbooth adds one folder and one playlist per call to an existing file. Playlists go at the root, as they do in `export.pdb`.
- Album, genre and label rows are written from the file tags, where djbooth only writes artist and key, so OneLibrary carries the same browse data as `export.pdb`.
- `myTag` gets the four parent rows rekordbox writes, without the 24 default tags, unless a player test shows the tags are needed.
- Values for `masterDbId`, `masterContentId`, `contentLink` and `myTagMasterDBID` follow whatever the open questions below settle.
- The journal mode is set to `DELETE` before the database is closed, so the tree holds one file and no `-wal` or `-shm` side files.

### Export flow

`ExportRequest` gains a field `one_library: bool`. When it is set, `export_device_library` does the following in addition to what it does today:

1. In the per-track loop, it records a `OneLibraryTrack` next to each `TrackInput`, from the same tags, analysis, file type, USB path and analysis path. The two databases then describe the same files with the same IDs.
2. After `writer.build()` and `verify_pdb`, it writes `PIONEER/rekordbox/exportLibrary.db` into the staging folder with the playlists in the same order.
3. It reopens that file read-only through `library/onelibrary.rs` and compares it with what was written, in the same way `verify_pdb` checks `export.pdb`. A mismatch fails the export, and the staging folder is removed as it is today.
4. `install` already moves the whole staging `PIONEER` folder into place, so it needs no change. `ExportReport` gains `one_library_path: Option<PathBuf>`.

The finished folder is a `dual_library_tree` in the terms of `EXPORT-PROVIDERS.md`. The provider declaration changes from Device Library only to both formats, which lets the planner pass the CDJ-3000X, CDJ-1500X and XDJ-AZ. The copy flow, its erase requirement and its device confirmation do not change.

In the app, the Export to USB panel from PR #28 would offer the option and turn it on by default when a selected target requires OneLibrary. The CLI's `export-pdb` command would take `--one-library`.

### Verification without a player

Each layer below catches a different class of mistake. None of them proves that a player accepts the file.

1. **Round trip in BoothReady.** The export reopens the database through BoothReady's reader and compares every track and playlist row, and the tests compare `export.pdb` and `exportLibrary.db` from one export: the same tracks, titles, artists, keys, BPM, paths, analysis paths and playlist order. `PRAGMA integrity_check` must return `ok`.
2. **Independent reader.** A development script opens a generated database with SQLite3MultipleCiphers (`better-sqlite3-multiple-ciphers`) and with pyrekordbox's `OneLibrary`. Both are separate codebases from the SQLCipher that writes the file. This runs locally, since CI would need Node or Python with native modules; a committed small synthetic `exportLibrary.db` lets a CI test check that BoothReady's reader still opens a file the other readers accepted.
3. **Comparison with rekordbox output.** An ignored test, run when an environment variable names a rekordbox-written `exportLibrary.db`, compares the generated schema and fixed rows with that file and reports any column whose type or typical value differs. It runs against the copies in `~/Backups/USB sticks` and against any fresh rekordbox export, which also catches a changed key. Those files hold Jommi's track names and stay out of the repository under `docs/PRIVACY.md`.
4. **BoothReady's own check.** Running `check-dir` on the finished folder must show both formats present, the database encrypted and, once the reader exists, the OneLibrary contents matching the Device Library.
5. **rekordbox as a reader.** rekordbox 7 lists the library of a connected device. A generated export on a FAT32 disk image, mounted on the Mac, may show up there and be browsable without a player. Whether rekordbox lists a mounted image and whether it writes to it are open questions, so this uses a disposable image and never a stick.

The player test then follows `docs/FIELD-TEST.md` on each of the CDJ-3000X, CDJ-1500X and XDJ-AZ: the library loads, the playlists appear in order, each track loads with its beat grid and waveform, and the history written back by the player is recorded. The firmware version of each player is part of the record.

### Licensing

The port of djbooth's code keeps djbooth's MIT notice in `UPSTREAM.md`, which already records the author's agreement for the Device Library port. The key string comes from djbooth and pyrekordbox, both MIT. SQLCipher's BSD-style license, and OpenSSL's Apache-2.0 license where it is vendored, need entries in the third-party notices shipped with the app. Nothing from rbxport or fourfour is used.

### Work

| Step | Content |
|---|---|
| 1 | Switch to `bundled-sqlcipher` with vendored OpenSSL on Windows and Linux, prove the three CI platforms and the release workflow still build, and add `library/onelibrary.rs` with the reader and the key. Raise `scan_rekordbox` to read OneLibrary contents and update the rules that say BoothReady cannot read inside it. |
| 2 | Port the writer, add `one_library` to `ExportRequest`, the read-back check, the `--one-library` CLI flag, the cross-format tests, the ignored comparison test and a small synthetic fixture. |
| 3 | Declare `dual_library_tree` to the planner, add the option to the app's export panel, update `EXPORT-PROVIDERS.md` and `FIELD-TEST.md`. |
| 4 | Player tests on the three target models, with Jommi naming each stick. |

## Open questions

- **Values without a rekordbox source.** BoothReady has no `master.db`, so it has nothing to put in `masterDbId`, `masterContentId` and `myTagMasterDBID`, and `contentLink` held one constant in every rekordbox row. The proposal is `NULL` for the first two, rekordbox's observed constants for `contentLink`, and `1` for `myTagMasterDBID` as djbooth writes; a player test settles whether any of them matter.
- **`property.dbVersion`.** rekordbox wrote `'10000'` in 2024 and `'1000'` in 2025. Which value current players expect, and whether they check it, is unknown.
- **`analysedBits` and the update counters.** rekordbox wrote `41` with `NULL` counters for most tracks and `105` with counters of `1` for a few. The meaning of the bits is undocumented in the sources read here. The proposal follows the majority, `41` with `NULL` counters.
- **Playlist rows.** No inspected rekordbox OneLibrary held a playlist. A rekordbox export of two synthetic tracks in one playlist, made by Jommi to a disk image or a spare stick, would settle the playlist row shapes and the current `dbVersion` in one step.
- **My Tag rows.** Whether players need the 24 default tags or only the four parents.
- **Artwork.** rekordbox writes artwork under `PIONEER/Artwork` and references it from `image`. BoothReady writes no artwork to either database today; whether the target players handle `image_id` `NULL` without trouble is untested.
- **Other formats.** Both rekordbox databases held MP3 files only, so the `fileType`, `bitDepth` and `samplingRate` values for FLAC, WAV, AIFF and M4A in OneLibrary come from djbooth and pyrekordbox.
- **Windows build.** Whether the vendored OpenSSL builds on the `windows-latest` runner and in the release workflow without extra setup.
- **rekordbox as a reader.** Whether rekordbox lists a mounted FAT32 image as a device, and whether it writes to it when it does.
- **Key changes.** Whether newer rekordbox versions or player firmware use a different key. Both inspected databases use the published one, the newer created 2025-12-25.

## How the findings were produced

All work happened on copies in a scratch folder. Nothing was written to a stick or to the backups.

- The two rekordbox databases are `PIONEER/rekordbox/exportLibrary.db` from `~/Backups/USB sticks/SAMSUNG USB 2026-10-18` (405 tracks) and from `~/Backups/USB sticks/APUTIKKU 2026-10-18` (154 tracks, with its WAL and SHM files).
- A Rust program built against `rusqlite` 0.37 with `bundled-sqlcipher` opened each copy with the decoded key, printed the cipher settings, `integrity_check`, the `sqlite_master` statements, row counts, the fixed tables and the value types of the track columns. The schema was compared with the `SCHEMA` string in djbooth's `export.ts` using `diff`.
- A second Rust program created a new database with the same key, the djbooth schema, the fixed rows copied from a rekordbox database through `ATTACH ... KEY`, one synthetic track and one playlist. Its first 16 bytes were random and its `integrity_check` returned `ok`.
- A Node script using `better-sqlite3-multiple-ciphers` from djbooth's checkout opened that spike database and the rekordbox databases with `cipher='sqlcipher'` and `legacy=4`. `quick_check` returned `ok` for each, and it read back the spike's 22 tables, the track and the playlist.
- The same script recomputed each track's analysis path with a transcription of `usb_analysis_path` and checked that the audio and `ANLZ0000.DAT` it names exist in the backup.

The spike's source files, without the key or any data, are in the session notes folder for this work.
