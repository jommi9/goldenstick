# Redesign devlog

## 19 October 2026: references and first mockup

The current interface was reviewed from its own screenshots (`app/screenshots`, the export-to-USB branch). It reads as a web page: centred columns, large headings, two-line track rows, and the stick on a separate tab from the music.

References looked at: rekordbox 7's EXPORT mode in the product image on rekordbox.com, and Lexicon's library screenshot on lexicondj.com. Taken from rekordbox: the player strip with overview waveform, phrase band and readouts; the narrow icon rail; the tree with devices beside playlists; dense rows with a preview waveform column; green key cells for tracks that mix. Taken from Lexicon: Camelot key pills in wheel colours and a calmer list with chips. The direction is written up in `LIBRARY-REDESIGN.md`.

`mockup/index.html` is a working, self-contained mockup of the new window. It draws the analysis BoothReady itself writes: the three-band overview from the `PWV7` section of each track's `.2EX` file, the row previews from the same data, and the phrase band from `PSSI`. The first pass used the colour waveform (`PWV5`) and came out pink; the three-band data in low, mid, high order gives rekordbox's blue lows, amber mids and white highs.

The mockup reads real library data from `local-data/mockup-data.js`, which is git-ignored, so the screenshots in this folder use the built-in sample (`?sample`) and show invented tracks.

![Playlist with a loaded track](screenshots/01-playlist-loaded.png)

The stick inspector replaces the old report page: capacity, the sync queue built by dragging playlists onto the stick or pressing Export, and the readiness matrix with a reason line for the selected player.

![Sync queue on a stick](screenshots/03-stick-sync.png)

![Readiness matrix with a failing player selected](screenshots/04-stick-readiness.png)

Fixes made while looking at the captures: the state column clipped at 1440 px, the track number column was too narrow, the inspector's full-width artwork pushed the details below the fold, the queue row's dot stretched into a bar, and the matrix verdicts clipped.

Open: the mockup has no light theme yet, drag and drop is wired but untested with a real pointer, and the Kingston stick's playlists show without artwork or keys because only the exported playlist has analysis.

## 19 October 2026: bigger, and filling the screen

Feedback on the first mockup: too tight and too small, and it should use the whole screen. The first pass copied rekordbox's laptop density (12 px text, 24 px rows) and was captured at 1440 x 900, which leaves a small window on a 2560 x 1440 display.

The base size went up throughout: 13.5 px text, 32 px rows, a 176 px player strip with 136 px artwork, a 104 px overview waveform and 44 px BPM digits, and wider tree and inspector panels. The window now scales with the screen from a 1900 px design width, so a 2560 px display draws everything about 35% larger, and there is a full-screen toggle on F. On a laptop-sized window the list first overflowed under the inspector; it now drops the plays-on and preview columns when narrow, and the inspector starts hidden there and opens when a stick is selected or a playlist is queued. The screenshots in this folder are now 2560 x 1440.

![Playlist at 2560 x 1440](screenshots/01-playlist-loaded.png)

## 19 October 2026: step 1, the shell in the app

The app now opens on the new window: title bar, player strip (transport, clock, BPM and key; no waveform yet), icon rail, tree with Collection, Playlists (music sources and their folders), Devices (capacity bar, library badges, readiness dot, simulated sticks in demo mode) and Gig kit, the dense track list with sortable columns, Camelot key pills, green cells for keys that mix with the loaded track, a plays-on bar per selected player, an inspector for a track, a source or a stick, and a status bar. The check, erase, copy and verify screens open as sheets over the window, as do Manage sources, Collect and Export to USB. Test lab and Dev tools sit behind the rail. A plugged-in stick appears in the tree with a toast and is read in the background. Rows are 32 px, following the second mockup pass rather than the 24 px of the first. The preview column draws a baseline until step 3 reads the analysis. `npm run check-ui` passes 723 checks; captures are in `app/screenshots` (1920 x 1080, plus 1280 x 800 and 2560 x 1440).

## 21 October 2026: sorting tracks

Folders in a music source are playlists, so sorting moves and copies files between them. Moving is a rename, which keeps cloud files online, and adding to a playlist copies the file. Removing moves the file into `_Removed by BoothReady/<date>/` inside the source, which the scan skips; nothing is deleted, and the last 50 changes can be undone with Cmd+Z or the Undo button in the list header.

Rows can be picked with Cmd-click, Shift-click and Cmd+A. The track inspector lists the playlists that hold a copy of the track, each with a button that takes it out, and has Add to playlist and Move to menus, both able to make a new playlist inside DJ. Dragging rows onto a folder in the tree moves them, and holding Option adds a copy.

When a folder named like an inbox is open, a bar above the list gives nine playlists a number key. Pressing a number moves the selected track there and selects the next one. A playlist keeps its number once it has one, and one picked from the More menu takes the slot used least recently, so the keys stay put while working through the inbox.

![The inbox bar](screenshots/sorting-triage.png)

![Sorting in the track inspector](screenshots/sorting-inspector.png)

A test of the undo log found that undoing a removal put the file back on disk but left the track out of the saved scan; it now comes back in both. `npm run check-ui` passes 772 checks, including four sorting screens.
