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
