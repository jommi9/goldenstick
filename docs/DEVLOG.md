# Devlog

## 2026-10-19: Export to USB in the desktop app (branch claude/export-to-usb, in progress)

- The Library screen's source view gets an **Export to USB** panel: tick folders of a source (each becomes a playlist named after the folder), see the track count, the cloud-only count, files left out, the analysis estimate (six seconds per track not yet analysed, with an analysis cache under the app's data folder so a second export of the same tracks has nothing to analyse), the export folder and the free space, then build the Device Library into a fresh folder under `exports/` in the app's data folder with progress and a stop button.
- The USB tab's export step offers finished exports to the copy flow next to verified drives (`export:<name>` sources). The copy uses the same confirmation and erase rules as before and hands over to verification.
- Screenshots: `docs/screenshots/30-export-plan.png`, `32-export-done.png`, `33-export-copy-offer.png` (browser mock). `npm run check-ui` passes 599 checks including the new export screens.
- Not yet done when this note was written: exercising demo mode in the Tauri window, the real export from the Drive folder, clippy on aarch64 and x86_64, cargo deny, and the pull request.
