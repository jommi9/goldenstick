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

async function walk() {
  const page = await browser.newPage({ viewport: { width: 1180, height: 860 }, colorScheme: "dark" });
  page.on("pageerror", (e) => browserErrors.push(e.message));
  page.on("console", (m) => m.type() === "error" && browserErrors.push(m.text()));
  const text = async (selector) => (await page.locator(selector).first().textContent()) ?? "";
  const click = (name) => page.getByRole("button", { name, exact: false }).first().click();

  // Every screen is checked for the notice the README promises, the ruleset
  // version the engine reports, and for rendering slips.
  const at = async (name) => {
    screen = name;
    screens.push(name);
    await page.waitForTimeout(150);
    const foot = await text("footer.foot");
    includes(foot, `Compatibility rules v${data.rules.version}`, "footer names the ruleset version");
    includes(foot, RULES_STATUS[data.rules.review_status], "footer carries the ruleset's review note");
    includes(foot, "not yet tested on hardware", "footer says the rules are not yet tested on hardware");
    const main = await text("main#main");
    check(!/\bundefined\b|\[object Object\]|\bNaN\b/.test(main), "no undefined, NaN or [object Object] rendered");
    console.log(`  ${name}`);
  };

  await page.goto(`http://127.0.0.1:${port}/`);
  await page.getByText("Plug in a USB").waitFor();
  await at("home");
  includes(await text("main#main"), "BoothReady does two things.", "home says what the app is for");
  includes(await text("main#main"), "Connect a USB drive and it appears here.", "home says where a connected USB will show up");
  for (const s of data.demo_sticks) {
    check(await page.locator(`[data-act="demo-insert"][data-id="${s.name}"]`).count(), `the demo panel offers ${s.title}`);
    includes(await text("main#main"), s.description, `the demo panel describes ${s.title}`);
  }

  // The 128 GB SanDisk: GPT + exFAT with only a Device Library.
  await page.locator(`[data-act="demo-insert"][data-id="${demoName(DRIVE)}"]`).click();
  await page.getByText("We think this is").waitFor();
  await page.getByText("Scanned").waitFor({ timeout: 10000 });
  await at("identify");
  const idText = await text("main#main");
  includes(idText, sandisk.summary.identification.display_name, "identification names the drive");
  includes(idText, sandisk.summary.scheme, "identification shows the partition scheme the engine read");
  includes(idText, sandisk.summary.filesystem, "identification shows the filesystem the engine read");
  await click("Yes, that's it");
  await page.getByText("Prepare this USB").waitFor();
  await at("assessment");
  includes(await text("main#main"), sandisk.summary.headline, "assessment shows the engine's headline");

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
  await page.getByRole("table").waitFor();
  await at("report");
  await checkReport(page, text);

  await page.getByRole("button", { name: `Rebuild as ${formatLabel(A.recommended_format)}` }).click();
  await page.getByText("You are about to erase:").waitFor();
  await at("confirm-erase");
  const c = sandisk.confirmation;
  const confirmText = await text("main#main");
  includes(confirmText, c.display_name, "confirmation names the drive about to be erased");
  includes(confirmText, gb(c.size_bytes), "confirmation shows the drive's capacity");
  includes(confirmText, `${c.files.toLocaleString("en-US")} file`, "confirmation counts the files that will be deleted");
  includes(confirmText, `MBR + ${fsLabel(A.recommended_format.filesystem)}`, "confirmation names the layout it will build");
  check(await page.getByRole("button", { name: /^Erase and prepare/ }).isEnabled(), "the erase button is enabled for an eligible drive");

  await page.getByRole("button", { name: /^Erase and prepare/ }).click();
  await page.getByText("Preparing your USB").waitFor();
  await at("preparing");
  await page.getByText("Now export from rekordbox").waitFor({ timeout: 15000 });
  await at("export-handoff");
  await click("Simulate the rekordbox export");
  await page.getByText("Verify before the gig").waitFor({ timeout: 15000 });
  await at("verify");
  await click("Start verification");
  await page.getByText(/^Verified \d+ files?/).waitFor({ timeout: 10000 });
  await at("verified");
  const exported = data.drives[DRIVE].exported.summary;
  includes(await text("main#main"), `Verified ${Math.max(exported.tracks_scanned, 1)} file`, "verification reports the engine's file count");

  await click("Continue");
  await page.getByText("Almost there").waitFor();
  await at("kit-progress");
  const progress = await text("main#main");
  includes(progress, `1 of ${plan.roles.length} USBs ready`, "kit progress counts the plan's roles");
  const next = plan.roles[1];
  includes(progress, `Next: ${ROLE_LABEL[next.role]}`, "kit progress names the next role in the plan");
  includes(progress, gb(next.ideal_capacity[0]), "kit progress asks for the next role's capacity");
  includes(progress, ROLE_LABEL[plan.roles[0].role], "the verified drive is listed under its role");
  for (const d of A.devices) includes(progress, d.model, `the verified drive lists ${d.model}`);

  await page.getByRole("button", { name: `Prepare the ${ROLE_LABEL[next.role]} USB` }).click();
  await page.getByText(`Next: ${ROLE_LABEL[next.role]}`).waitFor();
  await at("next-usb");
  includes(await text("main#main"), `Preparing ${plan.roles.length} USBs`, "the next-USB screen counts the plan's roles");
  await checkLibrary(page, text, at, click);
  await page.close();
}

