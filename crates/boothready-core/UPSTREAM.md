# Upstream code in boothready-core

## rekordbox Device Library writer

`src/library/pdb_writer.rs` and `src/export/device_library.rs` are a Rust
port of the Device Library writer in
[gakonst/djbooth](https://github.com/gakonst/djbooth) at commit
`452c46f03c99e87df1793558dab1f69fc0a810a1`, path
`packages/rekordbox-ts/src/devicelib`, MIT-licensed. The project owner has
the djbooth author's agreement to use it in BoothReady. The files map as
follows.

| djbooth (TypeScript) | BoothReady (Rust) |
|---|---|
| `pdb.ts`: `LegacyDevicePdbWriter`, `inspectLegacyDevicePdb`, DeviceSQL string codec | `src/library/pdb_writer.rs`: `DevicePdbWriter`, `inspect_pdb`, `encode_device_sql_string`, `decode_device_sql_string` |
| `template.ts`: gzip and base64 of rekordbox's empty `export.pdb` | the `EMPTY_EXPORT_PDB_GZIP_BASE64` constant and `empty_device_pdb` in the same file |
| `anlz-path.ts`: `legacyUsbAnalysisPath` | `usb_analysis_path` in the same file |
| `export.ts`: `exportLegacyDeviceLibrary` | `src/export/device_library.rs`: `export_device_library` |

The row layouts, string encodings, ID allocation, page bookkeeping and the
analysis path hash are unchanged, so a database written here has the same
shape as one written by djbooth, and the port is checked row for row against
BoothReady's own reader and against rekordcrate in the module tests.

The empty template is rekordcrate's
`data/complete_export/empty/PIONEER/rekordbox/export.pdb` (Holzhaus/rekordcrate,
MPL-2.0), used unmodified; its SHA-256 is
`aa22564888963e1ad1771b20cc858a3dc36b235a4463309da244d6894d605a09`. djbooth
redistributes it the same way. MPL-2.0 is file-level copyleft and the file is
data rather than linked code, and it is listed in the legal review in
`docs/ARCHITECTURE.md`.

### Local changes

- A data page is closed at 255 rows. djbooth fills a page up to 511 slots and
  stores the ninth bit of the count in the byte at `0x19`, which rekordbox
  itself does on playlist entry pages: crate-digger documents bytes
  `0x18..0x1B` as a 13-bit row-offset count followed by an 11-bit count of
  present rows. BoothReady's `src/library/pdb.rs` reads the 13-bit count,
  while readers written from crate-digger's older notes, among them
  rekordcrate 0.3.0 and beat-link, take the byte at `0x18` alone and would
  see only part of such a page. Pages written here are readable by both
  conventions at the cost of about ten percent more pages for playlists over
  255 tracks.
- `export_device_library` takes a list of audio files and playlists instead of
  reading rekordbox's `master.db`, and it reads title, artist, album, genre,
  tempo and key from the files' tags and from `djbooth-analyzer` instead of
  from database rows. Tempo and key from the analysis win over the tags.
- The export is always fresh. djbooth's ownership marker
  (`.music-cdj-export.json`), differential reuse of unchanged media files,
  removal of files from an earlier export and history import are not ported.
  A target that already holds `Contents` or `PIONEER`, or the staging folder
  of an interrupted export, is refused, following the regeneration rule in
  `docs/EXPORT-PROVIDERS.md`.
- A track whose analysis fails is still exported, without analysis files and
  with a warning in the report; djbooth requires an `ANLZ0000.DAT` for every
  track because its database already holds one.
- The generated database is read back through BoothReady's reader and
  through the ported inspection before the staged tree is moved into place.
- Errors are `WriterError` and `ExportError` values instead of thrown
  `TypeError` and `RangeError`; field widths are enforced by the Rust types.

### Updating

Diff `packages/rekordbox-ts/src/devicelib` between commit `452c46f` and the
new djbooth commit, apply the changes to the two Rust files, rerun
`cargo test -p boothready-core pdb_writer export::`, and update the commit
here.

## rekordbox OneLibrary writer and reader

`src/library/onelibrary_writer.rs` is a Rust port of djbooth's
`writeOneLibraryPlaylist` in `packages/rekordbox-ts/src/devicelib-plus/export.ts`
at the same commit `452c46f03c99e87df1793558dab1f69fc0a810a1` (MIT, Dylan Jones
and Georgios Konstantopoulos), and `src/library/onelibrary.rs` carries the
obfuscated database key from djbooth's `database.ts` with the base85, XOR and
zlib decoding from djbooth's `utils.ts`. pyrekordbox (MIT) publishes the same
key the same way. The key is never stored in plain text in this repository.

| djbooth (TypeScript) | BoothReady (Rust) |
|---|---|
| `devicelib-plus/export.ts`: `SCHEMA`, `MENU_ITEMS`, `CATEGORIES`, `SORTS`, `COLORS`, `writeOneLibraryPlaylist`, `insertContent` | `src/library/onelibrary_writer.rs`: the same constants, `write_one_library`, `insert_content` |
| `devicelib-plus/database.ts`: `DEVICE_LIBRARY_PLUS_KEY_BLOB` | `KEY_BLOB` in `src/library/onelibrary.rs` |
| `utils.ts`: `deobfuscate`, `decodeBase85`, `xorBlob` | `key` and `decode_base85` in the same file |

The schema and fixed rows are unchanged. An ignored test,
`rekordbox_reference_has_the_same_schema_and_fixed_rows`, compares them with a
database rekordbox wrote; it passed on 19 October 2026 against the two OneLibrary
databases in Jommi's stick backups.

### Local changes

- One call builds the whole database from the `TrackInput` rows the Device
  Library writer receives, so both databases describe the same files with the
  same IDs. djbooth appends one folder and one playlist per call to an existing
  file. Playlists go at the root, as in `export.pdb`, and an existing file is
  refused.
- Album, genre and label rows are written from the tags as well as artist and
  key.
- Values that differ from djbooth follow what rekordbox wrote in the December
  2025 database on Jommi's stick: `analysedBits` 41 (djbooth 105), `contentLink`
  788224 (djbooth `NULL`), `NULL` update counters (djbooth 2 and empty text),
  `property.dbVersion` "1000" (djbooth "10000"), and the four My Tag parent rows
  `Genre`, `Components`, `Situation` and `Untitled Column` (djbooth one `Tags`
  row). `masterDbId` and `masterContentId` are `NULL` because there is no
  rekordbox master database, and `myTagMasterDBID` is 1 as in djbooth.
- The journal mode is `DELETE`, so no `-wal` or `-shm` file is left beside the
  database.
- BoothReady reads the database back after writing it and compares every track
  and playlist with the export, where djbooth checks the one playlist it added.
