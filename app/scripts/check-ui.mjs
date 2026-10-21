// Walks the prototype flow in a headless browser against the mock backend and
// checks that each screen shows what the engine reported. The expectations
// come from src/mock-data.json (generated from real engine output by
// `npm run mock-data`), so the check fails when the UI stops reflecting the
// engine rather than when wording changes. Usage: npm run build && npm run check-ui
import { chromium } from "playwright";
import { preview } from "vite";
import { mkdirSync, readFileSync } from "node:fs";

const data = JSON.parse(readFileSync(new URL("../src/mock-data.json", import.meta.url), "utf8"));

// The small label tables the UI renders from, mirrored from src/art.ts and src/ui.ts.
const VERDICT_LABEL = {
  ready: "Ready",
  expected_to_work: "Expected to work",
  partial: "Partly ready",
  unknown: "Not enough data",
  at_risk: "At risk",
  fix_needed: "Fix needed",
};
const FAILING_VERDICTS = ["fix_needed", "at_risk"];
const STATUS_WORD = { pass: "OK", fail: "Not OK", warn: "Warning", unknown: "Unknown", info: "Not present" };
const ROLE_LABEL = { main: "Main", legacy_rescue: "Legacy Rescue", backup: "Independent Backup" };
const RULES_STATUS = {
  seed: "(seed data, awaiting review against vendor documentation)",
  desk_reviewed: "(checked against vendor documentation and DJ forums, not yet tested on hardware)",
};
const LAYERS = ["physical", "partition", "filesystem", "library", "audio"];
const fsLabel = (fs) => (fs === "exfat" ? "exFAT" : "FAT32");
const formatLabel = (f) => `${f.scheme.toUpperCase()} + ${fsLabel(f.filesystem)}`;
const gb = (bytes) => `${(bytes / 1e9).toFixed(1).replace(/\.0$/, "")} GB`;

// The scenario: the 128 GB SanDisk checked for the default "unknown club" preset.
const DRIVE = "demo:sandisk-128";
const PRESET = "unknown_club";
const sandisk = data.drives[DRIVE].initial;
const A = sandisk.assessments[PRESET];
const preset = data.presets.find((p) => p.id === PRESET);
const plan = data.plan_unknown_club;
const demoName = (id) => id.replace(/^demo:/, "");

// ------------------------------------------------------------------ harness

const failures = [];
let checks = 0;
let screen = "start";
const screens = [];

function check(cond, what) {
  checks++;
  if (!cond) failures.push(`[${screen}] ${what}`);
  return cond;
}
function same(actual, expected, what) {
  return check(actual === expected, `${what}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
}
function includes(haystack, needle, what) {
  return check(String(haystack).includes(needle), `${what}: ${JSON.stringify(needle)} not found in ${JSON.stringify(String(haystack).slice(0, 300))}`);
}

// Checks the engine's own data holds what the README promises for this scenario,
// so a regenerated mock that changes the story fails here rather than silently
// passing weaker UI checks.
function checkScenarioData() {
  screen = "mock-data";
  const dev = (id) => A.devices.find((d) => d.device_id === id);
  check(preset, `preset ${PRESET} is in the mock`);
  check(FAILING_VERDICTS.includes(dev("cdj-2000")?.verdict), `the engine fails the SanDisk for the CDJ-2000 (got ${dev("cdj-2000")?.verdict})`);
  for (const id of ["cdj-3000x", "xdj-az"]) {
    const lib = dev(id)?.layers.find((l) => l.layer === "library");
    check(lib?.status === "fail" && /OneLibrary/.test(lib.headline), `the engine flags OneLibrary as missing for ${id} (got ${JSON.stringify(lib)})`);
  }
  same(A.recommended_format.scheme, "mbr", "the engine recommends MBR");
  same(A.recommended_format.filesystem, "fat32", "the engine recommends FAT32");
  check(A.fixes.some((f) => f.kind === "reformat" && f.scheme === "mbr" && f.filesystem === "fat32"), "the fixes include a rebuild as MBR + FAT32");
  const legacy = plan.roles.find((r) => r.role === "legacy_rescue");
  const main = plan.roles.find((r) => r.role === "main");
  check(legacy && main && legacy.volume_label !== main.volume_label, "the plan has a Legacy Rescue drive separate from Main");
  check(legacy && legacy.assigned_drive !== DRIVE, "the Legacy Rescue drive is not the SanDisk");
  same(main?.assigned_drive, DRIVE, "the plan assigns the SanDisk to Main");
  check(RULES_STATUS[data.rules.review_status], `the ruleset's review status ${JSON.stringify(data.rules.review_status)} has a footer note`);
}

// ------------------------------------------------------------------ browser

const port = 4175;
const server = await preview({ preview: { port, strictPort: true, host: "127.0.0.1" }, logLevel: "error" });
const browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH || undefined });
const browserErrors = [];

// Camelot neighbours, mirrored from keysMix in src/library.ts.
const keysMix = (a, b) => {
  if (!a || !b) return false;
  const na = parseInt(a, 10);
  const nb = parseInt(b, 10);
  return na === nb || (a.slice(-1) === b.slice(-1) && ((na % 12) + 1 === nb || (nb % 12) + 1 === na));
};

