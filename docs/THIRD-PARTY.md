# Third-party code bundled in BoothReady builds

This lists code compiled into the CLI and the desktop app that carries its own
license terms beyond the crates' license fields, which `cargo deny` checks.

| Component | Where | License | Notes |
|---|---|---|---|
| SQLCipher 4.6.1 community edition | compiled in by `libsqlite3-sys` with the `bundled-sqlcipher` feature | BSD-style (Zetetic) | Replaces SQLite so rekordbox OneLibrary databases can be read and written. The notice must ship with the app. |
| OpenSSL 3 | Windows and Linux builds only, built from source by `openssl-src` through `bundled-sqlcipher-vendored-openssl` | Apache-2.0 | SQLCipher's cryptography on those platforms. macOS builds use CommonCrypto and contain no OpenSSL. |
| djbooth rekordbox writers | `crates/boothready-core/src/library/pdb_writer.rs`, `onelibrary_writer.rs`, `onelibrary.rs` | MIT | See `crates/boothready-core/UPSTREAM.md`. |
| djbooth analyser | `crates/djbooth-analyzer` | MIT, with Symphonia under MPL-2.0 | See `crates/djbooth-analyzer/UPSTREAM.md` and `THIRD_PARTY_LICENSES.md`. |
| M PLUS 1p | `app/public/licenses/M-PLUS-1p-OFL.txt` | SIL OFL 1.1 | The interface typeface. |

The notices for SQLCipher and OpenSSL are not yet copied into the app bundle;
the release workflow needs to add them before a public build.
