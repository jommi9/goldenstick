// Focused browser scenarios for the mock backend. The Rust test lab covers
// engine and helper behaviour; this runner covers the app's state transitions
// and reproducible fixture URLs.
import { chromium } from "playwright";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { preview } from "vite";

const out = process.env.TEST_LAB_DIR ?? "test-results";
mkdirSync(out, { recursive: true });
const server = await preview({ preview: { port: 4175, strictPort: true, host: "127.0.0.1" }, logLevel: "error" });
const browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH || undefined });

const cases = [
  {
    id: "clean-home",
    title: "clean home screen",
    url: "/",
    run: async (page) => {
      await page.getByText("Plug in a USB").waitFor();
      await page.getByRole("button", { name: "Dev tools" }).waitFor();
      return "home and development controls loaded";
    },
  },
  {
    id: "fixture-url",
    title: "reproduce an exported verified fixture",
    url: "/?insert=sandisk-128&stage=sandisk-128:exported&verified=sandisk-128",
    run: async (page) => {
      await page.getByRole("button", { name: "Dev tools" }).click();
      await page.locator('[data-act="open-device"][data-id="demo:sandisk-128"]').click();
      await page.getByText("Scanned").waitFor({ timeout: 15000 });
      await page.getByRole("button", { name: "Yes, that's it" }).click();
      await page.getByText("Verified by BoothReady and unchanged since.").waitFor();
      return "URL fixture opened with exported and verified state";
    },
  },
  {
    id: "reset-cancels-work",
    title: "reset cancels stale async UI state",
    url: "/?insert=sandisk-128",
    run: async (page) => {
      await page.getByRole("button", { name: "Dev tools" }).click();
      await page.locator('[data-act="open-device"][data-id="demo:sandisk-128"]').click();
      await page.waitForTimeout(120);
      await page.getByRole("button", { name: "Dev tools" }).click();
      await page.getByRole("button", { name: "Reset demo state" }).click();
      await page.getByText("No devices are currently connected.").waitFor();
      await page.waitForTimeout(1200);
      if ((await page.getByText("No devices are currently connected.").count()) !== 1) {
        throw new Error("a stale scan repopulated the reset state");
      }
      return "reset stayed empty after the scan promise completed";
    },
  },
  {
    id: "prepared-fixture",
    title: "open a prepared FAT32 fixture",
    url: "/?insert=kingston-32&stage=kingston-32:prepared",
    run: async (page) => {
      await page.getByRole("button", { name: "Dev tools" }).click();
      await page.locator('[data-act="open-device"][data-id="demo:kingston-32"]').click();
      await page.getByText("Scanned").waitFor({ timeout: 15000 });
      await page.getByText("FAT32", { exact: true }).waitFor();
      return "prepared fixture reports FAT32";
    },
  },
  {
    id: "test-lab-gui",
    title: "test lab GUI runs a virtual matrix",
    url: "/",
    run: async (page) => {
      await page.getByRole("button", { name: "Test lab" }).click();
      await page.getByRole("heading", { name: "Test lab" }).waitFor();
      await page.getByRole("button", { name: "Run virtual tests" }).click();
      await page.getByText("All selected scenarios passed").waitFor({ timeout: 10000 });
      await page.locator(".test-result").first().waitFor();
      return "virtual matrix is visible in the app test lab";
    },
  },
];

async function runCase(testCase) {
  const started = Date.now();
  const page = await browser.newPage({ viewport: { width: 1180, height: 860 }, colorScheme: "dark" });
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  try {
    await page.goto(`http://127.0.0.1:4175${testCase.url}`);
    const detail = await testCase.run(page);
    if (errors.length) throw new Error(`browser errors: ${errors.join("; ")}`);
    return { id: testCase.id, title: testCase.title, passed: true, duration_ms: Date.now() - started, detail };
  } catch (error) {
    const screenshot = join(out, `failed-${testCase.id}.png`);
    await page.screenshot({ path: screenshot, fullPage: true }).catch(() => {});
    return {
      id: testCase.id,
      title: testCase.title,
      passed: false,
      duration_ms: Date.now() - started,
      error: String(error),
      screenshot,
    };
  } finally {
    await page.close();
  }
}

try {
  const results = [];
  for (const testCase of cases) results.push(await runCase(testCase));
  const report = {
    kind: "browser_mock",
    passed: results.every((result) => result.passed),
    base_url: "http://127.0.0.1:4175/",
    results,
  };
  const reportPath = join(out, "boothready-ui-test-lab.json");
  writeFileSync(reportPath, `${JSON.stringify(report, null, 2)}\n`);
  for (const result of results) {
    console.log(`${result.passed ? "PASS" : "FAIL"} ${result.id}${result.detail ? `: ${result.detail}` : ""}`);
    if (result.error) console.log(`     ${result.error}`);
  }
  console.log(`Report: ${reportPath}`);
  if (!report.passed) process.exitCode = 1;
} finally {
  await browser.close();
  server.httpServer.close();
}