async function walk() {
  // Wide enough that the inspector shows by itself (1700 px of layout width at 90%).
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 }, colorScheme: "dark" });
  page.on("pageerror", (e) => browserErrors.push(e.message));
  page.on("console", (m) => m.type() === "error" && browserErrors.push(m.text()));
  const text = async (selector) => (await page.locator(selector).first().textContent()) ?? "";
  const click = (name) => page.locator(".sheet-body").getByRole("button", { name, exact: false }).first().click();
  /** The open sheet's text, or the window's when no sheet is open. */
  const view = async () => ((await page.locator(".sheet-body").count()) ? text(".sheet-body") : text("#main"));

  // Every screen is checked for the ruleset version and review status in the
  // status bar, and for rendering slips anywhere in the window.
  const at = async (name) => {
    screen = name;
    screens.push(name);
    await page.waitForTimeout(150);
    const foot = page.locator("footer.status");
    includes(await foot.textContent(), `Compatibility rules v${data.rules.version}`, "status bar names the ruleset version");
    const note = (await foot.locator("[title^='Compatibility rules']").getAttribute("title")) ?? "";
    includes(note, RULES_STATUS[data.rules.review_status], "status bar carries the ruleset's review note");
    includes(await foot.textContent(), "not yet tested on hardware", "status bar says the rules are not yet tested on hardware");
    check(!/\bundefined\b|\[object Object\]|\bNaN\b/.test(await text(".app")), "no undefined, NaN or [object Object] rendered");
    console.log(`  ${name}`);
  };

  await page.goto(`http://127.0.0.1:${port}/`);
  await page.locator("#list-title").waitFor();
  await at("window");
  await checkShell(page, text);
  for (const s of data.demo_sticks) {
    check(await page.locator(`[data-act="demo-insert"][data-id="${s.name}"]`).count(), `the Devices section offers ${s.title}`);
    same(await page.locator(`.sim .nm[title="${s.description}"]`).count(), 1, `the Devices section describes ${s.title}`);
  }
  includes(await text(".tree"), "Plug in a USB stick and it appears here.", "the tree says where a connected stick will show up");

  // The 128 GB SanDisk: GPT + exFAT with only a Device Library. It appears in
  // the tree and is read in the background; nothing takes over the window.
  await page.locator(`[data-act="demo-insert"][data-id="${demoName(DRIVE)}"]`).click();
  const dev = page.locator(`.dev[data-dev="${DRIVE}"]`);
  await dev.waitFor();
  same(await page.locator(".sheet").count(), 0, "plugging in a stick opens no sheet");
  includes(await text(".toast"), "is listed under Devices", "a toast says where the new stick is");
  await dev.locator(".badge.y").first().waitFor({ timeout: 15000 });
  await at("stick in the tree");
  const devText = (await dev.textContent()) ?? "";
  includes(devText, sandisk.summary.fs_label, "the stick shows its volume label");
  includes(devText, gb(data.drives[DRIVE].initial.card?.device?.size_bytes ?? sandisk.summary.size_bytes), "the stick shows its capacity");
  same((await dev.locator(".badge.y").allTextContents()).join(","), "RB", "the stick's badges show only the rekordbox Device Library the engine found");
  includes(await text(".titlebar"), "1 stick connected", "the title bar counts the stick");
  includes(await text("footer.status"), "1 stick connected", "the status bar counts the stick");

  // The title bar's player set is a real picker, the only one outside sheets.
  const picker = page.locator(".titlebar select#library-preset-title");
  same(await picker.count(), 1, "the title bar's Judging against is a picker");
  same(await page.locator("#list select").count(), 0, "the list header has no second player picker");
  includes(await text("footer.status"), "Runs locally", "the status bar says the app runs locally");
  const before = await picker.inputValue();
  const other = (await picker.locator("option").evaluateAll((os) => os.map((o) => o.value))).find((v) => v && v !== before);
  if (other) {
    await picker.selectOption(other);
    await page.waitForTimeout(300);
    same(await page.locator(".titlebar select#library-preset-title").inputValue(), other, "picking another player set in the title bar applies it");
    await page.locator(".titlebar select#library-preset-title").selectOption(before);
    await page.waitForTimeout(300);
  }

  // Tree sections and sticks fold like folders in a file explorer.
  await page.locator('[data-act="fold"][data-id="sec:collection"]').click();
  check((await page.locator('.tree [data-act="library-open"]').count()) === 0, "folding Collection hides its entries");
  await page.locator('[data-act="fold"][data-id="sec:collection"]').click();
  check((await page.locator('.tree [data-act="library-open"]').count()) > 0, "unfolding Collection shows them again");
  await dev.click();
  check((await page.locator(".dev.shut").count()) === 1, "clicking a stick folds it like a drawer");
  includes(await text("#insp"), "Check this stick", "the folded stick still shows in the inspector");
  await page.locator(".dev").first().click();
  check((await page.locator(".dev.shut").count()) === 0, "clicking the stick again unfolds it");
  await page.locator('#insp [data-act="device-check"]').click();
  await page.locator(".sheet").getByText("We think this is").waitFor();
  await page.locator(".sheet").getByText("Scanned").waitFor({ timeout: 10000 });
  await at("identify");
  const idText = await view();
  includes(idText, sandisk.summary.identification.display_name, "identification names the drive");
  includes(idText, sandisk.summary.scheme, "identification shows the partition scheme the engine read");
  includes(idText, sandisk.summary.filesystem, "identification shows the filesystem the engine read");
  includes(await text(".sheet-head"), sandisk.summary.identification.display_name, "the sheet's path names the drive");
  await click("Yes, that's it");
  await page.locator(".sheet").getByText("Prepare this USB").waitFor();
  await at("assessment");
  includes(await view(), sandisk.summary.headline, "assessment shows the engine's headline");

  await click("Prepare this USB");
  await page.getByText("What are you preparing for?").waitFor();
  await at("targets");
  for (const p of data.presets) check(await page.locator(`[data-act="preset"][data-id="${p.id}"]`).count(), `targets offer the ${p.id} preset`);

  await click("I know the equipment");
  await page.locator("#hwsearch").fill("old nexus");
  await page.waitForTimeout(400);
  await at("hardware-search");
  const hits = data.search["old nexus"] ?? [];
  check(hits.length > 0, "the mock has results for 'old nexus'");
  const grid = await text("#hwgrid");
  for (const h of hits) includes(grid, h.model, `searching 'old nexus' lists the ${h.model}`);
  await click("Back");

  await page.locator(`[data-act="preset"][data-id="${PRESET}"]`).click();
  await click("Continue");
  await page.locator(".sheet").getByRole("table").waitFor();
  await at("report");
  await checkReport(page, text);

  await page.getByRole("button", { name: `Rebuild as ${formatLabel(A.recommended_format)}` }).click();
  await page.getByText("You are about to erase:").waitFor();
  await at("confirm-erase");
  const c = sandisk.confirmation;
  const confirmText = await view();
  includes(confirmText, c.display_name, "confirmation names the drive about to be erased");
  includes(confirmText, gb(c.size_bytes), "confirmation shows the drive's capacity");
  includes(confirmText, `${c.files.toLocaleString("en-US")} file`, "confirmation counts the files that will be deleted");
  includes(confirmText, `MBR + ${fsLabel(A.recommended_format.filesystem)}`, "confirmation names the layout it will build");
  check(await page.getByRole("button", { name: /^Erase and prepare/ }).isEnabled(), "the erase button is enabled for an eligible drive");

  await page.getByRole("button", { name: /^Erase and prepare/ }).click();
  await page.getByText("Preparing your USB").waitFor();
  await at("preparing");
  same(await page.locator(".sheet-close").count(), 0, "the preparing sheet can't be closed while the drive is rebuilt");
  await page.getByText("Now export from rekordbox").waitFor({ timeout: 15000 });
  await at("export-handoff");
  await click("Simulate the rekordbox export");
  await page.getByText("Verify before the gig").waitFor({ timeout: 15000 });
  await at("verify");
  await click("Start verification");
  await page.getByText(/^Verified \d+ files?/).waitFor({ timeout: 10000 });
  await at("verified");
  const exported = data.drives[DRIVE].exported.summary;
  includes(await view(), `Verified ${Math.max(exported.tracks_scanned, 1)} file`, "verification reports the engine's file count");

  await click("Continue");
  await page.getByText("Almost there").waitFor();
  await at("kit-progress");
  const progress = await view();
  includes(progress, `1 of ${plan.roles.length} USBs ready`, "kit progress counts the plan's roles");
  const next = plan.roles[1];
  includes(progress, `Next: ${ROLE_LABEL[next.role]}`, "kit progress names the next role in the plan");
  includes(progress, gb(next.ideal_capacity[0]), "kit progress asks for the next role's capacity");
  includes(progress, ROLE_LABEL[plan.roles[0].role], "the verified drive is listed under its role");
  for (const d of A.devices) includes(progress, d.model, `the verified drive lists ${d.model}`);
  includes(await text("#tree-kit ~ .node"), "verified", "the Gig kit section marks the verified stick");
  includes(await text(".titlebar"), "1 ready", "the title bar offers the gig kit");

  await page.getByRole("button", { name: `Prepare the ${ROLE_LABEL[next.role]} USB` }).click();
  await page.getByText(`Next: ${ROLE_LABEL[next.role]}`).waitFor();
  await at("next-usb");
  includes(await view(), `Preparing ${plan.roles.length} USBs`, "the next-USB screen counts the plan's roles");
  await page.keyboard.press("Escape");
  same(await page.locator(".sheet").count(), 0, "Escape closes the sheet");
  await checkLibrary(page, text, at, view);
  await page.close();
  await checkSizes();
  await checkSorting();
  await checkTree();
}

// The four regions, the rail pages and the inspector toggle.
async function checkShell(page, text) {
  const lib = data.library[PRESET];
  for (const sel of [".titlebar", ".player", ".rail", ".tree", "#list", "#insp", "footer.status"]) same(await page.locator(sel).count(), 1, `the window has ${sel}`);
  const everything = lib.sources.reduce((n, s) => n + lib.tracks[s.id].length, 0);
  same((await text("#list-title")).trim(), "All tracks", "the window opens on the whole collection");
  includes(await text(".lhead .meta"), `${everything.toLocaleString("en-US")} tracks`, "the list header counts every track");
  same(await page.locator(".player-empty").count(), 1, "the player strip says nothing is loaded");
  for (const s of lib.sources) check((await page.locator(`.tree [data-act="library-folder"][data-id="${s.id}|"]`).count()) === 1, `the Playlists section lists ${s.name}`);
  await page.locator('[data-act="toggle-inspector"]').click();
  same(await page.locator("#insp").count(), 0, "the inspector button hides the inspector");
  await page.keyboard.press("Meta+i");
  same(await page.locator("#insp").count(), 1, "Cmd+I shows it again");
  await page.locator('[data-act="open-test-lab"]').click();
  await page.getByRole("heading", { name: "Test lab" }).waitFor();
  same(await page.locator(".tree").count(), 0, "Test lab takes the place of the library");
  same(await page.locator('[data-act="open-test-lab"][aria-current="page"]').count(), 1, "the rail marks Test lab");
  await page.locator('[data-act="open-devtools"]').click();
  await page.getByRole("heading", { name: "Developer tools" }).waitFor();
  check(true, "Dev tools opens from the rail");
  await page.locator('[data-act="panel-library"]').click();
  await page.locator("#list-title").waitFor();
  await page.locator('[data-act="open-library"]').first().click();
  await page.getByRole("heading", { name: "Your music" }).waitFor();
  await page.locator(".sheet-close").click();
  same(await page.locator(".sheet").count(), 0, "the close button closes a sheet");
}