// The music library: every source and every track shows what the engine's
// scan and its per-player verdicts say, for the preset selected above.
async function checkLibrary(page, text, at, click) {
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
  await at("library");
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
  includes(await text("main#main"), `${count(cloudTotal, "track")} are still in the cloud`, "the library says how many tracks are still online");

  for (const id of ["demo-computer", "demo-cloud"]) {
    const s = byId[id];
    const tracks = lib.tracks[id];
    await page.locator('[data-act="open-library"]').first().click();
    await page.locator(`[data-act="library-open"][data-id="${id}"]`).click();
    await page.getByRole("table").waitFor();
    await at(`library-source ${id}`);
    const rows = page.locator(".tracks tbody tr");
    same(await rows.count(), tracks.length, `${s.name} lists every scanned track`);
    for (const [i, t] of tracks.entries()) {
      const row = (await rows.nth(i).textContent()) ?? "";
      includes(row, t.title ?? t.name, `row ${i + 1} of ${s.name} is ${t.title ?? t.name}`);
      if (t.artist) includes(row, t.artist, `${t.name} shows its artist tag`);
      if (t.camelot) includes(row, t.camelot, `${t.name} shows its key on the Camelot wheel`);
      if (t.bpm !== null) includes(row, String(Math.round(t.bpm * 100) / 100), `${t.name} shows its tempo`);
      includes(row, STATE_LABEL[t.state], `${t.name} shows the engine's state`);
      for (const issue of t.issues.slice(0, 3)) {
        includes(row, issue.model, `${t.name} names the player that rejects it`);
        includes(row, issue.reason, `${t.name} gives the engine's reason for ${issue.model}`);
      }
      if (t.note) includes(row, t.note, `${t.name} shows why it is unreadable`);
      if (t.state === "cloud_only") includes(row, "not checked", `${t.name} is shown as not checked`);
    }
    for (const [state, n] of Object.entries(s.states)) {
      if (!n) continue;
      await page.locator(`[data-act="library-filter"][data-id="${state}"]`).click();
      await page.waitForTimeout(120);
      same(await page.locator(".tracks tbody tr").count(), n, `the ${state} filter on ${s.name} shows its ${n} tracks`);
    }
  }

  // The browser: a folder in the tree narrows the list to that folder, and
  // the collection holds every source's tracks.
  await page.locator('[data-act="open-library"]').first().click();
  await page.locator('[data-act="library-open"][data-id="demo-computer"]').click();
  await page.getByRole("table").waitFor();
  check((await page.locator(".rb-tree").count()) === 1, "the source opens in the browser with its tree");
  const hiRes = lib.tracks["demo-computer"].filter((t) => t.folder === "Hi-Res");
  await page.locator('[data-act="library-folder"][data-id="demo-computer|Hi-Res"]').click();
  await page.waitForTimeout(150);
  await at("library-folder Hi-Res");
  same(await page.locator(".tracks tbody tr").count(), hiRes.length, "a folder in the tree lists only its tracks");
  includes(await text("main#main h1"), "Hi-Res", "the selected folder names the view");
  await page.locator('[data-act="library-open"][data-id="*"]').click();
  await page.getByRole("table").waitFor();
  await page.waitForTimeout(150);
  await at("library-collection-all");
  const everything = lib.sources.reduce((n, s) => n + lib.tracks[s.id].length, 0);
  includes(await text(".tracks-foot"), `of ${everything.toLocaleString("en-US")}`, "the collection holds every source's tracks");

  // Tags: the engine read them, search finds an artist, and a heading
  // orders the list with untagged tracks last.
  check(lib.tracks["demo-computer"].filter((t) => t.state !== "unreadable").some((t) => t.title === null), "one demo track is left untagged");
  check(lib.tracks["demo-computer"].filter((t) => t.artist).length >= 6, "the demo tracks carry artist tags");
  await page.locator("#library-search").fill("klock");
  await page.waitForTimeout(400);
  const klock = lib.tracks["demo-computer"].filter((t) => t.artist === "Ben Klock").length;
  same(await page.locator(".tracks tbody tr").count(), klock, "searching an artist finds that artist's tracks");
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

  // Playing: a row's number is its play key, the bar along the bottom
  // follows the track across screens, and unreadable files can't be started.
  for (const s of lib.sources) check(lib.tracks[s.id].every((t) => t.source === s.id), `every track of ${s.name} names its source`);
  const unreadable = lib.tracks["demo-computer"].filter((t) => t.state === "unreadable").length;
  same(await page.locator("tbody tr.unreadable .rb-play").count(), 0, "an unreadable track has no play key");
  check(unreadable === 0 || (await page.locator("tbody tr.unreadable").count()) > 0, "an unreadable track is listed");
  const second = page.locator(".rb-play").nth(1);
  const secondName = (await second.getAttribute("aria-label"))?.replace(/^Play /, "");
  await second.click();
  await page.locator(".player").waitFor();
  await page.waitForFunction(() => document.querySelector(".player-key.main")?.getAttribute("aria-label") === "Pause");
  await at("library-player");
  check((await text(".player-name")).trim().startsWith(secondName), "the player shows the track that was clicked");
  same((await text("tr.playing td b")).trim(), secondName, "the playing row is marked");
  await page.waitForFunction(() => document.getElementById("player-elapsed")?.textContent !== "0:00", null, { timeout: 4000 });
  check(true, "the clock runs while the track plays");
  await page.locator('[data-act="library-next"]').click();
  await page.waitForFunction((name) => document.querySelector("tr.playing td b")?.textContent?.trim() !== name, secondName);
  check(!(await text(".player-name")).trim().startsWith(secondName), "next moves to the following track");
  await page.keyboard.press("Escape");
  await page.locator("main#main h1").click();
  await page.keyboard.press(" ");
  await page.waitForFunction(() => document.querySelector(".player-key.main")?.getAttribute("aria-label") === "Play");
  check(true, "the space bar pauses");
  await page.locator('.tab[data-act="home"]').click();
  check((await page.locator(".player").count()) === 1, "the player stays on other screens");
  await page.locator('[data-act="library-stop-play"]').click();
  same(await page.locator(".player").count(), 0, "stop closes the player");
  await page.locator('[data-act="open-library"]').first().click();
  await page.locator('[data-act="library-open"][data-id="*"]').click();
  await page.getByRole("table").waitFor();
  await page.locator('[data-act="library-folder"][data-id="demo-cloud|"]').click();
  await page.getByRole("table").waitFor();

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
  await page.getByRole("table").waitFor();
  await at("library-collection");
  const rows = page.locator(".tracks tbody tr");
  same(await rows.count(), made.tracks.length, "the collection lists every collected track");
  for (const [i, t] of made.tracks.entries()) {
    const row = (await rows.nth(i).textContent()) ?? "";
    includes(row, t.name, `collected row ${i + 1} is ${t.name}`);
    includes(row, STATE_LABEL[t.state], `${t.name} shows the state the engine found after collecting`);
    includes(row, t.detail, `${t.name} shows the format the engine read`);
    check(!row.includes("not checked") && !row.includes(STATE_LABEL.cloud_only), `${t.name} is no longer shown as online`);
    for (const issue of t.issues.slice(0, 3)) includes(row, issue.reason, `${t.name} gives the engine's reason for ${issue.model}`);
  }
  check((await page.locator('[data-act="library-collect-open"]').count()) === 0, "a collection doesn't offer to be collected again");
  const next = (await text(".nextstep")) ?? "";
  includes(next, "Export this collection to your USB", "a collection says what to do next");
  check((await page.locator('.nextstep [data-act="library-export-open"]').count()) === 1, "the next step offers to build the export here");
  check((await page.locator('.nextstep [data-act="open-rekordbox"]').count()) === 1, "the next step still offers rekordbox");
  await page.locator('[data-act="open-library"]').first().click();
  const card = (await page.locator(".source").last().textContent()) ?? "";
  includes(card, "Local collection", "the library marks the new source as a local collection");
  includes(card, `A verified copy of tracks from ${cloud.name}.`, "the collection card names its origin");
  await checkExport(page, text, at, click);
}

