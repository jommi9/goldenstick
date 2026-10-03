// Small cross-platform command runner for BoothReady development.
// Run `npm run devtools -- help` from app/ or use one of the package scripts.
import { existsSync, mkdirSync, readdirSync, rmSync, statSync } from "node:fs";
import { join, dirname, delimiter } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const appDir = join(dirname(fileURLToPath(import.meta.url)), "..");
const rootDir = join(appDir, "..");
const npm = process.platform === "win32" ? "npm.cmd" : "npm";
const npx = process.platform === "win32" ? "npx.cmd" : "npx";
const home = process.env.HOME ?? process.env.USERPROFILE ?? "";

function resolveTool(name) {
  if (has(name)) return name;
  const candidates = [
    join(home, ".cargo", "bin", name),
    join("/opt/homebrew/opt/rustup/bin", name),
    join("/usr/local/opt/rustup/bin", name),
  ];
  return candidates.find((candidate) => has(candidate)) ?? name;
}

const cargo = resolveTool("cargo");
const rustc = resolveTool("rustc");
const toolPath = [dirname(cargo), dirname(rustc), process.env.PATH].filter(Boolean).join(delimiter);

const command = process.argv[2] ?? "help";

function run(program, args, cwd = rootDir, extraEnv = {}) {
  console.log(`\n$ ${program} ${args.join(" ")}`);
  const result = spawnSync(program, args, {
    cwd,
    stdio: "inherit",
    env: { ...process.env, PATH: toolPath, ...extraEnv },
  });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}

function has(program, args = ["--version"]) {
  const result = spawnSync(program, args, { stdio: "ignore" });
  return !result.error && result.status === 0;
}

function help() {
  console.log(`BoothReady developer tools

Usage: npm run devtools -- <command>

Commands:
  doctor       Check the local toolchain and Playwright browser
  mock-data    Regenerate app/src/mock-data.json from the Rust engine
  check        Run formatting, lint, Rust tests and app type/build checks
  test         Run the Rust virtual matrix and browser UI test lab
  test-ui      Run browser UI scenarios against the mock backend
  screenshots  Run the Playwright walkthrough and save app/screenshots/
  desktop      Start the Tauri desktop app in demo mode
  clean        Remove generated app output and screenshot folders
  help         Show this message

The in-app Test lab panel is available from the top bar. Dev tools is available in demo mode and in the browser mock.
`);
}

function doctor() {
  const browserCaches = playwrightCacheCandidates();
  const browserCache = browserCaches.find((candidate) => existsSync(candidate) && readdirSync(candidate).some((name) => name.startsWith("chromium"))) ?? browserCaches[0];
  const playwrightReady = Boolean(browserCache) && existsSync(browserCache) && readdirSync(browserCache).some((name) => name.startsWith("chromium"));
  const checks = [
    ["node", has("node"), process.version],
    ["npm", has(npm), "available"],
    ["cargo", has(cargo), cargo],
    ["rustc", has(rustc), rustc],
    ["Playwright Chromium", playwrightReady, browserCache],
  ];
  let failed = false;
  for (const [name, ok, detail] of checks) {
    console.log(`${ok ? "ok   " : "FAIL "}${name}: ${detail}`);
    if (!ok) failed = true;
  }
  if (failed) process.exit(1);
}

function playwrightCacheCandidates() {
  const configured = process.env.PLAYWRIGHT_BROWSERS_PATH;
  if (configured && configured !== "0") return [configured];
  if (configured === "0") return [join(appDir, "node_modules", "playwright-core", ".local-browsers")];
  if (process.platform === "win32") return [join(process.env.LOCALAPPDATA ?? join(home, "AppData", "Local"), "ms-playwright")];
  if (process.platform === "darwin") return [join(home, "Library", "Caches", "ms-playwright")];
  return [join(process.env.XDG_CACHE_HOME ?? join(home, ".cache"), "ms-playwright")];
}

switch (command) {
  case "help":
    help();
    break;
  case "doctor":
    doctor();
    break;
  case "mock-data":
    run(cargo, ["run", "-q", "-p", "boothready-app", "--example", "mock_snapshot"]);
    console.log(`Generated ${join(appDir, "src/mock-data.json")}`);
    break;
  case "check":
    run(cargo, ["fmt", "--all"]);
    run(cargo, ["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]);
    run(cargo, ["test"]);
    run(cargo, ["test", "-p", "boothready-app"]);
    run(npm, ["run", "typecheck"], appDir);
    run(npm, ["run", "build"], appDir);
    run(cargo, ["run", "-q", "-p", "boothready-cli", "--", "test", "virtual"]);
    break;
  case "test":
    run(cargo, ["run", "-q", "-p", "boothready-cli", "--", "test", "virtual"]);
    run(npm, ["run", "build"], appDir);
    run(npm, ["run", "test-lab"], appDir);
    break;
  case "test-ui":
    run(npm, ["run", "build"], appDir);
    run(npm, ["run", "test-lab"], appDir);
    break;
  case "screenshots":
    run(npm, ["run", "screenshots"], appDir);
    break;
  case "desktop":
    run(npx, ["tauri", "dev"], appDir, { BOOTHREADY_DEMO: "1" });
    break;
  case "clean":
    for (const relative of ["dist", "screenshots", "test-results"]) {
      const target = join(appDir, relative);
      if (existsSync(target)) {
        const before = statSync(target).isDirectory() ? "folder" : "file";
        rmSync(target, { recursive: true, force: true });
        console.log(`Removed ${before} ${target}`);
      }
    }
    mkdirSync(join(appDir, "screenshots"), { recursive: true });
    console.log("Created an empty screenshots folder.");
    break;
  default:
    console.error(`Unknown developer command: ${command}`);
    help();
    process.exit(2);
}