// The window scales with the screen and gives up the inspector and columns when narrow.
async function checkSizes() {
  screen = "sizes";
  const big = await browser.newPage({ viewport: { width: 2560, height: 1440 }, colorScheme: "dark" });
  await big.goto(`http://127.0.0.1:${port}/`);
  await big.locator("#list-title").waitFor();
  const zoom = Number(await big.evaluate(() => document.documentElement.style.zoom));
  check(Math.abs(zoom - 2560 / 1900) < 0.01, `a 2560 px window draws about 35% larger (zoom ${zoom})`);
  same(await big.locator("#insp").count(), 1, "a wide window shows the inspector");
  await big.close();
  const small = await browser.newPage({ viewport: { width: 1180, height: 860 }, colorScheme: "dark" });
  small.on("pageerror", (e) => browserErrors.push(e.message));
  await small.goto(`http://127.0.0.1:${port}/`);
  await small.locator("#list-title").waitFor();
  same(Number(await small.evaluate(() => document.documentElement.style.zoom)), 0.9, "a small window draws no smaller than 90%");
  same(await small.locator("#insp").count(), 0, "a laptop-sized window starts without the inspector");
  same(await small.locator("th.c-plays").evaluate((el) => getComputedStyle(el).display), "none", "a narrow list drops the plays-on column first");
  check((await small.locator("th.c-title").evaluate((el) => el.getBoundingClientRect().width)) > 120, "the title column keeps its room in a narrow list");
  await small.close();
}

// Sorting: folders are playlists, so moving a track changes its folder, a
// copy puts it in a second one, the inbox sorts with number keys and Cmd+Z
// puts things back. It runs on its own page because sorting changes the
// mock's tracks for the rest of the visit.
async function checkSorting() {
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 }, colorScheme: "dark" });
  page.on("pageerror", (e) => browserErrors.push(e.message));
  page.on("console", (m) => m.type() === "error" && browserErrors.push(m.text()));
  const text = async (selector) => (await page.locator(selector).first().textContent()) ?? "";
  const at = async (name) => {
    screen = name;
    screens.push(name);
    await page.waitForTimeout(150);
    check(!/\bundefined\b|\[object Object\]|\bNaN\b/.test(await text(".app")), "no undefined, NaN or [object Object] rendered");
    console.log(`  ${name}`);
  };
  const tracks = data.library[PRESET].tracks["demo-computer"];
  const title = (t) => t.title ?? t.name;
  const rows = page.locator(".tracks tbody tr");
  const row = (t) => rows.filter({ has: page.locator("td.c-title", { hasText: title(t) }) });
  const node = (folder) => page.locator(`.tree [data-act="library-folder"][data-id="demo-computer|${folder}"]`);
  const toast = (msg) => page.locator(".toast", { hasText: msg }).waitFor({ timeout: 4000 });
  const rowCount = (n) => page.waitForFunction((n) => document.querySelectorAll(".tracks tbody tr").length === n, n, { timeout: 4000 });
  // Nested folders start folded in the tree, so tracks are found through their top-level folder, which lists everything under it.
  const top = (t) => t.folder.split("/")[0];
  const openFolder = async (folder) => {
    await node(folder).click();
    await page.waitForTimeout(200);
  };
  const lastPart = (folder) => folder.split("/").pop();
  /** The playlists named by the inspector's chips, from their titles since each chip also holds a remove button. */
  const chips = () => page.locator("#insp .inlist").evaluateAll((els) => els.map((e) => e.getAttribute("title")));
  const [first, second] = tracks.filter((t) => t.state === "ready");
  const copied = tracks.find((t) => t.state === "ready" && t.folder !== first.folder && t.folder !== "Hi-Res");
  const hiRes = tracks.filter((t) => t.folder === "Hi-Res");

  await page.goto(`http://127.0.0.1:${port}/`);
  await page.locator("#list-title").waitFor();
  await openFolder("");
  await page.locator("#list").getByRole("table").waitFor();

  // A track's inspector names the playlists that hold it.
  await row(first).locator("td.c-title").click();
  await page.locator("#insp .in-lists").waitFor();
  await at("sort-inspector");
  includes(await text("#insp .sorting .lbl"), "In playlists", "a selected track's inspector lists its playlists");
  same((await chips()).join(","), first.folder, "the chips name the track's one folder");
  same(await page.locator('.lhead [data-act="sort-undo"]').count(), 0, "there is nothing to undo before sorting");

  // Move to a new playlist: the folder appears in the tree.
  await page.locator("#library-move-to").selectOption("*new");
  await page.locator("#library-new-playlist").fill("00 INBOX");
  await page.locator("#library-new-playlist").press("Enter");
  await toast("Moved 1 track to 00 INBOX.");
  check(true, "a toast says the track moved to the new playlist");
  await node("00 INBOX").waitFor({ timeout: 4000 });
  includes(await node("00 INBOX").textContent(), "1", "the tree lists the new 00 INBOX folder with its track");
  same(await page.locator('.lhead [data-act="sort-undo"]').count(), 1, "the Undo button appears after a move");
  await row(second).locator("td.c-title").click();
  await page.locator("#insp .in-lists").waitFor();
  await page.locator("#library-move-to").selectOption("00 INBOX");
  await page.waitForFunction(() => document.querySelector('.tree [data-id="demo-computer|00 INBOX"] .ct')?.textContent?.trim() === "2", null, { timeout: 4000 });
  check(true, "Move to an existing folder puts a second track in the inbox");

  // The inbox: the triage bar numbers the playlists, and a number key moves
  // the selection there and selects the next track.
  await openFolder("00 INBOX");
  await at("sort-inbox");
  same(await rows.count(), 2, "the inbox lists the two moved tracks");
  same(await page.locator(".triage").count(), 1, "the inbox shows the triage bar");
  const slots = page.locator('.triage [data-act="sort-slot"]');
  check((await slots.count()) > 0 && (await slots.count()) <= 9, `the triage bar offers one to nine playlists (got ${await slots.count()})`);
  same((await slots.first().locator("kbd").textContent())?.trim(), "1", "the first slot is key 1");
  const slot1 = (await slots.first().getAttribute("data-id")) ?? "";
  check(slot1 && !/inbox/i.test(slot1), `slot 1 is a playlist other than the inbox (got ${JSON.stringify(slot1)})`);
  check(await slots.first().isDisabled(), "the slots wait for a selection");
  await row(first).locator("td.c-title").click();
  check(!(await slots.first().isDisabled()), "selecting a track enables the slots");
  await page.keyboard.press("1");
  await rowCount(1);
  await toast(`Moved 1 track to ${lastPart(slot1)}.`);
  same(await row(first).count(), 0, "key 1 moves the track out of the inbox");
  same(await page.locator(".tracks tbody tr.sel").count(), 1, "the next track is selected");
  includes(await text(".tracks tbody tr.sel td.c-title"), title(second), "the selection moves on to the remaining inbox track");
  includes(await text("#insp h2"), title(second), "the inspector follows to the next track");

  // Cmd+Z puts each change back, newest first, and Undo goes away at the end.
  await page.keyboard.press("Meta+z");
  await rowCount(2);
  await toast("Undone: 1 change put back.");
  same(await row(first).count(), 1, "Cmd+Z puts the track back in the inbox");
  same(await page.locator('.lhead [data-act="sort-undo"]').count(), 1, "Undo stays while there is more to undo");
  await page.keyboard.press("Meta+z");
  await rowCount(1);
  await page.keyboard.press("Meta+z");
  await rowCount(0);
  await page.waitForFunction(() => !document.querySelector('.lhead [data-act="sort-undo"]'), null, { timeout: 4000 });
  check(true, "the Undo button goes once nothing is left to undo");
  same(await node("00 INBOX").count(), 0, "undoing every move leaves no 00 INBOX folder");
  await openFolder(top(first));
  same(await row(first).count(), 1, "the first track is back in its own folder");
  await openFolder(top(second));
  same(await row(second).count(), 1, "the second track is back in its own folder");

  // Picking rows: Cmd-click adds one, Shift-click takes a range, Escape drops them.
  await openFolder("");
  await rows.nth(0).locator("td.c-title").click();
  await rows.nth(2).locator("td.c-title").click({ modifiers: ["Meta"] });
  await at("sort-picked");
  same(await page.locator(".tracks tbody tr.sel").count(), 2, "Cmd-click picks a second row");
  same((await text("#insp h2")).trim(), "2 tracks", "the inspector shows the 2-track selection");
  includes(await text("#insp .sorting .lbl"), "Sort", "the selection can be sorted together");
  await rows.nth(5).locator("td.c-title").click({ modifiers: ["Shift"] });
  same(await page.locator(".tracks tbody tr.sel").count(), 4, "Shift-click picks the range from the last picked row");
  same((await text("#insp h2")).trim(), "4 tracks", "the inspector counts the range");
  await page.keyboard.press("Escape");
  await page.waitForTimeout(150);
  same(await page.locator(".tracks tbody tr.sel").count(), 1, "Escape drops the picked rows");
  check((await text("#insp h2")).trim() !== "4 tracks", "the inspector leaves the selection view after Escape");

  // Add to playlist copies: the track stays and also shows up in the target.
  await openFolder(top(copied));
  const here = await rows.count();
  await row(copied).locator("td.c-title").click();
  await page.locator("#insp .in-lists").waitFor();
  await page.locator("#library-add-to").selectOption("Hi-Res");
  await toast("Added 1 track to Hi-Res.");
  await at("sort-added");
  same(await rows.count(), here, "adding a copy leaves the folder's list as it was");
  same(await row(copied).count(), 1, "the copied track stays in its folder");
  await page.waitForFunction(() => document.querySelectorAll("#insp .inlist").length === 2, null, { timeout: 4000 });
  same((await chips()).sort().join(","), ["Hi-Res", copied.folder].sort().join(","), "the inspector lists both playlists");
  await openFolder("Hi-Res");
  same(await rows.count(), hiRes.length + 1, "the target playlist gains the copy");
  same(await row(copied).count(), 1, "the copy is listed in the target playlist");
  await page.close();
}