// Export to USB: ticked folders become playlists, the plan counts the
// tracks and estimates the analysis, the build reports progress, and the
// finished export is offered to a prepared USB by the copy flow.
async function checkExport(page, text, at, click) {
  const lib = data.library[PRESET];
  const count = (n, one) => `${n.toLocaleString("en-US")} ${n === 1 ? one : `${one}s`}`;
  const computer = lib.sources.find((s) => s.id === "demo-computer");
  const tracks = lib.tracks["demo-computer"];
  const holdable = (t) => t.state !== "unreadable" && /\.(mp3|m4a|flac|wav|aif|aiff)$/i.test(t.name);
  const under = (folder) => tracks.filter((t) => t.folder === folder || t.folder.startsWith(`${folder}/`));
  const picked = [...under("Ben Klock"), ...under("Hi-Res")].filter(holdable);
  const leftOut = [...under("Ben Klock"), ...under("Hi-Res")].length - picked.length;

  await page.locator('[data-act="library-open"][data-id="demo-computer"]').click();
  await page.getByRole("table").waitFor();
  await page.locator('[data-act="library-export-open"]').click();
  await page.getByRole("heading", { name: `Build a rekordbox Device Library from ${computer.name}` }).waitFor();
  await at("library-export open");
  const folders = page.locator(".export-folder");
  const folderSet = new Set(tracks.map((t) => t.folder.split("/")).flatMap((parts) => parts.map((_, i) => parts.slice(0, i + 1).join("/"))));
  same(await folders.count(), folderSet.size, "every folder of the source can be ticked");
  check(await page.getByRole("button", { name: /^Build the export/ }).isDisabled(), "nothing can be built before a folder is ticked");
  await page.locator('.export-folder input[data-folder="Ben Klock"]').check();
  await page.locator('.export-folder input[data-folder="Hi-Res"]').check();
  await page.getByRole("button", { name: `Build the export (${count(picked.length, "track")})` }).waitFor();
  await at("library-export plan");
  const plan = (await text(".export")) ?? "";
  includes(plan, `Ben Klock (${under("Ben Klock").filter(holdable).length}), Hi-Res (${under("Hi-Res").filter(holdable).length})`, "the plan names a playlist per ticked folder with its track count");
  includes(plan, `${count(picked.length, "track")} to analyse`, "the plan counts the tracks to analyse");
  includes(plan, picked.length * 6 < 45 ? "under a minute" : `about ${Math.round((picked.length * 6) / 60)} min`, "the plan estimates the analysis at six seconds a track");
  includes(plan, "/exports/", "the plan shows the export's own folder on this computer");
  check(leftOut === 0 || plan.includes(`${count(leftOut, "file")} left out`), "the plan counts the files left out");
  const name = (await page.locator("#library-export-name").inputValue()) ?? "";
  includes(name, computer.name, "the export is named after the source");

  await page.getByRole("button", { name: /^Build the export/ }).click();
  await page.getByText(/^(Analysing|Copying|Writing the database|Finishing)/).first().waitFor();
  await at("library-export building");
  includes(await text(".export"), `of ${count(picked.length, "track")}`, "the build counts the tracks as it goes");
  check((await page.locator('[data-act="library-export-stop"]').count()) === 1, "the build can be stopped");
  await page.getByText(`Export built: ${count(picked.length, "track")} in 2 playlists`).waitFor({ timeout: 20000 });
  await at("library-export done");
  const done = (await text(".export")) ?? "";
  includes(done, "Nothing has been written to a USB yet.", "the finished export says no USB was written");
  includes(done, `/exports/${name}`, "the finished export names its folder");
  check((await page.locator('.export [data-act="home"]').count()) === 1, "the finished export points at the USB tab");

  // A second plan for the same folders has nothing left to analyse.
  await page.locator('[data-act="library-export-close"]').click();
  await page.locator('[data-act="library-export-open"]').click();
  await page.locator('.export-folder input[data-folder="Hi-Res"]').check();
  await page.getByRole("button", { name: /^Build the export/ }).waitFor();
  const again = (await text(".export")) ?? "";
  includes(again, "Nothing to analyse", "tracks analysed for an earlier export are not analysed again");
  includes(again, `${under("Hi-Res").filter(holdable).length} already analysed for an earlier export`, "the plan says how many analyses it reuses");
  await page.locator('[data-act="library-export-close"]').click();

  await page.locator('[data-act="open-library"]').first().click();
  await page.getByRole("heading", { name: "Ready to copy onto a USB" }).waitFor();
  const listed = (await text(".exports")) ?? "";
  includes(listed, name, "the library lists the finished export");
  includes(listed, `${count(picked.length, "track")} in 2 playlists`, "the listed export carries its counts");

  // The USB tab: a prepared drive is offered the export, the copy runs
  // through the usual confirmed flow, and verification follows.
  await page.locator('.tab[data-act="home"]').click();
  await page.locator('[data-act="demo-insert"][data-id="samsung-64"]').click();
  await page.getByText("Scanned").waitFor({ timeout: 15000 });
  await click("Yes, that's it");
  await click("See detailed report");
  await page.getByRole("table").waitFor();
  await click("It's fine, go to the export step");
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
}

async function checkReport(page, text) {
  const main = await text("main#main");
  includes(await text("main .eyebrow"), sandisk.summary.identification.display_name, "report names the drive");
  same((await text("main h1")).trim().replace(/\s+/g, " "), `${VERDICT_LABEL[A.overall]}. ${A.headline}`, "report headline is the engine's verdict and headline");
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
  includes(await text("main#main"), A.recommended_format.why, "Why? shows the engine's reasoning for the layout");

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
