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
  itself does on playlist entry pages. Readers written from crate-digger's
  notes, among them BoothReady's `src/library/pdb.rs`, rekordcrate and
  beat-link, take the count from the byte at `0x18` alone and would see only
  part of such a page. Pages written here are readable by both conventions
  at the cost of about ten percent more pages for playlists over 255 tracks.
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