// The music library: every source and every track shows what the engine's
// scan and its per-player verdicts say, for the preset selected above.
// The playlist tree works like a file manager: folders in name order, a
// context menu, a name typed in place, playlists dragged into folders, and
// every change undone with Cmd+Z.
async function checkTree() {
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 }, colorScheme: "dark" });
  page.on("pageerror", (e) => browserErrors.push(e.message));
  page.on("console", (m) => m.type() === "error" && browserErrors.push(m.text()));
  const text = async (selector) => (await page.locator(selector).first().textContent()) ?? "";
  const at = async (name) => {
    screen = name;
    screens.push(name);
    await page.waitForTimeout(150);
    check(!/\bundefined\b|\[object Object\]|\bNaN\b/.test(await text(".app")), "no undefined, NaN or [object Object] rendered");
    await page.screenshot({ path: `test-results/${name}.png` }).catch(() => {});
    console.log(`  ${name}`);
  };
  mkdirSync("test-results", { recursive: true });
  const node = (folder) => page.locator(`.tree [data-act="library-folder"][data-id="demo-computer|${folder}"]`);
  const toggle = (folder) => page.locator(`.tree [data-act="library-toggle"][data-id="demo-computer|${folder}"]`);
  const menu = page.locator(".ctx");
  const item = (label) => menu.getByRole("menuitem", { name: label });
  const toast = (msg) => page.locator(".toast", { hasText: msg }).waitFor({ timeout: 4000 });
  const gone = (folder) => page.waitForFunction((id) => !document.querySelector(`.tree [data-act="library-folder"][data-id="${id}"]`), `demo-computer|${folder}`, { timeout: 4000 });
  const count = async (folder) => (await node(folder).locator(".ct").textContent())?.trim();
  const hiRes = data.library[PRESET].tracks["demo-computer"].filter((t) => t.folder === "Hi-Res").length;

  await page.goto(`http://127.0.0.1:${port}/`);
  await page.locator("#list-title").waitFor();
  await node("").click();
  await page.locator("#list").getByRole("table").waitFor();
  await at("tree-order");

  // Top-level folders are in name order without regard to case, and a
  // folder with subfolders gets the folder glyph where a playlist gets the list.
  const tops = await page.locator('.tree [data-act="library-folder"][data-id^="demo-computer|"]').evaluateAll((els) =>
    els.map((e) => e.getAttribute("data-id").slice("demo-computer|".length)).filter((p) => p && !p.includes("/")),
  );
  const sorted = [...tops].sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase()));
  same(tops.join(","), sorted.join(","), "top-level playlists are listed in case-insensitive name order");
  includes(await node("Kolsch").innerHTML(), "M3 7h6l2 2h10v10H3z", "a folder with subfolders shows the folder glyph");
  includes(await node("Hi-Res").innerHTML(), "<circle", "a playlist without subfolders shows the playlist glyph");
  same(await node("Hi-Res").getAttribute("draggable"), "true", "a playlist can be dragged");

  // The context menu on a playlist.
  await node("Hi-Res").click({ button: "right" });
  await menu.waitFor({ timeout: 2000 });
  await at("tree-menu");
  same((await text(".ctx .ctx-title")).trim(), "Hi-Res", "the menu is titled with the playlist's name");
  for (const label of ["New playlist", "Rename", "Remove", "Export to USB"]) check((await item(label).count()) === 1, `the menu offers ${label}`);
  await page.keyboard.press("Escape");
  await page.waitForTimeout(150);
  same(await menu.count(), 0, "Escape closes the menu");

  // New playlist on the source: it appears empty, with its name ready to type.
  await node("").click({ button: "right" });
  await menu.waitFor({ timeout: 2000 });
  same(await item("Rename").count(), 0, "the source itself has no Rename");
  await item("New playlist").click();
  await toast("Made New playlist.");
  await page.locator("#library-rename").waitFor({ timeout: 4000 });
  await at("tree-rename");
  same(await page.locator("#library-rename").inputValue(), "New playlist", "the new playlist's name is ready to type");
  check(await page.locator("#library-rename").evaluate((e) => document.activeElement === e), "the name field has the focus");
  await page.keyboard.type("Friday warmup");
  await page.keyboard.press("Enter");
  await toast("Renamed New playlist to Friday warmup.");
  await node("Friday warmup").waitFor({ timeout: 4000 });
  same(await count("Friday warmup"), "0", "the renamed playlist is listed, empty");
  same(await page.locator('.lhead [data-act="sort-undo"]').count(), 1, "Undo is offered after the tree changed");

  // Rename from the menu, then Escape keeps the old name.
  await node("Friday warmup").click({ button: "right" });
  await item("Rename").click();
  await page.locator("#library-rename").waitFor({ timeout: 2000 });
  await page.keyboard.type("Nope");
  await page.keyboard.press("Escape");
  await page.waitForTimeout(150);
  same(await node("Friday warmup").count(), 1, "Escape leaves the name as it was");
  same(await page.locator("#library-rename").count(), 0, "and closes the name field");

  // A playlist dragged onto a folder moves into it.
  await page.dragAndDrop(`.tree [data-act="library-folder"][data-id="demo-computer|Friday warmup"]`, `.tree [data-act="library-folder"][data-id="demo-computer|Kolsch"]`);
  await toast("Moved Friday warmup to Kolsch.");
  await gone("Friday warmup");
  if ((await toggle("Kolsch").getAttribute("aria-expanded")) !== "true") await toggle("Kolsch").click();
  await node("Kolsch/Friday warmup").waitFor({ timeout: 4000 });
  await at("tree-moved");
  check(true, "the dragged playlist is listed inside the folder");

  // Renaming a playlist with tracks keeps them.
  await node("Hi-Res").click({ button: "right" });
  await item("Rename").click();
  await page.locator("#library-rename").waitFor({ timeout: 2000 });
  await page.keyboard.type("Hi Res Masters");
  await page.keyboard.press("Enter");
  await toast("Renamed Hi-Res to Hi Res Masters.");
  await node("Hi Res Masters").waitFor({ timeout: 4000 });
  same(await count("Hi Res Masters"), String(hiRes), "the renamed playlist keeps its tracks");
  await node("Hi Res Masters").click();
  await page.waitForFunction((n) => document.querySelectorAll(".tracks tbody tr").length === n, hiRes, { timeout: 4000 });
  check(true, "opening the renamed playlist lists its tracks");

  // Remove from the menu, and the keyboard: Return renames, Cmd+Delete removes.
  await node("Kolsch/Friday warmup").click({ button: "right" });
  await item("Remove").click();
  await toast("Moved Friday warmup to _Removed by BoothReady.");
  await gone("Kolsch/Friday warmup");
  check(true, "Remove takes the playlist out of the tree");
  await node("Hi Res Masters").focus();
  await page.keyboard.press("Enter");
  await page.locator("#library-rename").waitFor({ timeout: 2000 });
  check(true, "Return on a focused playlist opens its name");
  await page.keyboard.press("Escape");
  await page.waitForTimeout(150);
  await node("Hi Res Masters").focus();
  await page.keyboard.press("Meta+Backspace");
  await toast("Moved Hi Res Masters to _Removed by BoothReady.");
  await gone("Hi Res Masters");
  check(true, "Cmd+Delete removes the focused playlist");
  await at("tree-removed");

  // Cmd+Z walks every change back, newest first.
  await page.locator("body").click({ position: { x: 5, y: 5 } }).catch(() => {});
  await page.keyboard.press("Meta+z");
  await node("Hi Res Masters").waitFor({ timeout: 4000 });
  same(await count("Hi Res Masters"), String(hiRes), "undo brings the removed playlist back with its tracks");
  await page.keyboard.press("Meta+z");
  await node("Kolsch/Friday warmup").waitFor({ timeout: 4000 });
  await page.keyboard.press("Meta+z");
  await node("Hi-Res").waitFor({ timeout: 4000 });
  check(true, "undo restores the old name");
  await page.keyboard.press("Meta+z");
  await node("Friday warmup").waitFor({ timeout: 4000 });
  check(true, "undo moves the playlist back out of the folder");
  await page.keyboard.press("Meta+z");
  await node("New playlist").waitFor({ timeout: 4000 });
  await page.keyboard.press("Meta+z");
  await gone("New playlist");
  check(true, "undoing the new playlist takes the empty folder away");
  await page.waitForFunction(() => !document.querySelector('.lhead [data-act="sort-undo"]'), null, { timeout: 4000 });
  check(true, "nothing is left to undo");
  await page.close();
}

