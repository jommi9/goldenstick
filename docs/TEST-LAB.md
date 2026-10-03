# BoothReady test lab

The test lab covers engine behaviour, helper formatting, app state transitions and read-only checks against real USB drives. It keeps virtual writes inside fixture directories. The real-drive command never prepares, copies or ejects a device.

## Run the complete local suite

From `app/`:

```sh
npm run dev:doctor
npm run dev:test
```

`dev:test` runs the Rust virtual matrix and the browser UI scenarios. `dev:check` runs the required Rust and app checks, then runs the Rust virtual matrix. `npm run dev:screenshots` remains the visual walkthrough.

## Virtual USB matrix

The CLI uses the same engine and helper code as the desktop app against image-backed or folder-backed fixtures:

```sh
cargo run -p boothready-cli -- test list
cargo run -p boothready-cli -- test virtual
cargo run -p boothready-cli -- test virtual acceptance-main --keep
cargo run -p boothready-cli -- --json test virtual
```

The scenarios currently cover the PRD acceptance drive, an empty drive, a complete rekordbox export, helper formatting to MBR + FAT32 and a copy followed by full read-back verification. Reports are saved under `test-reports/virtual-<timestamp>/`. The default run removes fixture contents after saving each result; use `--keep` or `--root` when inspecting them.

Each scenario reports named checks. A failing scenario exits with status 1, so it can run in CI or a pre-push hook. Add a failing scenario check before changing engine code when a fixture exposes a regression.

## Browser UI scenarios

The browser runner uses the mock backend generated from the Rust fixtures:

```sh
cd app
npm run build
npm run test-lab
npm run dev:test-ui
```

The report is `app/test-results/boothready-ui-test-lab.json`. A failed case leaves a `failed-<case>.png` file beside it. The cases cover a clean start, a URL-reproducible exported and verified drive, reset during a scan and a prepared FAT32 fixture.

## Desktop test-lab GUI

Open **Test lab** from the BoothReady top bar. The panel exposes the virtual scenario matrix, a baseline diagnostics action and a read-only real-USB test. The virtual run writes its report under the app's `Downloads/BoothReady/test-lab/` folder. The real run shows the exact device ID, model and size before it reads, then saves diagnostics before and after the read plus a JSON report in the same folder.

Demo mode keeps the real-USB action disabled. Turn Demo mode off, refresh the device list and select the exact removable USB. Internal and system disks are omitted from the selector. The GUI's real-USB action never prepares, copies or ejects a device.

## Read-only testing with a real USB

Start with the field-test baseline before inserting a stick:

```sh
cargo run -p boothready-cli -- diagnose --out test-reports/real-baseline.json
```

After the stick is connected, list devices and copy the exact device ID:

```sh
cargo run -p boothready-cli -- devices
cargo run -p boothready-cli -- test real <exact-device-id> --out test-reports/real-<date>
```

The command prints the device ID, model and size before reading it. It refuses system disks and non-USB devices. It writes `diagnostics-before.json`, `diagnostics-after.json` and `report.json` on the computer. The report contains the drive analysis and compatibility assessment; it does not perform a destructive operation.

Use the connection setup you expect to use at a gig, including a USB-C adapter or hub. The report records the storage device and negotiated link details, while the adapter remains part of the setup being tested.

When a real drive is misread, keep both diagnostics reports and turn the observation into a failing fixture test before changing implementation code. On macOS, replay the report on any operating system:

```sh
cargo run -p boothready-cli -- test replay test-reports/real-<date>/diagnostics-after.json
```

The existing `prepare`, `copy` and `eject` commands remain the only paths for real-device writes and removal. They retain their exact-device checks and confirmation prompts. The test lab does not bypass them.

## What this cannot emulate

The virtual fixtures emulate USB identity, capacity, partition layouts, filesystems, DJ libraries, audio files, helper formatting and common failure cases. They do not reproduce a particular controller's flash translation layer, signal integrity, power behaviour, hub firmware or a player's physical USB port. A model or adapter still needs a field test before BoothReady makes a hardware claim.
