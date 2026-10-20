# Devlog

## 2026-10-19: Export to USB in the desktop app (branch claude/export-to-usb)

### What was built

- The Library screen's source view gets an **Export to USB** panel. Ticked folders become playlists named after them. Before anything is written, the panel shows the track count, how many tracks the cloud client will download, files left out, the analysis estimate (six seconds per track still to analyse), the export folder and the free space. The build goes into a fresh folder under `exports/` in the app's data folder, with progress and a stop button.
- Analyses are cached under `analysis-cache/` next to the exports, so a second export of the same tracks, or a rerun after a stop, does not analyse them again.
- The USB tab's export step offers finished exports to the copy flow next to verified drives. The copy keeps the existing plan, erase requirement and confirmation, and hands over to full verification.
- `boothready export-pdb` and the app share the playlist collector and the removable-drive guard, now in `boothready_core::export::playlists`.

### Demo mode in the desktop window

The DJ Library demo source was exported with three ticked folders: Ben Klock, Kolsch and Downloads. The plan listed the empty Downloads playlist and counted its damaged file as left out. The build finished, and the copy offer appeared on the simulated Kingston once it had been rebuilt as MBR and FAT32. Copying five files was followed by a full verification that read the Device Library back ([34-demo-export-verified.png](screenshots/before-redesign/34-demo-export-verified.png)).

The demo tracks are synthetic files under a second long, so the analyser declined every one ("beat analysis requires at least four seconds of audio"). The finished panel only listed this in its notes. It now says plainly when some or all tracks could not be analysed.

### Real run against the Drive library

The real app ran against the Google Drive `Music` source. It exported `DJ/TechnoRemix`: 8 tracks, all still online, 225 MB, into `exports/Music 2026-10-19`. No USB was written. Two sticks were plugged in and left alone.

- A first attempt with the debug build analysed about one track a minute while CPU-bound. Stop finished the current track, removed the staging and export folders and kept the two analyses. The rerun with a release build used those two from the cache. It analysed the other six and downloaded them from Drive in about a minute. The six-second estimate holds for release builds only.
- `boothready check-dir` on the result read a Device Library with 8 tracks and 24 analysis files. Library and audio passed for the CDJ-2000, CDJ-2000NXS2, CDJ-3000 and XDJ-XZ. The CDJ-3000X, CDJ-1500X and XDJ-AZ fail the library check because they need OneLibrary, which this writer does not produce. The panel now says the export holds a Device Library only.

### Fixes the runs turned up

- Ticking a folder deep in Jommi's folder list re-rendered the panel and scrolled the list back to the top. Scrolling boxes marked `data-keep-scroll` now keep their place across renders. Opening the panel from a folder in the tree scrolls that ticked folder into view.
- Stop gave no sign it had been pressed while the current track finished. The panel now says "Stopping after this track."
- A second export on the same day suggested a name that was already taken, so Build stayed disabled until the name was changed. Suggested names now take the next free number.
- The plan's cache note said "already analysed for an earlier export", which was wrong after a stopped run. It now says "already analysed on this computer".

### Screenshots

These come from the browser mock: [30-export-plan.png](screenshots/before-redesign/30-export-plan.png), [31-export-building.png](screenshots/before-redesign/31-export-building.png), [32-export-done.png](screenshots/before-redesign/32-export-done.png), [33-export-copy-offer.png](screenshots/before-redesign/33-export-copy-offer.png). [34-demo-export-verified.png](screenshots/before-redesign/34-demo-export-verified.png) is from the desktop app in demo mode. Captures from the real run show private track names and stay out of the repository.
