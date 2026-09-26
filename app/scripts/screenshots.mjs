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

async function run(scheme) {
  const page = await browser.newPage({ viewport: { width: 1180, height: 860 }, colorScheme: scheme });
  page.on("pageerror", (e) => errors.push(`${scheme}: ${e.message}`));
  page.on("console", (m) => m.type() === "error" && errors.push(`${scheme}: ${m.text()}`));
  const shot = async (name, full = false) => {
    await page.waitForTimeout(250);
    await page.screenshot({ path: `${out}/${scheme}-${name}.png`, fullPage: full });
    console.log(`${scheme}-${name}.png`);
  };
  const click = (name) => page.getByRole("button", { name, exact: false }).first().click();

  await page.goto(url);
  await page.getByText("Plug in a USB").waitFor();
  await shot("01-home");
  if (scheme === "light") return page.close();

  // USB A: the 128 GB SanDisk, GPT + exFAT with only a Device Library.
  await page.locator('[data-act="demo-insert"][data-id="sandisk-128"]').click();
  await page.getByText("We think this is").waitFor();
  await shot("02-identify-scanning");
  await page.getByText("Scanned").waitFor({ timeout: 10000 });
  await click("Yes, that's it");
  await page.getByText("Prepare this USB").waitFor();
  await shot("03-assessment");
  await click("Prepare this USB");
  await page.getByText("What are you preparing for?").waitFor();
  await shot("04-targets");
  await click("I know the equipment");
  await page.locator("#hwsearch").fill("old nexus");
  await page.waitForTimeout(400);
  await shot("05-hardware-search");
  await click("Back");
  await page.locator('[data-act="preset"][data-id="unknown_club"]').click();
  await click("Continue");
  await page.getByRole("table").waitFor();
  await page.locator('[data-act="cell"][data-id="cdj-3000x|library"]').click();
  await shot("06-report", true);
  await page.getByRole("button", { name: "Rebuild as MBR + FAT32" }).click();
  await page.getByText("You are about to erase:").waitFor();
  await shot("07-confirm-erase");
  await page.getByRole("button", { name: /^Erase and prepare/ }).click();
  await page.getByText("Preparing your USB").waitFor();
  await page.waitForTimeout(1500);
  await shot("08-preparing");
  await page.getByText("Now export from rekordbox").waitFor({ timeout: 15000 });
  await shot("09-export-handoff");
  await click("Simulate the rekordbox export");
  await page.getByText("Verify before the gig").waitFor({ timeout: 15000 });
  await click("Start verification");
  await page.waitForTimeout(1200);
  await shot("10-verifying");
  await page.getByText(/^Verified \d+ files?/).waitFor({ timeout: 10000 });
  await shot("11-verified");
  await click("Continue");
  await page.getByText("Almost there").waitFor();
  await shot("12-kit-progress", true);

  // USB 2: the 32 GB Kingston becomes the Legacy Rescue drive.
  await page.getByRole("button", { name: /Prepare the Legacy Rescue USB/ }).click();
  await page.getByText("Next: Legacy Rescue").waitFor();
  await shot("13-next-usb");
  await page.locator('[data-act="demo-insert"][data-id="kingston-32"]').click();
  await page.getByText("Scanned").waitFor({ timeout: 10000 });
  await click("Yes, that's it");
  await click("See detailed report");
  await page.getByRole("table").waitFor();
  await page.getByRole("button", { name: "Rebuild as MBR + FAT32" }).click();
  await page.getByRole("button", { name: /^Erase and prepare/ }).click();
  await page.getByText("Now export from rekordbox").waitFor({ timeout: 15000 });
  await shot("14-export-legacy");
  await click("Simulate the rekordbox export");
  await page.getByText("Verify before the gig").waitFor({ timeout: 15000 });
  await click("Start verification");
  await page.getByText(/^Verified \d+ files?/).waitFor({ timeout: 10000 });
  await shot("14b-verified-legacy");
  await click("Continue");
  await page.getByText("Legacy Rescue").first().waitFor();
  await shot("15-ready", true);
  await page.close();
}

try {
  await run("dark").catch(async (e) => {
    const pages = browser.contexts().flatMap((c) => c.pages());
    if (pages[0]) await pages[0].screenshot({ path: `${out}/FAILED.png`, fullPage: true });
    throw e;
  });
  await run("light");
} finally {
  await browser.close();
  server.httpServer.close();
}
if (errors.length) {
  console.error("Browser errors:\n" + errors.join("\n"));
  process.exit(1);
}
