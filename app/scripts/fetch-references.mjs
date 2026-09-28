// Downloads every document the compatibility rules cite, plus Reddit threads
// on the claims most worth a second opinion, into research/ at the repo root
// (gitignored). A rules review then reads full pages instead of search
// excerpts. See docs/RULES-REVIEW.md.
//
// Needs open internet and a Playwright browser:
//   cd app && npm ci && npx playwright install chromium
//   node scripts/fetch-references.mjs
// LIMIT=3 fetches only the first three references and one Reddit query.
import { chromium, request } from "playwright";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const rules = JSON.parse(readFileSync(join(root, "crates/boothready-core/data/ruleset.json"), "utf8"));
const out = join(root, "research");
mkdirSync(join(out, "refs"), { recursive: true });
mkdirSync(join(out, "reddit"), { recursive: true });

const limit = Number(process.env.LIMIT) || Infinity;
const UA =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_6) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0 Safari/537.36";
const today = new Date().toISOString().slice(0, 10);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// Searches aimed at the claims that decide whether a stick works and that
// vendor pages settle least well.
const REDDIT_QUERIES = [
  "CDJ-3000 exFAT",
  "CDJ-3000 USB not recognized mac",
  "CDJ-2000NXS2 GUID partition",
  "NXS2 format disk usb mac",
  "CDJ-2000NXS2 exFAT",
  "CDJ-900 usb format",
  "XDJ-XZ exFAT",
  "XDJ-1000MK2 FLAC",
  "CDJ-3000X Device Library OneLibrary",
  "XDJ-AZ OneLibrary playlists missing",
  "Opus Quad 96kHz",
  "Omnis Duo usb format",
  "CDJ-1500X OneLibrary",
  "XDJ-AN usb",
  "CDJ-3000 firmware 3.30 playlists",
  "rekordbox usb case sensitive",
  "SC6000 HFS+",
  "Prime 4 NTFS",
  "Engine DJ OneLibrary",
  "Mixxx rekordbox usb",
];

const index = { fetched: today, refs: {}, reddit: {} };
const browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH || undefined });
const page = await browser.newPage({ userAgent: UA });
const api = await request.newContext({ userAgent: UA });

try {
  for (const [id, ref] of Object.entries(rules.references).slice(0, limit)) {
    try {
      if (new URL(ref.url).pathname.toLowerCase().endsWith(".pdf")) {
        const r = await api.get(ref.url, { timeout: 60000 });
        if (!r.ok()) throw new Error(`HTTP ${r.status()}`);
        writeFileSync(join(out, "refs", `${id}.pdf`), await r.body());
        index.refs[id] = { url: ref.url, file: `refs/${id}.pdf` };
      } else {
        const resp = await page.goto(ref.url, { waitUntil: "domcontentloaded", timeout: 45000 });
        // Help centers render their articles after the first paint.
        await page.waitForTimeout(2000);
        const text = await page.evaluate(() => document.body.innerText);
        writeFileSync(
          join(out, "refs", `${id}.txt`),
          `${ref.title}\n${ref.url}\nfetched ${today}, HTTP ${resp?.status()}\n\n${text}\n`,
        );
        index.refs[id] = { url: ref.url, file: `refs/${id}.txt`, status: resp?.status(), chars: text.length };
      }
      console.log(`ok    ${id}`);
    } catch (e) {
      index.refs[id] = { url: ref.url, error: firstLine(e) };
      console.log(`FAIL  ${id}: ${firstLine(e)}`);
    }
  }

  for (const q of REDDIT_QUERIES.slice(0, limit === Infinity ? undefined : 1)) {
    const slug = q.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
    try {
      const search = await getJson(
        `https://old.reddit.com/search.json?q=${encodeURIComponent(q)}&sort=relevance&t=all&limit=15`,
      );
      const posts = search.data.children
        .map((c) => c.data)
        .filter((p) => p.num_comments > 0)
        .slice(0, 8);
      const threads = [];
      for (const p of posts) {
        await sleep(1200);
        const thread = await getJson(`https://old.reddit.com${p.permalink}.json?limit=40&sort=top`).catch(() => null);
        threads.push({
          title: p.title,
          subreddit: p.subreddit,
          url: `https://www.reddit.com${p.permalink}`,
          created: new Date(p.created_utc * 1000).toISOString().slice(0, 10),
          score: p.score,
          text: p.selftext,
          comments: thread ? flatten(thread[1].data.children) : [],
        });
      }
      writeFileSync(join(out, "reddit", `${slug}.json`), JSON.stringify({ query: q, fetched: today, threads }, null, 2));
      index.reddit[q] = { file: `reddit/${slug}.json`, threads: threads.length };
      console.log(`ok    reddit "${q}" (${threads.length} threads)`);
    } catch (e) {
      index.reddit[q] = { error: firstLine(e) };
      console.log(`FAIL  reddit "${q}": ${firstLine(e)}`);
    }
    await sleep(1500);
  }
} finally {
  writeFileSync(join(out, "index.json"), JSON.stringify(index, null, 2));
  await browser.close();
  await api.dispose();
}

const failed = [...Object.values(index.refs), ...Object.values(index.reddit)].filter((r) => r.error).length;
console.log(`\nSaved to research/. ${failed} of ${Object.keys(index.refs).length + Object.keys(index.reddit).length} failed.`);

/** Reddit's JSON, loaded in the real browser because Reddit refuses most other clients. */
async function getJson(url) {
  const resp = await page.goto(url, { waitUntil: "domcontentloaded", timeout: 30000 });
  if (!resp?.ok()) throw new Error(`HTTP ${resp?.status()} for ${url}`);
  return JSON.parse(await page.evaluate(() => document.body.innerText));
}

function firstLine(e) {
  return String(e?.message ?? e).split("\n")[0];
}

/** Comments and replies two levels deep, highest-voted first as Reddit sorts them. */
function flatten(children, depth = 0, acc = []) {
  for (const c of children) {
    if (c.kind !== "t1") continue;
    acc.push({ score: c.data.score, depth, body: c.data.body });
    if (c.data.replies && depth < 2) flatten(c.data.replies.data.children, depth + 1, acc);
  }
  return acc;
}