async function checkLibrary(page, text, at, view) {
  const lib = data.library[PRESET];
  const STATE_LABEL = { ready: "Ready", cloud_only: "In the cloud", blocked: "Won't play everywhere", unchecked: "No data", unreadable: "Unreadable" };
  const count = (n, one) => `${n.toLocaleString("en-US")} ${n === 1 ? one : `${one}s`}`;

  screen = "mock-data";
  const byId = Object.fromEntries(lib.sources.map((s) => [s.id, s]));
  check(byId["demo-computer"]?.location.kind === "computer", "the demo library has a folder on the computer");
  check(byId["demo-cloud"]?.location.kind === "cloud" && byId["demo-cloud"].states.cloud_only > 0, "the demo cloud folder has tracks that are still online");
  check(byId["demo-archive"]?.connected === false && byId["demo-archive"].states, "the unplugged archive drive keeps its last scan");
  check(byId["demo-computer"]?.states.blocked > 0 && byId["demo-computer"].states.unreadable > 0, "the demo computer folder has blocked and unreadable tracks");
  for (const t of lib.tracks["demo-cloud"].filter((t) => t.state === "cloud_only")) {
    check(t.detail === "" && t.issues.length === 0, `the engine made no format claim about the online file ${t.name}`);
  }

  await page.locator('[data-act="open-library"]').first().click();
  await page.getByRole("heading", { name: "Your music" }).waitFor();
  await at("library sources");
  const cards = page.locator(".source");
  same(await cards.count(), lib.sources.length, "a card per music source");
  for (const s of lib.sources) {
    const card = page.locator(`.source[data-source="${s.id}"]`);
    const cardText = (await card.textContent()) ?? "";
    includes(cardText, s.name, `source ${s.id} shows its folder name`);
    includes(cardText, s.path, `source ${s.id} shows its path`);
    includes(cardText, count(s.totals.tracks, "track"), `source ${s.name} shows the scanned track count`);
    same((await card.locator(".pill.offline").count()) > 0, !s.connected, `source ${s.name} is marked not connected only when it isn't`);
    for (const [state, n] of Object.entries(s.states)) {
      if (n > 0) includes(cardText, `${STATE_LABEL[state]} ${n.toLocaleString("en-US")}`, `source ${s.name} counts its ${state} tracks`);
      else check(!cardText.includes(STATE_LABEL[state]), `source ${s.name} shows no ${state} tracks`);
    }
    if (!s.connected) check(await card.locator('[data-act="library-scan"]').isDisabled(), `source ${s.name} can't be scanned while unplugged`);
  }
  const cloudTotal = lib.sources.reduce((n, s) => n + s.states.cloud_only, 0);
  includes(await view(), `${count(cloudTotal, "track")} are still in the cloud`, "the library says how many tracks are still online");

  for (const id of ["demo-computer", "demo-cloud"]) {
    const s = byId[id];
    const tracks = lib.tracks[id];
    if (!(await page.locator(".sheet").count())) await page.locator('[data-act="open-library"]').first().click();
    await page.locator(`.sheet [data-act="library-open"][data-id="${id}"]`).click();
    await page.locator("#list").getByRole("table").waitFor();
    await at(`library-source ${id}`);
    same(await page.locator(".sheet").count(), 0, "opening a source closes the sources sheet");
    same((await text("#list-title")).trim(), s.name, `the list is titled ${s.name}`);
    const rows = page.locator(".tracks tbody tr");
    same(await rows.count(), tracks.length, `${s.name} lists every scanned track`);
    for (const [i, t] of tracks.entries()) {
      const row = (await rows.nth(i).textContent()) ?? "";
      includes(row, t.title ?? t.name, `row ${i + 1} of ${s.name} is ${t.title ?? t.name}`);
      if (t.artist) includes(row, t.artist, `${t.name} shows its artist tag`);
      if (t.camelot) includes(row, t.camelot, `${t.name} shows its key on the Camelot wheel`);
      if (t.bpm !== null) includes(row, String(Math.round(t.bpm * 100) / 100), `${t.name} shows its tempo`);
      includes(row, STATE_LABEL[t.state], `${t.name} shows the engine's state`);
      if (t.state === "cloud_only") includes(row, "not checked", `${t.name} is shown as not checked`);
      // The reasons are in the inspector for the selected row.
      await rows.nth(i).locator("td.c-title").click();
      same(await rows.nth(i).getAttribute("class").then((c) => c?.includes("sel")), true, `selecting ${t.name} highlights its row`);
      const insp = (await text("#insp")) ?? "";
      includes(insp, t.title ?? t.name, `the inspector shows ${t.name}`);
      for (const issue of t.issues) {
        includes(insp, issue.model, `the inspector names the player that rejects ${t.name}`);
        includes(insp, issue.reason, `the inspector gives the engine's reason for ${issue.model}`);
      }
      if (t.note) includes(insp, t.note, `the inspector says why ${t.name} is unreadable`);
      if (t.detail) includes(insp, t.detail, `the inspector shows the format of ${t.name}`);
    }
    for (const [state, n] of Object.entries(s.states)) {
      if (!n) continue;
      await page.locator(`[data-act="library-filter"][data-id="${state}"]`).click();
      await page.waitForTimeout(120);
      same(await page.locator(".tracks tbody tr").count(), n, `the ${state} filter on ${s.name} shows its ${n} tracks`);
    }
  }

  // The tree: a folder narrows the list to that folder, the collection holds
  // every source's tracks, and its entries narrow it to one state.
  await page.locator('.tree [data-act="library-folder"][data-id="demo-computer|"]').click();
  await page.locator("#list").getByRole("table").waitFor();
  const hiRes = lib.tracks["demo-computer"].filter((t) => t.folder === "Hi-Res");
  await page.locator('[data-act="library-folder"][data-id="demo-computer|Hi-Res"]').click();
  await page.waitForTimeout(150);
  await at("library-folder Hi-Res");
  same(await page.locator(".tracks tbody tr").count(), hiRes.length, "a folder in the tree lists only its tracks");
  includes(await text("#list-title"), "Hi-Res", "the selected folder names the list");
  same(await page.locator(".tree .node.on").count(), 1, "the tree marks one node");
  await page.locator('[data-act="library-open"][data-id="*"]').click();
  await page.locator("#list").getByRole("table").waitFor();
  await page.waitForTimeout(150);
  await at("library-collection-all");
  const everything = lib.sources.reduce((n, s) => n + lib.tracks[s.id].length, 0);
  includes(await text(".tracks-foot"), `of ${everything.toLocaleString("en-US")}`, "the collection holds every source's tracks");
  const blocked = lib.sources.reduce((n, s) => n + s.states.blocked, 0);
  await page.locator('.tree [data-act="library-collection"][data-id="blocked"]').click();
  await page.waitForTimeout(200);
  same((await text("#list-title")).trim(), STATE_LABEL.blocked, "a Collection entry names the list after its state");
  same(await page.locator(".tracks tbody tr").count(), Math.min(100, blocked), "the Won't play everywhere entry lists the blocked tracks of every source");
  await page.locator('[data-act="library-open"][data-id="*"]').click();
  await page.waitForTimeout(200);
  await page.locator("#list-scroller").evaluate((el) => (el.scrollTop = el.scrollHeight));
  await page.waitForFunction((n) => document.querySelectorAll(".tracks tbody tr").length === n, everything, { timeout: 4000 }).catch(() => {});
  same(await page.locator(".tracks tbody tr").count(), everything, "scrolling to the end of the list loads the rest");
  await page.locator('[data-act="library-folder"][data-id="demo-computer|"]').click();
  await page.locator("#list").getByRole("table").waitFor();

  // Tags: the engine read them, search finds an artist, and a heading
  // orders the list with untagged tracks last.
  check(lib.tracks["demo-computer"].filter((t) => t.state !== "unreadable").some((t) => t.title === null), "one demo track is left untagged");
  check(lib.tracks["demo-computer"].filter((t) => t.artist).length >= 6, "the demo tracks carry artist tags");
  await page.locator("#library-search").fill("klock");
  await page.waitForTimeout(400);
  const klock = lib.tracks["demo-computer"].filter((t) => t.artist === "Ben Klock").length;
  same(await page.locator(".tracks tbody tr").count(), klock, "searching an artist finds that artist's tracks");
  same(await page.evaluate(() => document.activeElement?.id), "library-search", "the search box keeps focus while the list updates");
  await page.locator("#library-search").fill("");
  await page.waitForTimeout(400);
  await page.locator('[data-act="library-sort"][data-id="bpm"]').click();
  await page.waitForTimeout(200);
  await at("library-sort bpm");
  const tempos = await page.locator(".tracks tbody td.bpm").allTextContents();
  const known = tempos.map((x) => x.trim()).filter((x) => x !== "·").map(Number);
  check(known.length > 0 && known.every((v, i) => i === 0 || known[i - 1] <= v), "sorting by BPM puts the slowest first");
  check(tempos.map((x) => x.trim()).slice(known.length).every((x) => x === "·"), "tracks without a tempo come last");
  same(await page.locator('th[aria-sort="ascending"]').count(), 1, "the sorted column says so");
  await page.locator('[data-act="library-sort"][data-id="bpm"]').click();
  await page.waitForTimeout(200);
  const down = (await page.locator(".tracks tbody td.bpm").allTextContents()).map((x) => x.trim()).filter((x) => x !== "·").map(Number);
  check(down.every((v, i) => i === 0 || down[i - 1] >= v), "the same heading again reverses the order");
  await page.locator('[data-act="library-sort"][data-id=""]').click();
  await page.waitForTimeout(200);

  // Playing: a row's number is its play key, the strip along the top
  // follows the track across pages, and unreadable files can't be started.
  for (const s of lib.sources) check(lib.tracks[s.id].every((t) => t.source === s.id), `every track of ${s.name} names its source`);
  const tracks = lib.tracks["demo-computer"];
  const unreadable = tracks.filter((t) => t.state === "unreadable").length;
  same(await page.locator("tbody tr.unreadable .rb-play").count(), 0, "an unreadable track has no play key");
  check(unreadable === 0 || (await page.locator("tbody tr.unreadable").count()) > 0, "an unreadable track is listed");
  const second = page.locator(".rb-play").nth(1);
  const secondName = (await second.getAttribute("aria-label"))?.replace(/^Play /, "");
  await second.click();
  await page.waitForFunction(() => document.querySelector(".player-key.play")?.getAttribute("aria-label") === "Pause");
  await at("library-player");
  check((await text(".player-name")).trim().startsWith(secondName), "the player shows the track that was clicked");
  same((await text("tr.playing td.c-title")).trim(), secondName, "the playing row is marked");
  const loaded = tracks.find((t) => (t.title ?? t.name) === secondName);
  if (loaded?.camelot) includes(await text(".player .readout"), loaded.camelot, "the strip shows the loaded track's key");
  const mixing = tracks.filter((t) => t !== loaded && keysMix(loaded?.camelot, t.camelot)).length;
  same(await page.locator("td.c-key.match").count(), mixing, "keys that mix with the loaded track are marked");
  // Generous: under load the headless browser can take seconds to start the audio.
  await page.waitForFunction(() => document.getElementById("player-elapsed")?.textContent !== "0:00", null, { timeout: 15000 });
  check(true, "the clock runs while the track plays");
  await page.locator('[data-act="library-next"]').click();
  await page.waitForFunction((name) => document.querySelector("tr.playing td.c-title")?.textContent?.trim() !== name, secondName);
  check(!(await text(".player-name")).trim().startsWith(secondName), "next moves to the following track");
  await page.locator("#list-title").click();
  await page.keyboard.press(" ");
  await page.waitForFunction(() => document.querySelector(".player-key.play")?.getAttribute("aria-label") === "Play");
  check(true, "the space bar pauses");
  // The keyboard: arrows move the selection and Return loads it; a double-click loads a row too.
  await page.locator(".tracks tbody tr").nth(0).locator("td.c-title").click();
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  same(await page.locator(".tracks tbody tr.sel").getAttribute("data-row"), "2", "the down arrow moves the selection");
  await page.keyboard.press("Enter");
  await page.waitForFunction((name) => document.querySelector(".player-name")?.textContent?.trim().startsWith(name), tracks[2].title ?? tracks[2].name);
  check(true, "Return loads the selected track");
  await page.locator(".tracks tbody tr").nth(3).locator("td.c-title").dblclick();
  await page.waitForFunction((name) => document.querySelector(".player-name")?.textContent?.trim().startsWith(name), tracks[3].title ?? tracks[3].name);
  check(true, "a double-click loads the row");
  await page.locator('[data-act="open-test-lab"]').click();
  check((await page.locator(".player:not(.player-empty)").count()) === 1, "the player stays loaded on other pages");
  await page.locator('[data-act="library-stop-play"]').click();
  same(await page.locator(".player-empty").count(), 1, "stop unloads the player");
  await page.locator('[data-act="panel-library"]').click();
  await page.locator('[data-act="library-folder"][data-id="demo-cloud|"]').click();
  await page.locator("#list").getByRole("table").waitFor();

  // Collecting the cloud folder: the plan matches the scan, and the new
  // collection shows what the engine read once the tracks were local.
  const cloud = byId["demo-cloud"];
  const made = lib.collected;
  screen = "mock-data";
  check(made.source.collected_from === cloud.name, "the engine's collection records the source it came from");
  same(made.source.states.cloud_only, 0, "no collected track is still online");
  same(made.tracks.length, lib.tracks["demo-cloud"].length, "the engine collected every track of the cloud folder");
  check(made.tracks.some((t) => t.state === "blocked"), "a collected track is rejected by a player once its format can be read");

  await page.locator('[data-act="library-filter"][data-id="all"]').click();
  await page.locator('[data-act="library-collect-open"]').click();
  await page.getByRole("heading", { name: /^Bring \d+ tracks? onto this computer$/ }).waitFor();
  await at("library-collect plan");
  const plan = (await text(".collect")) ?? "";
  includes(plan, `Bring ${count(made.tracks.length, "track")} onto this computer`, "the plan counts the tracks to collect");
  includes(plan, `From the cloud${cloud.states.cloud_only.toLocaleString("en-US")},`, "the plan counts the tracks that will be downloaded");
  includes(plan, "Your cloud app downloads them as they are read.", "the plan says the cloud app does the downloading");
  includes(plan, "/Music/BoothReady/", "the plan shows the folder the copy goes to");
  await page.getByRole("button", { name: `Collect ${count(made.tracks.length, "track")}` }).click();
  await page.getByText(`Collected and verified ${count(made.tracks.length, "track")}.`).waitFor({ timeout: 15000 });
  await page.getByRole("button", { name: "Open the collection" }).click();
  await page.locator("#list").getByRole("table").waitFor();
  await at("library-collection");
  const rows = page.locator(".tracks tbody tr");
  same(await rows.count(), made.tracks.length, "the collection lists every collected track");
  for (const [i, t] of made.tracks.entries()) {
    const row = (await rows.nth(i).textContent()) ?? "";
    includes(row, t.title ?? t.name, `collected row ${i + 1} is ${t.name}`);
    includes(row, STATE_LABEL[t.state], `${t.name} shows the state the engine found after collecting`);
    includes(row, t.detail, `${t.name} shows the format the engine read`);
    check(!row.includes("not checked") && !row.includes(STATE_LABEL.cloud_only), `${t.name} is no longer shown as online`);
    await rows.nth(i).locator("td.c-title").click();
    for (const issue of t.issues) includes(await text("#insp"), issue.reason, `${t.name} gives the engine's reason for ${issue.model}`);
  }
  check((await page.locator('[data-act="library-collect-open"]').count()) === 0, "a collection doesn't offer to be collected again");
  const next = (await text(".nextstep")) ?? "";
  includes(next, "Export this collection to your USB", "a collection says what to do next");
  check((await page.locator('.nextstep [data-act="library-export-open"]').count()) === 1, "the next step offers to build the export here");
  check((await page.locator('.nextstep [data-act="open-rekordbox"]').count()) === 1, "the next step still offers rekordbox");
  same(await page.locator('.tree [data-act="library-folder"][data-id$="|"]').count(), lib.sources.length + 1, "the new collection appears in the tree");
  await page.locator('[data-act="open-library"]').first().click();
  const card = (await page.locator(".source").last().textContent()) ?? "";
  includes(card, "Local collection", "the library marks the new source as a local collection");
  includes(card, `A verified copy of tracks from ${cloud.name}.`, "the collection card names its origin");
  await checkExport(page, text, at, view);
}

