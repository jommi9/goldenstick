# BoothReady interface redesign

Status: direction and clickable mockup (`docs/design/mockup/index.html`), 19 October 2026. The app's code still has the old interface; this document is the target for rewriting it.

## What is wrong today

The app is laid out like a web page. Every screen is a centred column with a large heading, a paragraph of explanation and stacked cards, so a library of 1,000 tracks shows eleven rows at a time and the compatibility report needs two screens of scrolling. The USB stick and the music live on separate tabs, which hides the one relationship the app exists to manage: which of my playlists are on which stick, and will that stick play. Track rows are two lines tall, the folder tree sits inside a card, and a large share of the window goes to page chrome, breadcrumbs and a footer.

## References and what each contributes

These were looked at from the vendors' own product images on 19 October 2026.

- **rekordbox 7, EXPORT mode** (rekordbox.com overview hero image). The model to follow for the whole window: a player strip across the top with the overview waveform, phrase colours, cue markers, BPM and key; below it a three-part browser with a narrow icon rail, a tree (Collection, Playlists, Devices) and a dense single-line track list with a mini waveform column, artwork thumbnails, key cells highlighted when they mix with the loaded track, and star ratings. Gunmetal greys with one blue accent, so colour is spent on waveforms, keys and states. USB sticks appear as devices in the same tree and playlists are dragged onto them.
- **Lexicon** (lexicondj.com product screenshot). The best pure library manager: a searchable playlist tree with smart lists grouped by mood and timing, coloured cue chips under the waveform, keys as coloured Camelot pills, energy as a small bar meter, tags as outlined chips, and multi-track editing in a panel with an artwork strip.
- **Engine DJ Desktop** (known from use; no usable product image on its site). Drives listed with the library in one sidebar and a sync manager that shows what will change on the drive before it writes.

BoothReady keeps its own jobs, which none of these do: judge each stick against the players it will meet, show why a track will not play, and write sticks without rekordbox.

## The new window

One window, four regions, no pages.

1. **Player strip** (top, 132 px, collapsible). Artwork, title and artist; the full-width colour overview waveform from the track's own analysis (PWV5), with the phrase band under it (PSSI) and hot cue markers; elapsed and remaining time, BPM, Camelot key. Space plays and pauses. When nothing is loaded the strip shows the selected stick's capacity and readiness instead of a hero illustration.
2. **Tree** (left, 240 px). Sections in this order: Collection (all tracks, recently added, needs analysis, won't play), Playlists (the Drive folders under `DJ/`, nested as on disk, with track counts), Smart lists (by BPM range, by key, by format problem), Devices (each connected stick with label, model, a capacity bar, library badges for rekordbox, OneLibrary, Engine and Serato, and a readiness dot; expanding it lists the playlists already on the stick), and Gig kit. A plugged-in stick appears here with a toast; it never takes over the window.
3. **Track list** (centre). 24 px single-line rows. Default columns: state dot, artwork, preview waveform, title, artist, BPM, key pill, time, format, plays on, added. Columns sort, resize and hide. Keys that mix with the loaded track get a green cell, as in rekordbox. A filter bar above the list holds search, state chips with counts (Ready, In the cloud, Won't play everywhere, Unreadable) and the player set the tracks are judged against.
4. **Inspector** (right, 300 px, toggles). Context for the selection: one track (tags, analysis, a compatibility line per player family with the reason), several tracks (shared fields, as Lexicon's batch editor), a playlist (size, analysis status, which sticks hold it), or a stick (the readiness matrix and the export queue).

A status bar at the bottom carries scan and analysis progress, the number of connected sticks and "Runs locally".

The Playlists section is edited like a file manager, since the playlists are folders on disk: a context menu with New playlist, Rename, Remove and Export to USB; a name typed in place (double-click or Return); playlists dragged into folders or onto the source; folders sorted by name without regard to case, with a folder glyph for folders that hold subfolders and a list glyph for playlists; empty playlists shown with a count of 0. Every change goes through the sorting undo log, so Cmd+Z walks it back.

## Writing a stick

Drag playlists onto a stick in the tree, or tick them and press Export (Cmd+E). The inspector switches to the stick's **Sync** view: playlists to add, tracks to copy, tracks still to analyse with the time it will take, space needed against space free, and the libraries that will be written (Device Library now, OneLibrary when the writer exists). One primary button, "Write to MAIN_USB", opens the existing confirmation sheet with the device ID, model, size and serial tail. Progress replaces the button in place, then the verification result. The rule that an export is built whole in a local folder and never spliced into an existing library stays; the Sync view says when a stick's existing library will be replaced.

## Readiness without prose

The compatibility report becomes a compact matrix in the stick inspector: players as rows, the five checks as columns, one glyph per cell, and a one-line verdict per player. Selecting a cell shows the reason and its evidence in two lines. The "What to do" list becomes small fix rows with a button each. Long explanations move into the detail line or a "Why?" disclosure. The three-stick gig kit stays, as three compact cards under Gig kit in the tree.

## Visual language

- Surfaces: `#0c0d0f` window, `#141518` panels, `#1b1d21` raised, `#262930` lines. Text `#e8eaed`, secondary `#9aa1aa`, tertiary `#5d636c`.
- One accent, rekordbox blue `#2f8cff`, for selection, focus and the primary action. State colours stay reserved for states: ready `#3ecf72`, warning `#f2b33d`, problem `#ff5d5d`, cloud `#7aa7ff`. This keeps the flat greyscale direction of PR jommi9/goldenstick#17.
- Keys use the Camelot wheel colours, so neighbouring keys read as neighbouring hues.
- Waveforms use the analysis colours (blue lows, amber mids, white highs).
- Type: M PLUS 1p stays for the player strip's numbers and labels, where it echoes the CDJ screen (PR jommi9/goldenstick#16). The tree, list and inspector use the system UI face at 12 px with tabular numerals, because density matters more there. Section labels are 10.5 px uppercase with letter spacing.
- No cards inside cards, no centred columns, no page headings. Borders are 1 px lines between regions.

## Size and screens

The window is designed at 1900 px wide with 13.5 px text, 32 px rows and a 176 px player strip, and it scales with the screen instead of leaving empty space: on a 2560 x 1440 display everything draws about 35% larger, and on a laptop panel slightly smaller, never below 90%. The app opens maximised and has a full-screen toggle (F). When the list gets narrow it drops columns in a fixed order (plays on, then the preview waveform, then state), and below 1700 px of layout width the inspector starts hidden and opens by itself when a stick is selected or a playlist is queued.

## Keyboard

Arrow keys move the selection, Return loads into the player, Space plays, Cmd+F searches, Cmd+E exports the selection to the selected stick, Cmd+I toggles the inspector, Cmd+1/2/3 jump to Collection, Playlists and Devices.

## What carries over unchanged

The engine, the Tauri commands, the drive safety rules and the confirmation flows. The first run, identify and erase sheets keep their content and become sheets over the main window. Test lab and Dev tools move to a menu.

## Rewrite order

1. Shell: the four regions, tree, list and status bar, driven by the existing library commands. The USB tab's screens become sheets.
2. Devices in the tree with the stick inspector and readiness matrix.
3. Player strip with real waveforms and phrases from the analysis cache.
4. Sync view and drag-to-export, on top of the export-to-USB work (branch `claude/export-to-usb`).
5. Batch inspector and smart lists.

Each step keeps `npm run check-ui` passing with updated checks, and adds screenshots to `docs/screenshots` and a line to the devlog.
