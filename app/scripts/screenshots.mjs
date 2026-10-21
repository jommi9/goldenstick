// Walks the prototype flow in a headless browser against the mock backend
// and saves a screenshot per screen. Usage: npm run build && npm run screenshots
import { chromium } from "playwright";
import { preview } from "vite";
import { mkdirSync } from "node:fs";

const out = process.env.SHOTS_DIR ?? "screenshots";
mkdirSync(out, { recursive: true });
const server = await preview({ preview: { port: 4174, strictPort: true, host: "127.0.0.1" }, logLevel: "error" });
const url = "http://127.0.0.1:4174/";
const browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH || undefined });
const errors = [];

// The window is designed at 1900 px wide; 1920 x 1080 is the default capture.
const W = Number(process.env.SHOTS_W ?? 1920);
const H = Number(process.env.SHOTS_H ?? 1080);

async function run(scheme) {
  const page = await browser.newPage({ viewport: { width: W, height: H }, colorScheme: scheme });
  page.on("pageerror", (e) => errors.push(`${scheme}: ${e.message}`));
  page.on("console", (m) => m.type() === "error" && errors.push(`${scheme}: ${m.text()}`));
  const shot = async (name, target = page) => {
    await target.waitForTimeout(300);
    await target.screenshot({ path: `${out}/${scheme}-${name}.png` });
    console.log(`${scheme}-${name}.png`);
  };
  const sheet = page.locator(".sheet");
  const click = (name) => sheet.getByRole("button", { name, exact: false }).first().click();
  const plugIn = async (name) => {
    await page.locator(`[data-act="demo-insert"][data-id="${name}"]`).click();
    const dev = page.locator(`.dev[data-dev="demo:${name}"]`);
    await dev.locator(".badge.y").first().waitFor({ timeout: 15000 });
    return dev;
  };

  await page.goto(url);
  await page.locator("#list-title").waitFor();
  await shot("01-window");

  // USB A: the 128 GB SanDisk, GPT + exFAT with only a Device Library. It
  // appears in the tree and is read in the background.
  await page.locator('[data-act="demo-insert"][data-id="sandisk-128"]').click();
  await page.locator('.dev[data-dev="demo:sandisk-128"]').waitFor();
  await page.waitForTimeout(600);
  await shot("02-stick-plugged-in");
  const sandisk = page.locator('.dev[data-dev="demo:sandisk-128"]');
  await sandisk.locator(".badge.y").first().waitFor({ timeout: 15000 });
  // Clicking a stick in the tree only unfolds it and fills the inspector;
  // the inspector's Check button opens the identify sheet.
  await sandisk.click();
  await page.locator('#insp [data-act="device-check"]').click();
  await sheet.getByText("Scanned").waitFor({ timeout: 10000 });
  await shot("03-identify");
  await click("Yes, that's it");
  await sheet.getByText("Prepare this USB").waitFor();
  await shot("04-assessment");
  await click("Prepare this USB");
  await page.getByText("What are you preparing for?").waitFor();
  await shot("05-targets");
  await click("I know the equipment");
  await page.locator("#hwsearch").fill("old nexus");
  await page.waitForTimeout(400);
  await shot("06-hardware-search");
  await click("Back");
  await page.locator('[data-act="preset"][data-id="unknown_club"]').click();
  await click("Continue");
  await sheet.getByRole("table").waitFor();
  await page.locator('[data-act="cell"][data-id="cdj-3000x|library"]').click();
  await shot("07-report");
  await page.getByRole("button", { name: "Rebuild as MBR + FAT32" }).click();
  await page.getByText("You are about to erase:").waitFor();
  await shot("08-confirm-erase");
  await page.getByRole("button", { name: /^Erase and prepare/ }).click();
  await page.getByText("Preparing your USB").waitFor();
  await page.waitForTimeout(1500);
  await shot("09-preparing");
  await page.getByText("Now export from rekordbox").waitFor({ timeout: 15000 });
  await shot("10-export-handoff");
  await click("Simulate the rekordbox export");
  await page.getByText("Verify before the gig").waitFor({ timeout: 15000 });
  await click("Start verification");
  await page.waitForTimeout(1200);
  await shot("11-verifying");
  await page.getByText(/^Verified \d+ files?/).waitFor({ timeout: 10000 });
  await shot("12-verified");
  await click("Continue");
  await page.getByText("Almost there").waitFor();
  await shot("13-kit-progress");

  // USB 2: the 32 GB Kingston becomes the Legacy Rescue drive.
  await page.getByRole("button", { name: /Prepare the Legacy Rescue USB/ }).click();
  await page.getByText("Next: Legacy Rescue").waitFor();
  await shot("14-next-usb");
  await sheet.locator('[data-act="demo-insert"][data-id="kingston-32"]').click();
  await sheet.getByText("Scanned").waitFor({ timeout: 10000 });
  await click("Yes, that's it");
  await click("See detailed report");
  await sheet.getByRole("table").waitFor();
  await page.getByRole("button", { name: "Rebuild as MBR + FAT32" }).click();
  await page.getByRole("button", { name: /^Erase and prepare/ }).click();
  await page.getByText("Now export from rekordbox").waitFor({ timeout: 15000 });
  await shot("15-export-legacy");
  await click("Simulate the rekordbox export");
  await page.getByText("Verify before the gig").waitFor({ timeout: 15000 });
  await click("Start verification");
  await page.getByText(/^Verified \d+ files?/).waitFor({ timeout: 10000 });
  await click("Continue");
  await page.getByText("Legacy Rescue").first().waitFor();
  await shot("16-ready");

  // USB 3: the 64 GB Samsung becomes the Independent Backup. It already
  // holds another library, so the copy offer asks for an erase first; then
  // the verified Main is copied onto it and every file is checked.
  await page.getByRole("button", { name: /Backup USB/ }).click();
  await sheet.locator('[data-act="demo-insert"][data-id="samsung-64"]').click();
  await sheet.getByText("Scanned").waitFor({ timeout: 10000 });
  await click("Yes, that's it");
  await click("See detailed report");
  await sheet.getByRole("table").waitFor();
  await click("It's fine, go to the export step");
  await page.getByRole("button", { name: "Erase this USB first" }).first().waitFor({ timeout: 15000 });
  await shot("17-backup-copy-blocked");
  await page.getByRole("button", { name: "Erase this USB first" }).first().click();
  await page.getByRole("button", { name: /^Erase and prepare/ }).click();
  const copy = page.getByRole("button", { name: /^Copy \d+ files/ }).first();
  await copy.waitFor({ timeout: 15000 });
  await copy.click();
  await page.getByText(/copied/).waitFor();
  await shot("18-copying");
  await page.getByText("Verify before the gig").waitFor({ timeout: 15000 });
  await click("Start verification");
  await page.getByText(/^Verified \d+ files?/).waitFor({ timeout: 10000 });
  await click("Continue");
  await page.getByText("You're booth-ready").waitFor();
  await shot("19-kit-complete");
  await page.keyboard.press("Escape");

  // The stick inspector for a checked stick, with its playlists in the tree.
  await page.locator('.dev[data-dev="demo:sandisk-128"]').click();
  await page.waitForTimeout(300);
  await shot("20-stick-inspector");

  // The music library: a folder on the computer, a cloud folder with tracks
  // still online and an archive drive that isn't plugged in.
  await page.locator('[data-act="open-library"]').first().click();
  await page.getByRole("heading", { name: "Your music" }).waitFor();
  await shot("21-music-sources");
  await sheet.locator('[data-act="library-open"][data-id="demo-computer"]').click();
  await page.locator("#list").getByRole("table").waitFor();
  await page.locator('.tracks tbody tr[data-row="5"] td.c-title').click();
  await shot("22-source-blocked-track");
  await page.locator('.tree [data-act="library-folder"][data-id="demo-cloud|"]').click();
  await page.locator("#list").getByRole("table").waitFor();
  await page.locator('.tracks tbody tr[data-row="2"] td.c-title').click();
  await shot("23-cloud-source");
  await page.locator('[data-act="library-folder"][data-id="demo-cloud|Label Promos"]').click();
  await page.waitForTimeout(250);
  await shot("24-folder");
  await page.locator('[data-act="library-open"][data-id="*"]').click();
  await page.locator("#list").getByRole("table").waitFor();
  await page.locator(".tracks tbody tr").nth(2).locator("td.c-title").dblclick();
  await page.waitForFunction(() => document.querySelector(".player-key.play")?.getAttribute("aria-label") === "Pause");
  await page.waitForTimeout(1200);
  await shot("25-collection-playing");
  await page.locator('.tree [data-act="library-collection"][data-id="blocked"]').click();
  await page.waitForTimeout(300);
  await shot("26-wont-play-everywhere");
  await page.locator('[data-act="library-stop-play"]').click();
  await page.locator('.tree [data-act="library-folder"][data-id="demo-cloud|"]').click();
  await page.locator("#list").getByRole("table").waitFor();

  // Collect the cloud folder into a local copy, then open the result.
  await page.locator('[data-act="library-collect-open"]').click();
  await page.getByRole("heading", { name: /onto this computer$/ }).waitFor();
  await shot("27-collect-plan");
  await page.getByRole("button", { name: /^Collect \d+ tracks?$/ }).click();
  await page.getByText(/^(Reading|Copying|Checking)/).first().waitFor();
  await shot("28-collecting");
  await page.getByRole("button", { name: "Open the collection" }).click({ timeout: 15000 });
  await page.locator("#list").getByRole("table").waitFor();
  await shot("29-collection");

  // Export to USB: tick folders of a source, build the Device Library into
  // a folder on this computer, then copy it onto a prepared USB.
  await page.locator('.tree [data-act="library-folder"][data-id="demo-computer|"]').click();
  await page.locator("#list").getByRole("table").waitFor();
  await page.locator('.lhead [data-act="library-export-open"]').click();
  await page.getByRole("heading", { name: /^Build a rekordbox Device Library/ }).waitFor();
  await page.locator('.export-folder input[data-folder="Ben Klock"]').check();
  await page.locator('.export-folder input[data-folder="Hi-Res"]').check();
  await page.getByRole("button", { name: /^Build the export \(\d+ tracks?\)$/ }).waitFor();
  await shot("30-export-plan");
  await page.getByRole("button", { name: /^Build the export/ }).click();
  await page.getByText(/^(Analysing|Copying)/).first().waitFor();
  await page.waitForTimeout(500);
  await shot("31-export-building");
  await page.getByText(/^Export built: /).waitFor({ timeout: 20000 });
  await shot("32-export-done");
  await page.getByRole("button", { name: "Done" }).click();
  await page.locator('[data-act="demo-remove"][data-id="samsung-64"]').click();
  await page.waitForTimeout(500);
  // The Samsung was identified earlier, so selecting it opens the inspector and its report is one click.
  const samsung = await plugIn("samsung-64");
  await samsung.click();
  await page.locator('#insp [data-act="device-report"]').click();
  await sheet.getByRole("table").waitFor();
  await click("It's fine, go to the export step");
  await page.getByText("Put your music on this USB").waitFor({ timeout: 15000 });
  await page.getByRole("button", { name: "Erase this USB first" }).first().click();
  await page.getByRole("button", { name: /^Erase and prepare/ }).click();
  const offer = page.locator('[data-copy-source^="export:"]');
  await offer.getByRole("button", { name: /^Copy \d+ files/ }).waitFor({ timeout: 20000 });
  await shot("33-export-copy-offer");
  await offer.getByRole("button", { name: /^Copy \d+ files/ }).click();
  await page.getByText(/^Copying the export/).waitFor();
  await page.getByText("Verify before the gig").waitFor({ timeout: 20000 });
  await page.keyboard.press("Escape");

  // A drive that is already right: the report is the main action, and
  // preparing, which erases, is the second.
  await page.locator('[data-act="open-devtools"]').click();
  await page.getByRole("heading", { name: "Developer tools" }).waitFor();
  await shot("34-dev-tools");
  // The Samsung was identified earlier, so opening it goes straight to its check.
  await page.locator('[data-act="open-device"][data-id="demo:samsung-64"]').click();
  await page.getByText("Preparing it would erase everything on it.").waitFor({ timeout: 15000 });
  await shot("35-healthy-drive-check");
  await page.keyboard.press("Escape");
  await page.locator('[data-act="open-test-lab"]').click();
  await page.getByRole("heading", { name: "Test lab" }).waitFor();
  await shot("36-test-lab");
  await page.close();

  // The same window on a laptop panel and on a 2560 x 1440 display.
  for (const [name, w, h] of [["37-laptop", 1280, 800], ["38-large-display", 2560, 1440]]) {
    const p = await browser.newPage({ viewport: { width: w, height: h }, colorScheme: scheme });
    await p.goto(url);
    await p.locator("#list-title").waitFor();
    await p.locator('.tree [data-act="library-folder"][data-id="demo-computer|"]').click();
    await p.locator("#list").getByRole("table").waitFor();
    await p.locator(".tracks tbody tr").nth(5).locator("td.c-title").dblclick();
    await p.waitForTimeout(800);
    await shot(name, p);
    await p.close();
  }
}

try {
  await run("dark").catch(async (e) => {
    const pages = browser.contexts().flatMap((c) => c.pages());
    if (pages[0]) await pages[0].screenshot({ path: `${out}/FAILED.png`, fullPage: true });
    throw e;
  });
} finally {
  await browser.close();
  server.httpServer.close();
}
if (errors.length) {
  console.error("Browser errors:\n" + errors.join("\n"));
  process.exit(1);
}