// Export to USB: ticked folders become playlists, the plan counts the
// tracks and estimates the analysis, the build reports progress, and the
// finished export is offered to a prepared USB by the copy flow.
async function checkExport(page, text, at, view) {
  const lib = data.library[PRESET];
  const count = (n, one) => `${n.toLocaleString("en-US")} ${n === 1 ? one : `${one}s`}`;
  const computer = lib.sources.find((s) => s.id === "demo-computer");
  const tracks = lib.tracks["demo-computer"];
  const holdable = (t) => t.state !== "unreadable" && /\.(mp3|m4a|flac|wav|aif|aiff)$/i.test(t.name);
  const under = (folder) => tracks.filter((t) => t.folder === folder || t.folder.startsWith(`${folder}/`));
  const picked = [...under("Ben Klock"), ...under("Hi-Res")].filter(holdable);
  const leftOut = [...under("Ben Klock"), ...under("Hi-Res")].length - picked.length;
  const exportButton = () => page.locator('.lhead [data-act="library-export-open"]');

  await page.locator('.sheet [data-act="library-open"][data-id="demo-computer"]').click();
  await page.locator("#list").getByRole("table").waitFor();
  await exportButton().click();
  await page.getByRole("heading", { name: `Build a rekordbox Device Library from ${computer.name}` }).waitFor();
  await at("library-export open");
  const folders = page.locator(".export-folder");
  const folderSet = new Set(tracks.map((t) => t.folder.split("/")).flatMap((parts) => parts.map((_, i) => parts.slice(0, i + 1).join("/"))));
  same(await folders.count(), folderSet.size, "every folder of the source can be ticked");
  check(await page.getByRole("button", { name: /^Build the export/ }).isDisabled(), "nothing can be built before a folder is ticked");
  // A long folder list keeps its place when a folder deep in it is ticked.
  await page.addStyleTag({ content: ".export-folders { max-height: 100px !important; }" });
  const box = page.locator('[data-keep-scroll="export-folders"]');
  await box.evaluate((el) => (el.scrollTop = el.scrollHeight));
  await page.locator('.export-folder input[data-folder="Kolsch"]').scrollIntoViewIfNeeded();
  const scrolled = await box.evaluate((el) => el.scrollTop);
  await page.locator('.export-folder input[data-folder="Kolsch"]').check();
  await page.getByRole("button", { name: /^Build the export \(/ }).waitFor();
  check(scrolled > 0 && (await box.evaluate((el) => el.scrollTop)) === scrolled, "ticking a folder keeps the folder list where it was scrolled");
  await page.locator('.export-folder input[data-folder="Kolsch"]').uncheck();
  await page.getByRole("button", { name: "Build the export" }).waitFor();
  await page.evaluate(() => document.querySelectorAll("style").forEach((s) => s.textContent?.includes("max-height: 100px") && s.remove()));
  await page.locator('.export-folder input[data-folder="Ben Klock"]').check();
  await page.locator('.export-folder input[data-folder="Hi-Res"]').check();
  await page.getByRole("button", { name: `Build the export (${count(picked.length, "track")})` }).waitFor();
  await at("library-export plan");
  const plan = (await text(".export")) ?? "";
  includes(plan, `Ben Klock (${under("Ben Klock").filter(holdable).length}), Hi-Res (${under("Hi-Res").filter(holdable).length})`, "the plan names a playlist per ticked folder with its track count");
  includes(plan, `${count(picked.length, "track")} to analyse`, "the plan counts the tracks to analyse");
  includes(plan, picked.length * 6 < 45 ? "under a minute" : `about ${Math.round((picked.length * 6) / 60)} min`, "the plan estimates the analysis at six seconds a track");
  includes(plan, "/exports/", "the plan shows the export's own folder on this computer");
  includes(plan, "Device Library only", "the sheet says the export holds a Device Library only");
  check(leftOut === 0 || plan.includes(`${count(leftOut, "file")} left out`), "the plan counts the files left out");
  const name = (await page.locator("#library-export-name").inputValue()) ?? "";
  includes(name, computer.name, "the export is named after the source");

  await page.getByRole("button", { name: /^Build the export/ }).click();
  await page.getByText(/^(Analysing|Copying|Writing the database|Finishing)/).first().waitFor();
  await at("library-export building");
  includes(await text(".export"), `of ${count(picked.length, "track")}`, "the build counts the tracks as it goes");
  check((await page.locator('[data-act="library-export-stop"]').count()) === 1, "the build can be stopped");
  same(await page.locator(".sheet-close").count(), 0, "the export sheet can't be closed while it builds");
  includes(await text("footer.status"), `Building ${name}`, "the status bar shows the build");
  await page.getByText(`Export built: ${count(picked.length, "track")} in 2 playlists`).waitFor({ timeout: 20000 });
  await at("library-export done");
  const done = (await text(".export")) ?? "";
  includes(done, "Nothing has been written to a USB yet.", "the finished export says no USB was written");
  includes(done, "with beat grids and waveforms for all of them", "the finished export says every track was analysed");
  check(!done.includes("couldn't be analysed"), "a fully analysed export raises no analysis warning");
  includes(done, `/exports/${name}`, "the finished export names its folder");
  includes(done, "select it under Devices", "the finished export says where to put it on a stick");

  // A second plan for the same folders has nothing left to analyse.
  await page.locator('[data-act="library-export-close"]').click();
  await exportButton().click();
  await page.locator('.export-folder input[data-folder="Hi-Res"]').check();
  await page.getByRole("button", { name: /^Build the export/ }).waitFor();
  const again = (await text(".export")) ?? "";
  includes(again, "Nothing to analyse", "tracks analysed for an earlier export are not analysed again");
  includes(again, `${under("Hi-Res").filter(holdable).length} already analysed on this computer`, "the plan says how many analyses it reuses");
  await page.keyboard.press("Escape");
  same(await page.locator(".sheet").count(), 0, "Escape closes the export sheet before a build");

  // Stopping a build says so at once and leaves no export behind.
  await page.keyboard.press("Meta+e");
  await page.locator('.export-folder input[data-folder="Paula Temple"]').check();
  check(true, "Cmd+E opens the export sheet");
  await page.getByRole("button", { name: /^Build the export \(/ }).click();
  await page.locator('[data-act="library-export-stop"]').click();
  check((await text(".export")).includes("Stopping after this track.") || (await text(".export")).includes("Stopped."), "a stopped build says it is stopping");
  await page.getByText("Stopped. Nothing was written to the export folder.").waitFor({ timeout: 10000 });
  await at("library-export stopped");
  check(await page.locator('.export-folder input[data-folder="Paula Temple"]').isChecked(), "a stopped build keeps the ticked folders");
  await page.locator('[data-act="library-export-close"]').click();

  await page.locator('[data-act="open-library"]').first().click();
  await page.getByRole("heading", { name: "Ready to copy onto a USB" }).waitFor();
  const listed = (await text(".exports")) ?? "";
  includes(listed, name, "the library lists the finished export");
  includes(listed, `${count(picked.length, "track")} in 2 playlists`, "the listed export carries its counts");
  same(await page.locator(".export-row").count(), 1, "a stopped build adds no export to the list");
  await page.keyboard.press("Escape");

  // A stick: a prepared drive is offered the export, the copy runs through
  // the usual confirmed flow, and verification follows.
  await page.locator('[data-act="demo-insert"][data-id="samsung-64"]').click();
  const samsung = page.locator('.dev[data-dev="demo:samsung-64"]');
  await samsung.waitFor();
  await samsung.locator(".badge.y").first().waitFor({ timeout: 15000 });
  await samsung.click();
  await page.locator('#insp [data-act="device-check"]').click();
  await page.locator(".sheet").getByText("Scanned").waitFor({ timeout: 15000 });
  await page.locator(".sheet").getByRole("button", { name: "Yes, that's it" }).click();
  await page.locator(".sheet").getByRole("button", { name: "See detailed report" }).click();
  await page.locator(".sheet").getByRole("table").waitFor();
  await page.locator(".sheet").getByRole("button", { name: "It's fine, go to the export step" }).click();
  await page.getByText("Put your music on this USB").waitFor({ timeout: 15000 });
  await at("export-handoff with a local export");
  const offer = () => page.locator(`[data-copy-source="export:${name}"]`);
  same(await offer().count(), 1, "the USB is offered the export built on this computer");
  includes(await offer().textContent(), `Copy the export ${name}`, "the offer names the export");
  // The Samsung still holds another library, so the export is never spliced into it.
  check((await offer().getByRole("button", { name: "Erase this USB first" }).count()) === 1, "a USB with another library on it must be erased before the copy");
  await offer().getByRole("button", { name: "Erase this USB first" }).click();
  await page.getByText("You are about to erase:").waitFor();
  await page.getByRole("button", { name: /^Erase and prepare/ }).click();
  await offer().getByRole("button", { name: /^Copy \d+ files/ }).waitFor({ timeout: 20000 });
  await at("export-handoff after the erase");
  check((await offer().getByRole("button", { name: "Erase this USB first" }).count()) === 0, "a prepared USB needs no erase before the copy");
  await offer().getByRole("button", { name: /^Copy \d+ files/ }).click();
  await page.getByText(`Copying the export ${name}`).waitFor();
  await at("copying a local export");
  await page.getByText("Verify before the gig").waitFor({ timeout: 20000 });
  check(true, "the copy of an export hands over to verification");
  await page.keyboard.press("Escape");
  await samsung.click();
  includes(await text("#insp"), "Samsung", "a checked stick opens in the inspector");
  same(await page.locator(".sheet").count(), 0, "a stick that was already identified opens no sheet");
  check((await page.locator('#insp [data-act="device-report"]').count()) === 1, "the stick inspector offers its report");
}

async function checkReport(page, text) {
  const main = await text(".sheet-body");
  includes(await text(".sheet-body .eyebrow"), sandisk.summary.identification.display_name, "report names the drive");
  same((await text(".sheet-body h1")).trim().replace(/\s+/g, " "), `${VERDICT_LABEL[A.overall]}. ${A.headline}`, "report headline is the engine's verdict and headline");
  includes(main, `For ${preset.title.toLowerCase()}`, "report names the chosen preset");

  // The matrix: one row per device the engine assessed, each cell labelled
  // with the layer's status and headline, each row ending in its verdict.
  const rows = page.locator("table.matrix tbody tr");
  same(await rows.count(), A.devices.length, "matrix has a row per assessed device");
  for (const d of A.devices) {
    const row = rows.filter({ has: page.locator(`[data-id="${d.device_id}|library"]`) });
    if (!check((await row.count()) === 1, `matrix has a row for ${d.model}`)) continue;
    includes(await row.locator("td").first().textContent(), d.model, `matrix row names ${d.model}`);
    same((await row.locator("td").last().textContent())?.trim(), VERDICT_LABEL[d.verdict], `matrix verdict for ${d.model}`);
    for (const layer of LAYERS) {
      const l = d.layers.find((x) => x.layer === layer);
      const cell = page.locator(`[data-act="cell"][data-id="${d.device_id}|${layer}"]`);
      if (!check(l && (await cell.count()) === 1, `matrix has a ${layer} cell for ${d.model}`)) continue;
      same(await cell.getAttribute("title"), l.headline, `${d.model} ${layer} cell title`);
      same((await cell.locator(".sr-only").textContent())?.trim(), STATUS_WORD[l.status], `${d.model} ${layer} cell status`);
    }
  }

  // The README's claims for this scenario, as the UI shows them.
  const cdj2000 = A.devices.find((d) => d.device_id === "cdj-2000");
  const cdj2000Row = rows.filter({ has: page.locator('[data-id="cdj-2000|library"]') });
  check(FAILING_VERDICTS.map((v) => VERDICT_LABEL[v]).includes((await cdj2000Row.locator("td").last().textContent())?.trim()), `the CDJ-2000 row shows a failing verdict (engine says ${cdj2000?.verdict})`);
  for (const id of ["cdj-3000x", "xdj-az"]) {
    const d = A.devices.find((x) => x.device_id === id);
    const l = d.layers.find((x) => x.layer === "library");
    const cell = page.locator(`[data-act="cell"][data-id="${id}|library"]`);
    same((await cell.locator(".sr-only").textContent())?.trim(), STATUS_WORD.fail, `${d.model} library cell is marked Not OK`);
    await cell.click();
    const panel = page.getByRole("region", { name: "Explanation" });
    await panel.waitFor();
    same(await cell.getAttribute("aria-pressed"), "true", `${d.model} library cell is pressed after a click`);
    same((await panel.locator("h3").textContent())?.trim(), `${d.model}: ${l.headline}`, `explanation heading for ${d.model}`);
    includes(await panel.textContent(), "OneLibrary", `explanation for ${d.model} mentions OneLibrary`);
    includes(await panel.textContent(), l.detail, `explanation for ${d.model} shows the engine's detail`);
  }

  // What to do: every fix the engine proposed, in its order, and the rebuild
  // names the recommended layout.
  const fixes = page.locator(".fixes .fix");
  same(await fixes.count(), A.fixes.length, "every fix the engine proposed is listed");
  for (const [i, f] of A.fixes.entries()) {
    includes(await fixes.nth(i).locator("b").first().textContent(), f.title, `fix ${i + 1} title`);
    includes(await fixes.nth(i).textContent(), f.detail, `fix ${i + 1} detail`);
    if (f.destructive) includes(await fixes.nth(i).textContent(), "Erases this USB", `fix ${i + 1} warns that it erases the drive`);
  }
  const rebuild = page.getByRole("button", { name: `Rebuild as ${formatLabel(A.recommended_format)}` });
  check((await rebuild.count()) >= 1, `the report offers "Rebuild as ${formatLabel(A.recommended_format)}"`);
  includes(main, "Rebuild as MBR + FAT32", "the report recommends MBR and FAT32");
  await page.getByRole("button", { name: "Why?" }).click();
  includes(await text(".sheet-body"), A.recommended_format.why, "Why? shows the engine's reasoning for the layout");

  // The kit plan: a card per role with its layout and capacity; the Legacy
  // Rescue is its own drive, and this one is the Main.
  includes(main, "Your USB kit", "the report shows the kit plan");
  const roles = page.locator(".roles .role");
  same(await roles.count(), plan.roles.length, "a card per role in the plan");
  for (const [i, r] of plan.roles.entries()) {
    const card = roles.nth(i);
    same((await card.locator("h3").textContent())?.trim(), ROLE_LABEL[r.role], `role card ${i + 1} title`);
    const cardText = await card.textContent();
    includes(cardText, `USB ${i + 1}`, `role card ${i + 1} is numbered`);
    includes(cardText, formatLabel(r.format), `role card ${ROLE_LABEL[r.role]} shows its layout`);
    includes(cardText, `ideally ${gb(r.ideal_capacity[0])}`, `role card ${ROLE_LABEL[r.role]} shows its capacity`);
    for (const p of r.purpose) includes(cardText, p, `role card ${ROLE_LABEL[r.role]} lists its purpose`);
    includes(cardText, r.why, `role card ${ROLE_LABEL[r.role]} explains itself`);
    const thisUsb = (await card.locator(".pill.accent", { hasText: "This USB" }).count()) > 0;
    same(thisUsb, r.assigned_drive === DRIVE, `role card ${ROLE_LABEL[r.role]} marked as this USB only when the plan assigns it`);
    if (r.role === "legacy_rescue") {
      check(!thisUsb, "the Legacy Rescue is a separate drive from the one being prepared");
      check((await card.getByRole("button", { name: `Make this USB the ${ROLE_LABEL[r.role]}` }).count()) === 1, "the Legacy Rescue card offers to use this USB for it");
    }
  }
  for (const n of plan.notes) includes(main, n, "the plan's notes are shown");
}

try {
  checkScenarioData();
  console.log("Walking the UI:");
  await walk();
} catch (e) {
  failures.push(`[${screen}] walkthrough stopped: ${e.message.split("\n")[0]}`);
  // The screen it stopped on, for a look at what went wrong.
  const open = browser.contexts().flatMap((c) => c.pages())[0];
  if (open) {
    mkdirSync("test-results", { recursive: true });
    await open.screenshot({ path: "test-results/check-ui-failed.png", fullPage: true }).catch(() => {});
    failures.push(`[${screen}] screen saved to test-results/check-ui-failed.png`);
  }
} finally {
  await browser.close();
  server.httpServer.close();
}
if (browserErrors.length) failures.push(...browserErrors.map((e) => `[browser] ${e}`));
if (failures.length) {
  console.error(`\n${failures.length} of ${checks} checks failed:\n` + failures.map((f) => `  - ${f}`).join("\n"));
  process.exit(1);
}
console.log(`\n${checks} checks passed on ${screens.length} screens: ${screens.join(", ")}`);
