# BoothReady session rules

## Repository layout

- `crates/boothready-model`: shared data types.
- `crates/boothready-core`: engine logic. Features include `sqlite` and `fixtures`.
- `crates/boothready-platform`: macOS, Windows and Linux device backends and diagnostics.
- `crates/boothready-helper`: elevated helper. This is the only code allowed to erase a drive.
- `crates/boothready-testlab`: shared virtual scenarios used by the CLI and desktop test-lab GUI.
- `crates/boothready-cli`: the `boothready` command line tool.
- `app/`: Tauri 2 desktop app.
- `app/src-tauri`: the app's Rust side. It is a workspace member even though it is outside the default members list.
- `docs/ARCHITECTURE.md`: system design and the remaining verification work.
- `docs/FIELD-TEST.md`: real-device test checklist.
- `docs/RULES-REVIEW.md`: source review procedure for compatibility rules.
- `docs/TEST-LAB.md`: virtual matrix, browser scenarios and read-only real-USB testing.

## Required checks

Before any push, run all of these from the repository root unless a command says otherwise:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test
cargo test -p boothready-app
cd app && npm run typecheck && npm run build
```

For the UI walkthrough, install dependencies with `npm ci` in `app/`, install the Playwright browser with `npx playwright install chromium`, then run `npm run screenshots`.

The app's development helper commands live in `app/scripts/dev.mjs`:

```sh
npm run dev:doctor
npm run dev:mock
npm run dev:check
npm run dev:test
npm run dev:test-ui
npm run dev:screenshots
npm run dev:desktop
```

The app's **Test lab** panel runs the virtual matrix, saves diagnostics and exposes a read-only test for one exact removable USB. In demo mode or the browser mock, the separate **Dev tools** panel can reset fixtures, insert or remove simulated drives, open a device and copy a compact state snapshot.

The CLI test lab is read-only for real devices and writes only to report folders on the computer:

```sh
cargo run -p boothready-cli -- test list
cargo run -p boothready-cli -- test virtual
cargo run -p boothready-cli -- test real <exact-device-id> --out test-reports/real-<date>
cargo run -p boothready-cli -- test replay <diagnostics.json>
```

The virtual matrix may format image-backed fixtures and copy between fixture folders because those paths never refer to a physical device. The real test command refuses system disks and non-USB devices, then only enumerates and reads the named device. Use the existing confirmation flow for any real preparation, copy or eject operation.

The local demo desktop flow is:

```sh
cd app
BOOTHREADY_DEMO=1 npx tauri dev
```

In PowerShell use `$env:BOOTHREADY_DEMO=1; npx tauri dev`.

Read-only CLI commands are always safe:

```sh
cargo run -p boothready-cli -- devices
cargo run -p boothready-cli -- check
cargo run -p boothready-cli -- diagnose
```

## Drive safety

- Never erase, prepare or copy onto a drive unless the user named that exact stick in the current conversation and confirmed it immediately before the operation.
- Before any such operation, show the user the device ID, model and size.
- Never touch an internal or system disk.
- Erasing must go through `boothready-helper`. Never use `diskutil`, `diskpart` or `dd` as a workaround.
- USB-C adapters and hubs are supported connection paths. Treat them as transport around the storage device, do not infer a physical connector from negotiated USB speed, and test the adapter or hub that the user expects to use.
- Save a diagnostics report before any field-test stick is inserted and again whenever the result looks wrong.
- When a drive is misread, turn its report into a failing test before changing implementation code.

## Field testing

Follow `docs/FIELD-TEST.md` step by step on macOS and Windows. Keep the expected result and the observed result for every step. Use `boothready_platform::diagnostics::replay` to replay macOS reports on any OS. On Windows, compare the raw IOCTL buffers with PowerShell's view of the same disks.

## Rules review

From `app/`, run `node scripts/fetch-references.mjs`, then compare every claim in `crates/boothready-core/data/ruleset.json` with the saved pages in `research/`. Edit `ruleset.json` directly and preserve its layout. Vendor claims need vendor evidence for that aspect. Community claims need community evidence. Reddit is community evidence only when several threads agree. Run `cargo test -p boothready-core rules` after every edit, and record the sentence from each page that justifies every change.

## Git workflow

Use one branch and one pull request into `main` per task. Do not merge until the user says to merge. Every pull request description must contain a summary, a test plan listing commands actually run, and a `Not verified yet` section. Never claim hardware support without a hardware test.

## Writing rules

Be direct and explain complex points in full. In commit messages, pull request descriptions and docs:

- Do not use em dashes.
- Avoid antithesis and corrections framed as “not X but Y”.
- Avoid the rule of three and parallel sentence structures.
- Avoid filler intensifiers such as “really” and “truly”, and corporate verbs such as “leverage” and “underscore”.
- Avoid hedging, throat-clearing openers and closing summary sentences.
- Join related clauses with subordination instead of chaining short sentences.

## Parked ideas

Ask before starting any of these:

- Warn when a stick's OneLibrary is older than its Device Library.
- Revisit the BoothReady name because `boothready.app` already exists.
- Sign builds with an Apple Developer ID and a Windows code-signing certificate.
