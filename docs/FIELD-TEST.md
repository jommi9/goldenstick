# Field test

This is the first run of BoothReady against real USB sticks, on a Mac and on a Windows PC. CI has only run the macOS and Windows backends against the runners' own internal disks, so every step below is the first time that code touches a stick.

## What you need

- The alpha installers. On GitHub, open Actions, pick **Installers**, choose **Run workflow**, and download the macOS `.dmg` and the Windows `.msi` from the finished run.
- Three sticks you can wipe. The most useful set is one small stick (8 to 32 GB), one large stick (128 GB or more) formatted exFAT, and one stick formatted on a Mac with Disk Utility's defaults, which gives it a GUID partition map.
- On a Mac, ideally also a stick formatted as APFS in Disk Utility.
- rekordbox 7.2.11 or later, for the export step.

Nothing here touches your internal disk or any stick you don't pick, but only use sticks whose contents you don't need.

## Whenever something looks wrong

Click **Save diagnostics** in the app's footer. It writes `boothready-diagnostics-<os>-<time>.json` to your Downloads folder. Send that file together with a line about what you expected and what you saw.

The file lists disk and volume names, sizes, partition layouts and USB IDs, as BoothReady read them and as the operating system reported them. It holds no file names and nothing from your music. For a Mac, a report replays on any machine, so the bug turns into a test before it gets fixed.

From a terminal, `boothready diagnose` writes the same report.

## Steps

Do these on the Mac first, then repeat them on Windows. Note the result of each step, even when it works.

1. **Install and launch.**
   - On the Mac, open the `.dmg`, drag BoothReady to Applications, then right-click it and choose Open, because the alpha is unsigned.
   - On Windows, run the `.msi`, and choose **More info**, then **Run anyway** if SmartScreen stops it.
   - Expected: the home screen says "Plug in a USB", and the footer says it runs locally.
2. **Nothing plugged in.** Click Save diagnostics once before inserting any stick. This report is the baseline for your machine.
3. **Insert the small stick.**
   - Expected: it appears within about two seconds with the right brand and size.
   - Your internal disk is never offered.
   - Save diagnostics if the brand, size, filesystem or partition scheme is wrong.
4. **Assess it** for "Unknown club or festival equipment".
   - Expected: the matrix shows each player with a partition, filesystem, library and audio verdict.
   - Clicking a cell shows the evidence and the document it comes from.
5. **Insert the Mac-formatted GPT stick.**
   - Expected: every Pioneer player shows "GPT is not supported", and the fix offered is a rebuild as MBR + FAT32.
6. **Prepare the small stick.**
   - Confirm the erase. You'll be asked for your password (Mac) or an administrator prompt (Windows).
   - Expected: after a minute or less, the stick mounts again as FAT32, labelled for its role (`BR_MAIN`, `BR_LEGACY` or `BR_BACKUP`).
   - Check it in Finder or Explorer: it should be empty apart from a hidden `.boothready` folder.
7. **Export from rekordbox** to the prepared stick with both Device Library and OneLibrary.
   - Expected: BoothReady notices the export by itself and moves on to verification.
8. **Verify.**
   - Expected: quick verification passes. A full verification reads every file and passes.
9. **Copy to a second stick.** Prepare the large stick as the Backup and accept "Copy from" the first one.
   - Expected: the copy finishes and a full verification passes.
   - Pausing halfway and starting the copy again should pick up where it stopped, without copying finished files twice.
10. **Eject.**
    - Expected: the app says the stick is safe to remove, and the OS agrees.
    - Try once with a Finder or Explorer window open on the stick. The app should name what's holding it.
11. **APFS stick (Mac only).** Insert it.
    - Expected: it shows the APFS volume's name and "APFS", not a question mark.
12. **In the booth.** Put the prepared FAT32 stick into any CDJ or XDJ you can get to, ideally an NXS2 and a 3000.
    - Note the model and firmware version (shown in the player's utility menu), whether playlists appear, and whether tracks load.

## What to send back

- The diagnostics files from steps 2, 3 and any step that went wrong.
- For each step, whether it did what the "Expected" line says.
- For step 12, the player model, its firmware version and what happened.
