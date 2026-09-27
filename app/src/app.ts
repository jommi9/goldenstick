// The BoothReady UI: one state object, one render function, and actions
// wired through data-act attributes. Screens follow PRD §11-§43 and the
// prototype flow in §99.

import type { Api } from "./api";
import { hardwareArt, statusIcon, usbArt, VERDICT_LABEL, verdictStatus } from "./art";
import type {
  AppInfo,
  Candidate,
  ConfirmationDetails,
  CopyReport,
  CopySource,
  DemoStickInfo,
  DeviceAssessment,
  DeviceCard,
  DriveAssessment,
  DriveSummary,
  Filesystem,
  Fix,
  GigProfile,
  HardwareCard,
  KitPlan,
  KnownMedia,
  Layer,
  LayerResult,
  Preset,
  Role,
  RolePlan,
  VerifyProgress,
  VerifyReport,
} from "./types";
import {
  EVIDENCE_LABEL,
  FS_LABEL,
  gb,
  html,
  LIB_LABEL,
  num,
  plural,
  primaryVolume,
  raw,
  Raw,
  ROLE_LABEL,
  when,
} from "./ui";

type View =
  | { v: "home" }
  | { v: "buying" }
  | { v: "identify"; id: string }
  | { v: "looks-different"; id: string }
  | { v: "assessment"; id: string }
  | { v: "targets"; id: string }
  | { v: "hardware"; id: string }
  | { v: "report"; id: string }
  | { v: "confirm"; id: string; fs: Filesystem }
  | { v: "preparing"; id: string }
  | { v: "export"; id: string }
  | { v: "copying"; id: string; from: string }
  | { v: "verify"; id: string }
  | { v: "ready" }
  | { v: "kit-next"; role: Role };

interface Done {
  role: Role | null;
  deviceId: string;
  name: string;
  image: string;
  color: string;
  targets: string[];
  assessment: DriveAssessment | null;
  report: VerifyReport;
  tracks: number;
  playlists: number;
}

const S = {
  api: null as unknown as Api,
  info: null as AppInfo | null,
  view: { v: "home" } as View,
  devices: [] as DeviceCard[],
  summaries: new Map<string, DriveSummary>(),
  scanning: new Map<string, { files: number; bytes: number }>(),
  scanErrors: new Map<string, string>(),
  confirmedId: new Set<string>(),
  advanceWhenScanned: new Set<string>(),
  presets: [] as Preset[],
  hardware: [] as HardwareCard[],
  hwQuery: "",
  presetId: "unknown_club" as string | null,
  targets: [] as string[],
  redundancy: true,
  assessments: new Map<string, DriveAssessment>(),
  plan: null as KitPlan | null,
  role: null as Role | null,
  candidates: [] as Candidate[],
  candQuery: "",
  cell: null as { device: string; layer: Layer } | null,
  showTracks: false,
  showWhy: false,
  confirmation: null as ConfirmationDetails | null,
  prepareSteps: [] as { step: string; detail: string }[],
  prepareError: null as string | null,
  exportBaseline: null as string | null,
  exportTimer: 0,
  exportDetected: false,
  verifyMode: "full" as "quick" | "full",
  verifyRunning: false,
  verifyProgress: null as VerifyProgress | null,
  verifyReport: null as VerifyReport | null,
  /** Device the progress/report above belong to. Never show one drive's result for another. */
  verifyFor: null as string | null,
  copySources: [] as CopySource[],
  /** Device the copy sources above were listed for. */
  copySourcesFor: null as string | null,
  copyRunning: false,
  copyProgress: null as VerifyProgress | null,
  copyReport: null as CopyReport | null,
  copyError: null as string | null,
  done: [] as Done[],
  ejectMessages: new Map<string, string>(),
  alert: null as { kind: "bad" | "warn" | "ok"; text: string } | null,
  toast: null as string | null,
  toastTimer: 0,
  demoSticks: [] as DemoStickInfo[],
  known: [] as KnownMedia[],
  profiles: [] as GigProfile[],
  busy: false,
};

const root = () => document.getElementById("root")!;

function toast(msg: string) {
  S.toast = msg;
  clearTimeout(S.toastTimer);
  S.toastTimer = window.setTimeout(() => {
    S.toast = null;
    render();
  }, 4200);
  render();
}

function go(view: View) {
  if (S.view.v === "export" && view.v !== "export") clearInterval(S.exportTimer);
  if (view.v === "verify" && S.verifyFor !== view.id) {
    S.verifyReport = null;
    S.verifyProgress = null;
    S.verifyFor = view.id;
  }
  S.view = view;
  S.cell = null;
  render();
  window.scrollTo({ top: 0 });
  document.getElementById("main")?.focus({ preventScroll: true });
}

const card = (id: string) => S.devices.find((d) => d.device.id === id);
const summary = (id: string) => S.summaries.get(id);
const nameOf = (id: string) => summary(id)?.identification.display_name ?? card(id)?.identification.display_name ?? "USB drive";
const preset = () => S.presets.find((p) => p.id === S.presetId) ?? null;
const hwName = (id: string) => S.hardware.find((h) => h.id === id)?.model ?? id;

function setPreset(id: string) {
  const p = S.presets.find((x) => x.id === id);
  if (!p) return;
  S.presetId = id;
  S.targets = [...p.devices];
  S.redundancy = p.redundancy;
}

// ------------------------------------------------------------------ data flow

async function refreshDevices() {
  try {
    S.devices = await S.api.listDevices();
  } catch (e) {
    S.alert = { kind: "bad", text: String(e) };
  }
  if (S.info?.demo) S.demoSticks = await S.api.demoAvailable().catch(() => []);
}

async function scan(id: string) {
  S.scanning.set(id, { files: 0, bytes: 0 });
  S.scanErrors.delete(id);
  render();
  try {
    const s = await S.api.analyze(id);
    S.summaries.set(id, s);
    if (s.identification.confirmed_by_user) S.confirmedId.add(id);
    S.assessments.set(id, await S.api.assess(id, S.targets));
  } catch (e) {
    S.scanErrors.set(id, String(e));
  }
  S.scanning.delete(id);
  if (S.advanceWhenScanned.has(id) && S.view.v === "identify" && S.view.id === id) {
    S.advanceWhenScanned.delete(id);
    go({ v: "assessment", id });
    return;
  }
  render();
}

async function openDevice(id: string) {
  S.alert = null;
  S.showTracks = false;
  go({ v: "identify", id });
  await scan(id);
}

async function reassess(id: string) {
  if (!S.targets.length) return;
  S.assessments.set(id, await S.api.assess(id, S.targets));
  S.plan = await S.api.plan(S.targets, S.redundancy, id).catch(() => null);
}

function roleForDevice(id: string): RolePlan | undefined {
  if (S.role) return S.plan?.roles.find((r) => r.role === S.role);
  return S.plan?.roles.find((r) => r.assigned_drive === id);
}

async function onDeviceEvent(e: { kind: string; id?: string; device?: { id: string; bus: unknown } }) {
  await refreshDevices();
  const id = e.kind === "disappeared" ? e.id! : e.device!.id;
  const view = S.view;
  const viewId = "id" in view ? view.id : null;
  if (e.kind === "appeared") {
    const c = card(id);
    if (c?.is_usb && (view.v === "home" || view.v === "kit-next")) {
      if (view.v === "kit-next") S.role = view.role;
      await openDevice(id);
      return;
    }
    if (c?.is_usb) toast(`${c.identification.display_name} connected.`);
  } else if (e.kind === "disappeared") {
    S.summaries.delete(id);
    S.assessments.delete(id);
    if (viewId === id) {
      if (view.v === "preparing" && !S.prepareError) {
        S.alert = { kind: "bad", text: "USB disconnected before preparation finished. Don't use this USB for a gig yet. Plug it back in to rebuild it." };
      } else if (view.v === "verify" && S.verifyRunning) {
        S.alert = { kind: "warn", text: "USB disconnected during verification. It isn't verified." };
      } else {
        toast("USB removed.");
      }
      if (view.v !== "ready") go({ v: "home" });
    }
  } else if (e.kind === "changed" && viewId === id && view.v === "export") {
    void checkExport();
  }
  if (view.v === "copying" && e.kind === "disappeared" && (id === view.id || id === view.from) && S.copyRunning) {
    S.alert = { kind: "warn", text: "A USB was disconnected during the copy. Plug it back in and copy again; it picks up where it stopped." };
  }
  // A drive plugged in or pulled out may change what can be copied from.
  if (S.view.v === "export" && viewId !== id) void loadCopySources(S.view.id);
  render();
}

// ------------------------------------------------------------------ actions

async function loadCopySources(id: string) {
  const list = await S.api.copySources(id).catch(() => [] as CopySource[]);
  S.copySources = list;
  S.copySourcesFor = id;
  if (S.view.v === "export" && S.view.id === id) render();
}

/** Copies only make sense between drives with the same job: a Legacy Rescue
 * needs its own conservative export, and nothing else should get one. */
function copyFits(src: Role | null, dest: Role | null): boolean {
  return (src === "legacy_rescue") === (dest === "legacy_rescue");
}

async function runCopy(id: string, from: string) {
  S.copyRunning = true;
  S.copyProgress = null;
  S.copyReport = null;
  S.copyError = null;
  go({ v: "copying", id, from });
  try {
    const r = await S.api.copyDrive(from, id);
    S.copyReport = r;
    if (!r.cancelled) {
      S.copyRunning = false;
      await scan(id);
      await reassess(id);
      S.verifyMode = "full";
      S.verifyFor = null;
      go({ v: "verify", id });
      toast(`Copied ${plural(r.files_copied, "file")}. A full verification now checks each one against the original.`);
      return;
    }
  } catch (e) {
    S.copyError = String(e);
  }
  S.copyRunning = false;
  render();
}

async function startExportWatch(id: string) {
  S.exportDetected = false;
  void loadCopySources(id);
  const st = await S.api.exportStatus(id).catch(() => null);
  S.exportBaseline = JSON.stringify(st);
  clearInterval(S.exportTimer);
  S.exportTimer = window.setInterval(() => void checkExport(), 2000);
}

async function checkExport() {
  const view = S.view;
  if (view.v !== "export" || S.exportDetected) return;
  const st = await S.api.exportStatus(view.id).catch(() => null);
  if (!st || JSON.stringify(st) === S.exportBaseline) return;
  S.exportDetected = true;
  clearInterval(S.exportTimer);
  toast("rekordbox export detected. Checking USB…");
  await scan(view.id);
  await reassess(view.id);
  const a = S.assessments.get(view.id);
  const libOk = a?.devices.every((d) => d.layers.find((l) => l.layer === "library")?.status === "pass");
  go(libOk ? { v: "verify", id: view.id } : { v: "report", id: view.id });
}

async function prepare(id: string, fs: Filesystem) {
  const conf = S.confirmation;
  if (!conf) return;
  S.prepareSteps = [];
  S.prepareError = null;
  go({ v: "preparing", id });
  try {
    await S.api.prepare(id, conf.token, fs, S.role ?? roleForDevice(id)?.role ?? "main", S.targets);
    S.done = S.done.filter((d) => d.deviceId !== id);
    if (S.verifyFor === id) S.verifyFor = null;
    await refreshDevices();
    await scan(id);
    await reassess(id);
    go({ v: "export", id });
    await startExportWatch(id);
  } catch (e) {
    S.prepareError = String(e);
    render();
  }
}

async function runVerify(id: string) {
  S.verifyFor = id;
  S.verifyRunning = true;
  S.verifyReport = null;
  S.verifyProgress = null;
  render();
  try {
    const r = await S.api.verify(id, S.verifyMode === "full");
    S.verifyReport = r;
    if (r.passed) {
      await scan(id);
      const role = S.role ?? roleForDevice(id)?.role ?? null;
      const targets = role && S.plan ? S.plan.roles.find((x) => x.role === role)?.targets ?? S.targets : S.targets;
      const a = await S.api.assess(id, targets).catch(() => null);
      const s = summary(id);
      S.done = S.done.filter((d) => d.deviceId !== id && (role === null || d.role !== role));
      S.done.push({
        role,
        deviceId: id,
        name: nameOf(id),
        image: s?.identification.image ?? "stick-generic",
        color: s?.identification.color ?? "#6b7280",
        targets,
        assessment: a,
        report: r,
        tracks: s?.tracks_scanned ?? 0,
        playlists: s?.playlists.filter((p) => !p.is_folder).length ?? 0,
      });
    }
  } catch (e) {
    S.alert = { kind: "bad", text: String(e) };
  }
  S.verifyRunning = false;
  render();
}

async function ejectAll() {
  for (const d of S.done) {
    if (!card(d.deviceId)) continue;
    const r = await S.api.eject(d.deviceId);
    S.ejectMessages.set(d.deviceId, r.message);
  }
  render();
}

function nextRole(): RolePlan | undefined {
  return S.plan?.roles.find((r) => !S.done.some((d) => d.role === r.role));
}

async function act(el: HTMLElement) {
  const a = el.dataset.act!;
  const id = el.dataset.id ?? "";
  const view = S.view;
  const vid = "id" in view ? view.id : "";
  switch (a) {
    case "home":
      S.role = null;
      go({ v: "home" });
      await refreshDevices();
      render();
      break;
    case "buying":
      go({ v: "buying" });
      break;
    case "open-device":
      await openDevice(id);
      break;
    case "id-yes":
      S.confirmedId.add(vid);
      if (S.summaries.has(vid)) go({ v: "assessment", id: vid });
      else {
        S.advanceWhenScanned.add(vid);
        render();
      }
      break;
    case "id-different":
      S.candQuery = "";
      S.candidates = await S.api.identityCandidates(vid);
      go({ v: "looks-different", id: vid });
      break;
    case "pick-candidate": {
      const idn = await S.api.confirmIdentity(vid, id);
      const s = summary(vid);
      if (s) S.summaries.set(vid, { ...s, identification: idn });
      S.confirmedId.add(vid);
      toast(`Got it: ${idn.display_name}.`);
      go(S.summaries.has(vid) ? { v: "assessment", id: vid } : { v: "identify", id: vid });
      break;
    }
    case "skip-candidate":
      S.confirmedId.add(vid);
      go(S.summaries.has(vid) ? { v: "assessment", id: vid } : { v: "identify", id: vid });
      break;
    case "prepare-this":
      go({ v: "targets", id: vid });
      break;
    case "detailed-report":
      await reassess(vid);
      go({ v: "report", id: vid });
      break;
    case "preset":
      setPreset(id);
      render();
      break;
    case "know-equipment":
      S.presetId = null;
      S.hwQuery = "";
      S.hardware = await S.api.hardware();
      go({ v: "hardware", id: vid });
      break;
    case "toggle-hw":
      S.presetId = null;
      S.targets = S.targets.includes(id) ? S.targets.filter((t) => t !== id) : [...S.targets, id];
      S.redundancy = false;
      renderHwGrid();
      break;
    case "targets-continue":
      if (!S.targets.length) {
        toast("Pick at least one piece of equipment.");
        break;
      }
      S.busy = true;
      render();
      await reassess(vid);
      S.busy = false;
      go({ v: "report", id: vid });
      break;
    case "cell": {
      const [dev, layer] = id.split("|");
      S.cell = S.cell && S.cell.device === dev && S.cell.layer === layer ? null : { device: dev, layer: layer as Layer };
      render();
      break;
    }
    case "why":
      S.showWhy = !S.showWhy;
      render();
      break;
    case "fix":
      await doFix(vid, id);
      break;
    case "use-role":
      S.role = id as Role;
      toast(`This USB will be your ${ROLE_LABEL[id]} drive.`);
      render();
      break;
    case "switch-device":
      S.role = (el.dataset.role as Role) ?? null;
      await openDevice(id);
      break;
    case "go-confirm": {
      S.confirmation = await S.api.confirmation(vid).catch((e) => {
        toast(String(e));
        return null;
      });
      if (!S.confirmation) break;
      const fs = (el.dataset.fs as Filesystem) ?? S.assessments.get(vid)?.recommended_format.filesystem ?? "fat32";
      go({ v: "confirm", id: vid, fs });
      break;
    }
    case "erase":
      if (view.v === "confirm") await prepare(view.id, view.fs);
      break;
    case "go-export":
      go({ v: "export", id: vid });
      await startExportWatch(vid);
      break;
    case "open-rekordbox":
      await S.api.openRekordbox().catch((e) => toast(String(e)));
      break;
    case "simulate-export":
      await S.api.demoSimulateExport(vid, (S.role ?? roleForDevice(vid)?.role) === "legacy_rescue");
      await checkExport();
      break;
    case "check-export-now":
      S.exportBaseline = "force";
      await checkExport();
      break;
    case "copy-from":
      await runCopy(vid, id);
      break;
    case "cancel-copy":
      await S.api.cancelCopy();
      break;
    case "go-verify":
      S.verifyFor = null;
      go({ v: "verify", id: vid });
      break;
    case "verify-mode":
      S.verifyMode = id as "quick" | "full";
      render();
      break;
    case "run-verify":
      await runVerify(vid);
      break;
    case "cancel-verify":
      await S.api.cancelVerify();
      break;
    case "to-ready":
      go({ v: "ready" });
      break;
    case "next-usb": {
      const r = nextRole();
      if (r) go({ v: "kit-next", role: r.role });
      break;
    }
    case "eject-all":
      await ejectAll();
      break;
    case "save-setup": {
      const name = prompt("Name this setup", preset()?.title ?? "My gig kit");
      if (name) {
        await S.api.saveProfile(name, S.presetId, S.targets, S.redundancy);
        S.profiles = await S.api.profiles();
        toast(`Saved "${name}".`);
      }
      break;
    }
    case "load-profile": {
      const p = S.profiles.find((x) => x.id === id);
      if (p) {
        S.presetId = p.preset;
        S.targets = p.targets;
        S.redundancy = p.redundancy;
        toast(`Using "${p.name}". Plug in a USB.`);
      }
      break;
    }
    case "demo-on":
      await S.api.setDemoMode(true);
      S.info = await S.api.appInfo();
      await refreshDevices();
      render();
      break;
    case "demo-off":
      await S.api.setDemoMode(false);
      S.info = await S.api.appInfo();
      await refreshDevices();
      render();
      break;
    case "demo-insert":
      await S.api.demoInsert(id);
      S.demoSticks = await S.api.demoAvailable();
      render();
      break;
    case "demo-remove":
      await S.api.demoRemove(id);
      S.demoSticks = await S.api.demoAvailable();
      render();
      break;
    case "demo-reset":
      await S.api.demoReset();
      S.done = [];
      S.summaries.clear();
      await refreshDevices();
      go({ v: "home" });
      break;
    case "dismiss-alert":
      S.alert = null;
      render();
      break;
  }
}

async function doFix(id: string, kind: string) {
  switch (kind) {
    case "reformat":
      await act(Object.assign(document.createElement("button"), { dataset: { act: "go-confirm" } }) as HTMLElement);
      break;
    case "export_rekordbox":
    case "reexport_rekordbox":
    case "export_engine":
      go({ v: "export", id });
      await startExportWatch(id);
      break;
    case "remove_apple_double": {
      const n = await S.api.removeAppleDouble(id);
      toast(`Removed ${plural(n, "macOS metadata file")}.`);
      await scan(id);
      await reassess(id);
      render();
      break;
    }
    case "review_tracks":
      S.showTracks = !S.showTracks;
      render();
      break;
    case "repair_filesystem":
      toast(navigator.userAgent.includes("Mac") ? "Open Disk Utility, select the USB and choose First Aid." : "Right-click the USB in Explorer, choose Properties > Tools > Check.");
      break;
    case "use_another_usb":
      go({ v: "home" });
      break;
  }
}

// ------------------------------------------------------------------ views

function shell(content: Raw): Raw {
  const info = S.info;
  return html`<div class="app">
    <header class="topbar">
      <button class="btn link brand" data-act="home" aria-label="BoothReady home"><span class="brand-mark">BR</span><span style="color:var(--text)">BoothReady</span></button>
      <span class="spacer"></span>
      ${info?.demo ? html`<span class="pill accent">Demo mode: simulated USB drives</span>` : ""}
      ${S.done.length ? html`<button class="btn" data-act="to-ready">Gig kit (${S.done.length})</button>` : ""}
    </header>
    <main id="main" tabindex="-1">
      ${S.alert ? html`<div class="banner ${S.alert.kind}" role="alert" style="margin-bottom:18px"><div style="flex:1">${S.alert.text}</div><button class="btn link" data-act="dismiss-alert">Dismiss</button></div>` : ""}
      ${content}
    </main>
    <footer class="foot">
      <span>BoothReady ${info?.version ?? ""}</span>
      <span>Compatibility rules v${info?.rules_version ?? "?"}${info?.rules_review_status === "seed" ? " (seed data, awaiting review against vendor documentation)" : ""}</span>
      <span>Runs locally. Nothing about your music leaves this computer.</span>
    </footer>
    <div aria-live="polite" class="sr-only" id="live">${S.toast ?? ""}</div>
    ${S.toast ? html`<div class="toast" role="status">${S.toast}</div>` : ""}
  </div>`;
}

function homeView(): Raw {
  const usbs = S.devices.filter((d) => d.is_usb);
  const others = S.devices.filter((d) => !d.is_usb);
  return html`
    <section class="hero">
      <div class="plug-art">${raw(usbArt("stick-slider", "#ff7a1a", 150, "A USB drive"))}</div>
      <h1>Plug in a USB</h1>
      <p class="muted" style="max-width:520px">We'll identify it, check what's on it and tell you where it will work.</p>
      <button class="btn link" data-act="buying">I don't have a USB yet</button>
    </section>
    <div class="stack">
      ${usbs.length ? html`<section><div class="eyebrow">Connected</div><div class="devlist">${usbs.map(deviceRow)}</div></section>` : ""}
      ${S.info?.demo ? demoPanel() : html`<p class="small faint" style="text-align:center">No USB handy? <button class="btn link" data-act="demo-on">Try it with simulated drives</button></p>`}
      ${S.profiles.length ? html`<section><div class="eyebrow">My gig kits</div><div class="devlist">${S.profiles.map((p) => html`<button class="devrow" data-act="load-profile" data-id="${p.id}"><div class="grow"><b>${p.name}</b><div class="small muted">${plural(p.targets.length, "player")}${p.redundancy ? ", with backup" : ""}</div></div><span class="pill">Use</span></button>`)}</div></section>` : ""}
      ${S.known.length ? html`<section><div class="eyebrow">Known USBs</div><div class="small muted">${S.known.map((k) => k.nickname ?? k.display_name).join(", ")}</div></section>` : ""}
      ${others.length ? html`<details class="small faint"><summary>${plural(others.length, "other disk")} hidden (not USB drives, never offered for erasing)</summary><ul>${others.map((d) => html`<li>${d.identification.display_name}: ${d.eligibility.reasons.join(" ")}</li>`)}</ul></details>` : ""}
    </div>`;
}

function deviceRow(d: DeviceCard): Raw {
  const v = primaryVolume(d.device);
  return html`<button class="devrow" data-act="open-device" data-id="${d.device.id}">
    ${raw(usbArt(d.identification.image, d.identification.color, 48, d.identification.display_name))}
    <div class="grow"><b>${d.identification.display_name}</b><div class="small muted">${gb(d.device.size_bytes)}${v?.label ? ` · ${v.label}` : ""}${v?.filesystem ? ` · ${FS_LABEL[v.filesystem]}` : ""}</div></div>
    <span class="pill">Check</span></button>`;
}

function demoPanel(): Raw {
  return html`<section class="demo-panel">
    <div style="display:flex;align-items:center;gap:10px"><div class="eyebrow" style="flex:1;margin:0">Simulated USB drives</div><button class="btn link small" data-act="demo-reset">Reset</button><button class="btn link small" data-act="demo-off">Use real drives</button></div>
    ${S.demoSticks.map(
      (s) => html`<div class="row"><div style="flex:1"><b>${s.title}</b> <span class="small muted">${s.size_gb} GB</span><div class="small muted">${s.description}</div></div>
        ${s.inserted ? html`<button class="btn" data-act="demo-remove" data-id="${s.name}">Pull out</button>` : html`<button class="btn primary" data-act="demo-insert" data-id="${s.name}">Plug in</button>`}</div>`,
    )}
  </section>`;
}

function buyingView(): Raw {
  return html`<h1>Choosing a USB for DJ gear</h1>
    <p class="muted">Characteristics matter more than brands. BoothReady doesn't have lab results for specific models yet, so it won't claim any stick is guaranteed.</p>
    <div class="grid-2" style="margin-top:18px">
      <div class="card"><h3>Main USB</h3><ul><li>Big enough for your whole library plus 10 %</li><li>USB-A plug, or a USB-C stick with a USB-A adapter you trust</li><li>A plain flash drive, not a portable SSD, because SSDs can draw more power than older players supply</li></ul></div>
      <div class="card"><h3>Legacy Rescue</h3><ul><li>16 to 32 GB</li><li>USB-A, simple mass-storage stick</li><li>Holds essential playlists in formats older CDJs play</li></ul></div>
      <div class="card"><h3>Backup</h3><ul><li>Same size as Main, or big enough for your essentials</li><li>A different brand from Main, so one bad batch can't take out both</li></ul></div>
      <div class="card"><h3>Test before trusting it</h3><p class="small muted">Plug any new stick into BoothReady and run a full verification after preparing it. Counterfeit sticks that report fake capacity fail that read-back.</p></div>
    </div>
    <div class="actions"><button class="btn" data-act="home">Back</button></div>`;
}

const CONF_TEXT: Record<string, string> = {
  exact: "Exact match",
  strong: "Strong match",
  probable: "Probable match",
  unknown: "Couldn't identify",
};

function identifyView(id: string): Raw {
  const c = card(id);
  const s = summary(id);
  const idn = s?.identification ?? c?.identification;
  const scanning = S.scanning.get(id);
  const err = S.scanErrors.get(id);
  const vol = c ? primaryVolume(c.device) : undefined;
  const waiting = S.advanceWhenScanned.has(id);
  return html`<div class="grid-2">
    <section class="card">
      <div class="eyebrow">USB detected</div>
      <h2>${c?.identification.display_name ?? "USB drive"}</h2>
      <dl class="facts" style="margin-top:12px">
        <dt>Capacity</dt><dd>${gb(c?.device.size_bytes ?? 0)}</dd>
        <dt>Connection</dt><dd>${s?.connection ?? "USB device"}</dd>
        <dt>Current volume</dt><dd>${vol?.label ?? s?.fs_label ?? "No name"}</dd>
        <dt>Filesystem</dt><dd>${s?.filesystem ?? (vol?.filesystem ? FS_LABEL[vol.filesystem] : "Unknown")}</dd>
        <dt>Partition layout</dt><dd>${s?.scheme ?? c?.device.partition_scheme?.toUpperCase() ?? "Unknown"}</dd>
        <dt>Status</dt><dd>${err ? html`<span style="color:var(--bad)">${err}</span>` : scanning ? html`<span class="spinner" aria-hidden="true"></span> Scanning… ${scanning.files ? plural(scanning.files, "track") : ""}` : "Scanned"}</dd>
      </dl>
    </section>
    <section class="card" style="text-align:center">
      <div class="eyebrow">We think this is</div>
      <div class="art-frame">${raw(usbArt(idn?.image ?? "stick-generic", idn?.color ?? "#6b7280", 170, idn?.display_name))}</div>
      <h2 style="margin-top:12px">${idn?.display_name ?? "USB drive"}</h2>
      <p><span class="pill">${CONF_TEXT[idn?.confidence ?? "unknown"]}</span></p>
      <p class="small muted">${idn?.explanation ?? ""}</p>
      <p class="small faint">The picture shows the product family. It can't tell us which chips are inside.</p>
      <div class="actions" style="justify-content:center">
        <button class="btn primary" data-act="id-yes">${waiting ? raw('<span class="spinner"></span> Finishing the scan') : "Yes, that's it"}</button>
        <button class="btn" data-act="id-different">Looks different</button>
      </div>
    </section>
  </div>`;
}

function looksDifferentView(): Raw {
  return html`<h1>Which one looks like yours?</h1>
    <p class="muted">Your answer helps BoothReady recognise this drive next time. It doesn't change the compatibility check.</p>
    <div class="hwgrid" style="margin-top:16px">
      ${S.candidates.map((c) => html`<button class="hw" data-act="pick-candidate" data-id="${c.catalog_id}">${raw(usbArt(c.image, c.color, 110, c.display_name))}<div><b>${c.display_name}</b></div></button>`)}
    </div>
    ${S.candidates.length ? "" : html`<p class="muted">No similar products in the catalog yet.</p>`}
    <div class="actions"><button class="btn" data-act="skip-candidate">None of these</button></div>`;
}

function assessmentView(id: string): Raw {
  const s = summary(id);
  if (!s) return html`<p><span class="spinner"></span> Scanning…</p>`;
  const a = S.assessments.get(id);
  const idn = s.identification;
  return html`
    ${s.interrupted ? html`<div class="banner bad" style="margin-bottom:16px"><div style="flex:1"><b>We found an incomplete BoothReady preparation.</b> This USB may be missing files. Don't use it for a gig until it's finished.</div><button class="btn" data-act="go-export">Resume</button><button class="btn danger" data-act="go-confirm">Rebuild</button></div>` : ""}
    ${s.verified ? html`<div class="banner ok" style="margin-bottom:16px">${raw(statusIcon("pass"))}<div>Verified by BoothReady and unchanged since.</div></div>` : ""}
    <section class="card">
      <div style="display:flex;gap:20px;align-items:center;flex-wrap:wrap">
        <div class="art-frame">${raw(usbArt(idn.image, idn.color, 120, idn.display_name))}</div>
        <div style="flex:1;min-width:260px">
          <h1 style="font-size:30px">${idn.display_name}</h1>
          <p style="font-size:18px;font-weight:600">${s.headline}</p>
          ${s.role ? html`<span class="pill accent">${s.role} drive</span>` : ""}
        </div>
      </div>
      <ul class="checks">${a?.checks.map((c) => html`<li>${raw(statusIcon(c.status))} <span>${c.label}</span></li>`)}
        ${s.dirty ? html`<li>${raw(statusIcon("warn"))} <span>Not ejected safely last time</span></li>` : ""}
        ${s.apple_double ? html`<li>${raw(statusIcon("warn"))} <span>${plural(s.apple_double, "hidden macOS '._' file")} next to your music</span></li>` : ""}
      </ul>
      <div class="stats">
        <div class="stat"><b>${num(a?.tracks.scanned ?? s.tracks_scanned)}</b><span class="small muted">tracks scanned</span></div>
        <div class="stat"><b>${num(a?.tracks.compatible_everywhere ?? 0)}</b><span class="small muted">play on typical club gear</span></div>
        <div class="stat"><b>${num(a?.tracks.need_attention ?? 0)}</b><span class="small muted">need attention</span></div>
      </div>
      <p class="small faint" style="margin-top:10px">${num(s.content.files)} files, ${gb(s.content.used_bytes)} used${s.content.last_modified_unix ? `, last changed ${when(s.content.last_modified_unix)}` : ""}. ${s.layout_source === "os" ? "Layout as reported by the operating system." : ""}</p>
      <div class="actions">
        <button class="btn primary" data-act="prepare-this">Prepare this USB</button>
        <button class="btn" data-act="detailed-report">See detailed report</button>
      </div>
    </section>`;
}

function targetsView(id: string): Raw {
  const order = ["unknown_club", "pioneer_only", "denon_engine", "laptop_mixxx", "mixed", "max_compat"];
  const list = [...S.presets].sort((a, b) => order.indexOf(a.id) - order.indexOf(b.id));
  return html`<h1>What are you preparing for?</h1>
    <p class="muted">You don't need to know model numbers. Pick the closest match.</p>
    <div class="options" style="margin-top:16px" role="radiogroup">
      ${list.map(
        (p) => html`<button class="option" role="radio" aria-pressed="${S.presetId === p.id}" aria-checked="${S.presetId === p.id}" data-act="preset" data-id="${p.id}">
          <span class="radio"></span><span class="grow"><b>${p.title}</b>${p.id === "unknown_club" ? html` <span class="pill accent">Recommended</span>` : ""}<div class="small muted">${p.subtitle}</div></span></button>`,
      )}
      <button class="option" aria-pressed="${S.presetId === null}" data-act="know-equipment"><span class="radio"></span><span class="grow"><b>I know the equipment</b><div class="small muted">Pick the players by picture${S.presetId === null && S.targets.length ? `: ${S.targets.map(hwName).join(", ")}` : ""}</div></span></button>
    </div>
    <div class="actions"><button class="btn primary" data-act="targets-continue" ${S.busy ? "disabled" : ""}>${S.busy ? raw('<span class="spinner"></span> Checking') : "Continue"}</button><button class="btn" data-act="home">Cancel</button></div>
    <input type="hidden" value="${id}">`;
}

function hwGrid(): Raw {
  return html`${S.hardware.map(
    (h) => html`<button class="hw" aria-pressed="${S.targets.includes(h.id)}" data-act="toggle-hw" data-id="${h.id}">
      ${h.legacy ? html`<span class="tag pill">Older</span>` : ""}${raw(hardwareArt(h, 110))}
      <div><b>${h.model}</b></div><div class="small muted">${h.manufacturer}, ${h.released}</div></button>`,
  )}${S.hardware.length ? "" : html`<p class="muted">Nothing matches. Try "CDJ", "Denon" or "old nexus".</p>`}`;
}

function renderHwGrid() {
  const g = document.getElementById("hwgrid");
  if (g) g.innerHTML = hwGrid().s;
  const n = document.getElementById("hwcount");
  if (n) n.textContent = `Continue with ${plural(S.targets.length, "player")}`;
}

function hardwareView(): Raw {
  return html`<h1>Which equipment will you play on?</h1>
    <p class="muted">Select everything you might meet. Searching "old nexus" or "big touch screen" works too.</p>
    <input class="search" id="hwsearch" placeholder="Search equipment" value="${S.hwQuery}" aria-label="Search equipment" style="margin:14px 0">
    <div class="hwgrid" id="hwgrid">${hwGrid()}</div>
    <div class="actions"><button class="btn primary" id="hwcount" data-act="targets-continue">Continue with ${plural(S.targets.length, "player")}</button><button class="btn" data-act="prepare-this">Back</button></div>`;
}

const LAYER_COLS: [Layer, string][] = [
  ["physical", "USB"],
  ["partition", "Partition"],
  ["filesystem", "Filesystem"],
  ["library", "DJ library"],
  ["audio", "Audio"],
];

function cellButton(d: DeviceAssessment, l: LayerResult): Raw {
  const pressed = S.cell?.device === d.device_id && S.cell.layer === l.layer;
  const label = l.layer === "physical" && l.status === "unknown" ? "Untested" : l.status === "pass" ? "OK" : l.headline.length < 22 ? l.headline : l.status === "fail" ? "Not OK" : l.status === "warn" ? "Warning" : "Unknown";
  return html`<button class="cell" aria-pressed="${pressed}" data-act="cell" data-id="${d.device_id}|${l.layer}" title="${l.headline}">${raw(statusIcon(l.status, 18))}<span class="small">${label}</span></button>`;
}

function explainPanel(a: DriveAssessment): Raw {
  if (!S.cell) return html``;
  const d = a.devices.find((x) => x.device_id === S.cell!.device);
  const l = d?.layers.find((x) => x.layer === S.cell!.layer);
  if (!d || !l) return html``;
  return html`<div class="explain" style="margin-top:14px" role="region" aria-label="Explanation">
    <h3>${d.model}: ${l.headline}</h3>
    <p>${l.detail}</p>
    <p class="small muted">Evidence: <b>${EVIDENCE_LABEL[l.evidence]}</b>${l.source ? `. Source: ${l.source}` : ""}</p>
    ${l.layer === "audio" && d.audio.issues.length ? html`<ul class="issue-list">${d.audio.issues.slice(0, 50).map((i) => html`<li><b>${i.path.split("/").pop()}</b>: ${i.reason}</li>`)}</ul>` : ""}
  </div>`;
}

function fixButton(f: Fix): Raw {
  const label: Record<string, string> = {
    reformat: "Fix this",
    export_rekordbox: "Open rekordbox",
    reexport_rekordbox: "Open rekordbox",
    export_engine: "Prepare in Engine DJ",
    remove_apple_double: "Remove them",
    review_tracks: S.showTracks ? "Hide tracks" : "Show tracks",
    repair_filesystem: "How to repair",
    use_another_usb: "Use another USB",
  };
  return html`<button class="btn ${f.destructive ? "danger" : ""}" data-act="fix" data-id="${f.kind}">${label[f.kind] ?? "Fix"}</button>`;
}

function reportView(id: string): Raw {
  const a = S.assessments.get(id);
  const s = summary(id);
  if (!a || !s) return html`<p><span class="spinner"></span> Checking…</p>`;
  const status = verdictStatus(a.overall);
  const trackIssues = a.devices.flatMap((d) => d.audio.issues.map((i) => ({ ...i, model: d.model })));
  const uniq = new Map<string, string[]>();
  for (const i of trackIssues) uniq.set(i.path, [...(uniq.get(i.path) ?? []), `${i.model}: ${i.reason}`]);
  const rec = a.recommended_format;
  const layoutOk = !a.fixes.some((f) => f.kind === "reformat");
  return html`
    <div class="eyebrow">${s.identification.display_name}</div>
    <h1><span class="verdict ${status}">${VERDICT_LABEL[a.overall]}.</span> ${a.headline}</h1>
    <p class="muted">For ${preset()?.title.toLowerCase() ?? S.targets.map(hwName).join(", ")}. <button class="btn link" data-act="prepare-this">Change</button></p>
    <section class="card" style="margin-top:16px;overflow-x:auto">
      <table class="matrix">
        <thead><tr><th>Hardware</th>${LAYER_COLS.map(([, t]) => html`<th>${t}</th>`)}<th>Overall</th></tr></thead>
        <tbody>${a.devices.map(
          (d) => html`<tr><td><b>${d.model}</b>${d.legacy ? html` <span class="small faint">older</span>` : ""}</td>
            ${LAYER_COLS.map(([layer]) => {
              const l = d.layers.find((x) => x.layer === layer)!;
              return html`<td>${cellButton(d, l)}</td>`;
            })}
            <td><span class="verdict ${verdictStatus(d.verdict)}">${VERDICT_LABEL[d.verdict]}</span></td></tr>`,
        )}</tbody>
      </table>
      <p class="small faint" style="margin-top:8px">Select a cell to see why. "Expected to work" means the data behind it is inferred, not documented or tested.</p>
      ${explainPanel(a)}
    </section>
    ${a.fixes.length
      ? html`<h2 style="margin-top:26px">What to do</h2><div class="fixes">${a.fixes.map(
          (f) => html`<div class="fix"><div class="grow"><b>${f.title}</b>${f.destructive ? html` <span class="pill">Erases this USB</span>` : ""}<p class="small muted" style="margin:4px 0 0">${f.detail}</p>
            ${f.kind === "reformat" ? html`<button class="btn link small" data-act="why">${S.showWhy ? "Hide" : "Why?"}</button>${S.showWhy ? html`<p class="small explain">${rec.why}</p>` : ""}` : ""}
            ${f.kind === "review_tracks" && S.showTracks ? html`<ul class="issue-list">${[...uniq.entries()].map(([p, rs]) => html`<li><b>${p}</b><br><span class="muted">${rs.join("; ")}</span></li>`)}</ul>` : ""}
            </div>${fixButton(f)}</div>`,
        )}</div>`
      : html`<div class="banner ok" style="margin-top:20px">${raw(statusIcon("pass"))}<div>Nothing to fix for this equipment. Verify the drive to be sure every file reads back.</div></div>`}
    ${kitSection(id)}
    <div class="actions">
      ${layoutOk ? html`<button class="btn primary" data-act="go-verify">Verify this USB</button>` : html`<button class="btn primary" data-act="go-confirm" data-fs="${rec.filesystem}">Rebuild as ${rec.scheme.toUpperCase()} + ${rec.filesystem === "exfat" ? "exFAT" : "FAT32"}</button>`}
      <button class="btn" data-act="go-export">It's fine, go to the export step</button>
    </div>`;
}

function kitSection(id: string): Raw {
  const plan = S.plan;
  if (!plan || plan.roles.length < 2) return html``;
  const mine = S.role ?? plan.roles.find((r) => r.assigned_drive === id)?.role;
  return html`<h2 style="margin-top:26px">Your USB kit</h2>
    <p class="muted">Several USBs work as a system: each has a job, so together they cover more equipment than copies would.</p>
    <div class="roles">${plan.roles.map((r, i) => {
      const assigned = r.assigned_drive ? card(r.assigned_drive) : null;
      const isThis = mine === r.role;
      return html`<div class="role" style="${isThis ? "border-color:var(--accent)" : ""}">
        <div class="eyebrow">USB ${i + 1}</div><h3>${ROLE_LABEL[r.role]}</h3>
        <div class="small muted">${r.format.scheme.toUpperCase()} + ${r.format.filesystem === "exfat" ? "exFAT" : "FAT32"}, ideally ${gb(r.ideal_capacity[0])}${r.ideal_capacity[1] !== r.ideal_capacity[0] ? ` to ${gb(r.ideal_capacity[1])}` : ""}</div>
        <ul class="small">${r.purpose.map((p) => html`<li>${p}</li>`)}</ul>
        <p class="small muted">${r.why}</p>
        ${r.drive_notes.map((n) => html`<p class="small faint">${n}</p>`)}
        ${isThis ? html`<span class="pill accent">This USB</span>` : assigned && assigned.device.id !== id ? html`<button class="btn" data-act="switch-device" data-id="${assigned.device.id}" data-role="${r.role}">Use ${assigned.identification.display_name}</button>` : html`<button class="btn" data-act="use-role" data-id="${r.role}">Make this USB the ${ROLE_LABEL[r.role]}</button>`}
      </div>`;
    })}</div>
    ${plan.notes.map((n) => html`<p class="small faint" style="margin-top:8px">${n}</p>`)}`;
}

function confirmView(id: string, fs: Filesystem): Raw {
  const c = S.confirmation;
  if (!c) return html``;
  const role = S.role ?? roleForDevice(id)?.role ?? "main";
  return html`<section class="card danger-card" style="max-width:640px;margin:0 auto">
    <div class="eyebrow" style="color:var(--bad)">Rebuild mode</div>
    <h1>You are about to erase:</h1>
    <div style="display:flex;gap:18px;align-items:center;margin:14px 0">
      <div class="art-frame">${raw(usbArt(c.image, c.color, 110, c.display_name))}</div>
      <dl class="facts">
        <dt>Drive</dt><dd>${c.display_name}</dd>
        <dt>Capacity</dt><dd>${gb(c.size_bytes)}</dd>
        <dt>Volume</dt><dd>${c.volume_label ?? "No name"}</dd>
        ${c.serial_tail ? html`<dt>Serial ending</dt><dd>${c.serial_tail}</dd>` : ""}
      </dl>
    </div>
    <div class="banner warn"><div>It currently holds <b>${plural(c.files, "file")}</b>, ${gb(c.used_bytes)} used, most recently changed ${when(c.last_modified_unix)}. All of it will be deleted.</div></div>
    <p style="margin-top:14px">It will be rebuilt as <b>MBR + ${fs === "exfat" ? "exFAT" : "FAT32"}</b> and named <b>${role === "legacy_rescue" ? "BR_LEGACY" : role === "backup" ? "BR_BACKUP" : "BR_MAIN"}</b>. Your computer may ask for an administrator password.</p>
    ${c.eligible ? "" : html`<div class="banner bad">${c.reasons.join(" ")}</div>`}
    <div class="actions">
      <button class="btn danger" data-act="erase" ${c.eligible ? "" : "disabled"}>Erase and prepare ${c.display_name}</button>
      <button class="btn" data-act="detailed-report">Cancel</button>
    </div>
  </section>`;
}

const STEP_ORDER = ["checking", "unmounting", "partitioning", "formatting", "verifying"];
const STEP_TEXT: Record<string, string> = {
  checking: "Confirming this is the drive you selected",
  unmounting: "Unmounting the drive",
  partitioning: "Writing a new partition table",
  formatting: "Formatting",
  verifying: "Checking the new layout",
};

function preparingView(id: string): Raw {
  const seen = S.prepareSteps.map((s) => s.step);
  const current = seen[seen.length - 1];
  return html`<section class="card" style="max-width:640px;margin:0 auto">
    <div class="eyebrow">${nameOf(id)}</div>
    <h1>${S.prepareError ? "Preparation stopped" : "Preparing your USB"}</h1>
    <p class="muted">${S.prepareError ? "" : "Don't unplug the USB."}</p>
    <ol class="steps">${STEP_ORDER.map((st) => {
      const done = seen.includes(st) && st !== current;
      const active = st === current && !S.prepareError;
      const detail = S.prepareSteps.find((x) => x.step === st)?.detail ?? STEP_TEXT[st];
      return html`<li class="${done || active ? "" : "todo"}">${done ? raw(statusIcon("pass", 20)) : active ? raw('<span class="spinner"></span>') : raw(statusIcon("info", 20))} ${detail}</li>`;
    })}</ol>
    ${S.prepareError ? html`<div class="banner bad">${S.prepareError}</div><div class="actions"><button class="btn primary" data-act="detailed-report">Back to the report</button></div>` : ""}
  </section>`;
}

function exportView(id: string): Raw {
  const s = summary(id);
  const role = S.role ?? roleForDevice(id)?.role ?? null;
  const rp = role ? S.plan?.roles.find((r) => r.role === role) : undefined;
  const formats = rp?.library_formats ?? ["rekordbox_device_library", "rekordbox_one_library"];
  const engine = formats.includes("engine_database") && !formats.some((f) => f.startsWith("rekordbox"));
  const legacy = role === "legacy_rescue";
  const label = s?.fs_label ?? "the USB";
  const src = S.copySourcesFor === id ? S.copySources.find((c) => copyFits(c.role, role)) : undefined;
  const software = engine ? "Engine DJ" : "rekordbox";
  return html`<section class="card" style="max-width:720px;margin:0 auto">
    <div class="eyebrow">${nameOf(id)}${role ? ` · ${ROLE_LABEL[role]}` : ""}</div>
    ${src
      ? html`<h1>Put your music on this USB</h1>
        <div class="copy-offer">
          <h2>Copy from ${src.display_name}</h2>
          <p class="muted">It passed verification${src.role ? ` as your ${ROLE_LABEL[src.role]}` : ""}. BoothReady can copy its whole export onto this USB instead of a second export from ${software}, then check every file against the original.</p>
          ${src.problems.length ? html`<div class="banner ${src.needs_erase ? "warn" : "bad"}"><div>${src.problems.join(". ")}.</div></div>` : ""}
          <div class="actions">${src.needs_erase
            ? html`<button class="btn primary" data-act="go-confirm" data-fs="${S.assessments.get(id)?.recommended_format.filesystem ?? "fat32"}">Erase this USB first</button>`
            : html`<button class="btn primary" data-act="copy-from" data-id="${src.device_id}" ${src.problems.length ? "disabled" : ""}>Copy ${plural(src.files, "file")}, ${gb(src.bytes)}</button>`}</div>
        </div>
        <h2>${engine ? "Or add your music in Engine DJ" : "Or export from rekordbox"}</h2>`
      : html`<h1>${engine ? "Now add your music in Engine DJ" : "Now export from rekordbox"}</h1>`}
    <p class="muted">BoothReady doesn't write rekordbox databases itself. rekordbox does that part, then BoothReady checks the result.</p>
    ${engine
      ? html`<ol><li>Open Engine DJ and find <b>${label}</b> under Devices.</li><li>Drag the playlists you need onto it and wait for the sync to finish.</li></ol>`
      : html`<ol>
          <li>Open rekordbox 7.2.11 or later.</li>
          <li>In Export mode, pick <b>${label}</b> and check that it will write ${formats.map((f) => LIB_LABEL[f]).join(" and ")}${formats.length === 1 ? "" : ", which current rekordbox does together"}.</li>
          <li>${legacy ? "Export only your essential playlists. Leave out FLAC, ALAC and anything above 48 kHz, because older players can't play them." : "Export the playlists you want to play."}</li>
          <li>Wait for the export to finish. You don't need to eject.</li></ol>`}
    <div class="banner warn" style="margin-top:12px"><span class="spinner" aria-hidden="true"></span><div>Waiting for the export. BoothReady will notice it by itself.</div></div>
    <div class="actions">
      ${engine ? "" : html`<button class="btn primary" data-act="open-rekordbox">Open rekordbox</button>`}
      ${S.info?.demo ? html`<button class="btn primary" data-act="simulate-export">Simulate the rekordbox export</button>` : ""}
      <button class="btn" data-act="check-export-now">I've finished, check now</button>
      <button class="btn link" data-act="detailed-report">Back to the report</button>
    </div>
  </section>`;
}

function copyingView(id: string, from: string): Raw {
  const p = S.copyProgress;
  const r = S.copyReport;
  const pct = p && p.bytes_total ? Math.min(100, (100 * p.bytes_done) / p.bytes_total) : 0;
  const stopped = S.copyError !== null || r?.cancelled;
  return html`<section class="card" style="max-width:720px;margin:0 auto">
    <div class="eyebrow">${nameOf(id)}</div>
    <h1>${S.copyError ? "The copy stopped" : r?.cancelled ? "Copy paused" : `Copying from ${nameOf(from)}`}</h1>
    ${S.copyRunning
      ? html`<p class="muted">Don't unplug either USB.</p>
        <div style="margin-top:16px"><div class="bar"><span style="width:${pct.toFixed(1)}%"></span></div>
          <p class="small" style="margin-top:8px" aria-live="polite">${p ? `${gb(p.bytes_done)} of ${gb(p.bytes_total)} copied${p.eta_secs != null ? `, about ${Math.max(1, Math.round(p.eta_secs / 60))} min left` : ""}` : "Starting…"}</p>
          <div class="actions"><button class="btn" data-act="cancel-copy">Pause</button></div></div>`
      : ""}
    ${stopped
      ? html`${S.copyError ? html`<div class="banner bad">${S.copyError}</div>` : html`<div class="banner warn"><div>Nothing is lost. Copying again picks up where it stopped.</div></div>`}
        <div class="actions"><button class="btn primary" data-act="copy-from" data-id="${from}">${S.copyError ? "Try again" : "Continue copying"}</button><button class="btn link" data-act="go-export">Back</button></div>`
      : ""}
  </section>`;
}

function verifyView(id: string): Raw {
  const mine = S.verifyFor === id;
  const p = mine ? S.verifyProgress : null;
  const r = mine ? S.verifyReport : null;
  const pct = p && p.bytes_total ? Math.min(100, (100 * p.bytes_done) / p.bytes_total) : 0;
  return html`<section class="card" style="max-width:720px;margin:0 auto">
    <div class="eyebrow">${nameOf(id)}</div>
    <h1>Verify before the gig</h1>
    <p class="muted">Writing files isn't enough. Verification reads them back to catch failing or fake flash memory.</p>
    ${!S.verifyRunning && !r
      ? html`<div class="options" role="radiogroup" style="margin-top:12px">
          <button class="option" role="radio" aria-pressed="${S.verifyMode === "full"}" data-act="verify-mode" data-id="full"><span class="radio"></span><span class="grow"><b>Full</b> <span class="pill accent">For gigs that matter</span><div class="small muted">Reads every music file end to end. Takes a while on big libraries.</div></span></button>
          <button class="option" role="radio" aria-pressed="${S.verifyMode === "quick"}" data-act="verify-mode" data-id="quick"><span class="radio"></span><span class="grow"><b>Quick</b><div class="small muted">Checks the layout, the library and a sample of files.</div></span></button>
        </div>
        <div class="actions"><button class="btn primary" data-act="run-verify">Start verification</button></div>`
      : ""}
    ${S.verifyRunning
      ? html`<div style="margin-top:16px"><div class="bar"><span style="width:${pct.toFixed(1)}%"></span></div>
          <p class="small" style="margin-top:8px" aria-live="polite">${p ? `${gb(p.bytes_done)} of ${gb(p.bytes_total)} verified${p.eta_secs != null ? `, about ${Math.max(1, Math.round(p.eta_secs / 60))} min left` : ""}` : "Starting…"}</p>
          <div class="actions"><button class="btn" data-act="cancel-verify">Cancel</button></div></div>`
      : ""}
    ${r
      ? html`<div style="margin-top:16px"><ul class="checks">${r.checks.map((c) => html`<li>${raw(statusIcon(c.passed ? "pass" : "fail"))} <span><b>${c.name}</b>: ${c.detail}</span></li>`)}</ul>
          ${r.failures.length ? html`<ul class="issue-list">${r.failures.slice(0, 50).map((f) => html`<li><b>${f.path}</b>: ${f.problem}</li>`)}</ul>` : ""}
          <div class="banner ${r.passed ? "ok" : "bad"}">${r.passed ? `Verified ${plural(r.files_checked, "file")}, ${gb(r.bytes_checked)}.` : r.cancelled ? "Verification was cancelled. This USB isn't verified." : "Verification failed. Don't rely on this USB until it's fixed."}</div>
          <div class="actions">${r.passed ? html`<button class="btn primary" data-act="to-ready">Continue</button>` : html`<button class="btn primary" data-act="detailed-report">Back to the report</button><button class="btn" data-act="run-verify">Try again</button>`}</div></div>`
      : ""}
  </section>`;
}

function readyView(): Raw {
  if (!S.done.length) return html`<p class="muted">Nothing verified yet.</p><div class="actions"><button class="btn" data-act="home">Plug in a USB</button></div>`;
  const next = nextRole();
  return html`<div class="readiness">
      ${next ? html`<div class="eyebrow">${S.done.length} of ${S.plan?.roles.length} USBs ready</div><h1>Almost there</h1>` : html`<h1>You're booth-ready</h1>`}
      <p class="muted">Try the USBs in this order if the first one gives you trouble.</p>
    </div>
    <div class="stack" style="max-width:820px;margin:0 auto">
      ${S.done.map((d) => {
        const devs = d.assessment?.devices ?? [];
        const msg = S.ejectMessages.get(d.deviceId);
        return html`<section class="usb-block"><div style="display:flex;gap:14px;align-items:center">
          ${raw(usbArt(d.image, d.color, 64, d.name))}
          <div style="flex:1"><div class="eyebrow" style="margin:0">${d.role ? ROLE_LABEL[d.role] : "Your USB"}</div><h3 style="margin:0">${d.name}</h3>
          <div class="small muted">${plural(d.tracks, "track")}, ${plural(d.playlists, "playlist")}, ${d.report.passed ? "file verification passed" : "not verified"}</div></div>
          ${msg ? html`<span class="small">${msg}</span>` : ""}</div>
          <ul class="hwlist">${devs.map((h) => html`<li>${raw(statusIcon(verdictStatus(h.verdict), 18))} <span>${h.model}${h.verdict === "expected_to_work" ? html` <span class="small faint">(expected)</span>` : h.verdict !== "ready" ? html` <span class="small faint">(${VERDICT_LABEL[h.verdict].toLowerCase()})</span>` : ""}</span></li>`)}</ul>
        </section>`;
      })}
      ${next ? html`<section class="card"><h3>Next: ${ROLE_LABEL[next.role]}</h3><p class="muted">Insert a USB of ${gb(next.ideal_capacity[0])}${next.ideal_capacity[1] !== next.ideal_capacity[0] ? ` to ${gb(next.ideal_capacity[1])}` : " or larger"}. ${next.why}</p><div class="actions"><button class="btn primary" data-act="next-usb">Prepare the ${ROLE_LABEL[next.role]} USB</button></div></section>` : ""}
      <div class="actions" style="justify-content:center">
        <button class="btn ${next ? "" : "primary"}" data-act="eject-all">Eject USBs</button>
        <button class="btn" data-act="save-setup">Save this setup</button>
      </div>
    </div>`;
}

function kitNextView(role: Role): Raw {
  const r = S.plan?.roles.find((x) => x.role === role);
  return html`<section class="hero">
    <div class="plug-art">${raw(usbArt("stick-cap", "#94a3b8", 130, "A USB drive"))}</div>
    <div class="eyebrow">Preparing ${S.plan?.roles.length ?? 1} USBs</div>
    <h1>Next: ${ROLE_LABEL[role]}</h1>
    <p class="muted" style="max-width:560px">${r ? `Insert a USB of ${gb(r.ideal_capacity[0])}${r.ideal_capacity[1] !== r.ideal_capacity[0] ? ` to ${gb(r.ideal_capacity[1])}` : " or larger"}.` : "Insert the next USB."}</p>
    ${S.info?.demo ? demoPanel() : ""}
    <div class="actions"><button class="btn" data-act="to-ready">Back</button></div>
  </section>`;
}

function body(): Raw {
  const v = S.view;
  switch (v.v) {
    case "home":
      return homeView();
    case "buying":
      return buyingView();
    case "identify":
      return identifyView(v.id);
    case "looks-different":
      return looksDifferentView();
    case "assessment":
      return assessmentView(v.id);
    case "targets":
      return targetsView(v.id);
    case "hardware":
      return hardwareView();
    case "report":
      return reportView(v.id);
    case "confirm":
      return confirmView(v.id, v.fs);
    case "preparing":
      return preparingView(v.id);
    case "export":
      return exportView(v.id);
    case "copying":
      return copyingView(v.id, v.from);
    case "verify":
      return verifyView(v.id);
    case "ready":
      return readyView();
    case "kit-next":
      return kitNextView(v.role);
  }
}

export function render() {
  const active = document.activeElement as HTMLElement | null;
  const focusId = active?.id;
  root().innerHTML = shell(body()).s;
  if (focusId) document.getElementById(focusId)?.focus();
  const search = document.getElementById("hwsearch") as HTMLInputElement | null;
  if (search && focusId === "hwsearch") {
    search.setSelectionRange(search.value.length, search.value.length);
  }
}

let searchTimer = 0;

export async function start(api: Api) {
  S.api = api;
  S.info = await api.appInfo();
  S.presets = await api.presets();
  S.hardware = await api.hardware();
  setPreset("unknown_club");
  [S.known, S.profiles] = await Promise.all([api.knownMedia(), api.profiles()]);
  await refreshDevices();
  api.onDeviceEvent((e) => void onDeviceEvent(e as never));
  api.onScanProgress((p) => {
    if (S.scanning.has(p.device_id)) {
      S.scanning.set(p.device_id, { files: p.files, bytes: p.bytes });
      if (S.view.v === "identify") render();
    }
  });
  api.onPrepareProgress((p) => {
    S.prepareSteps.push(p);
    if (S.view.v === "preparing") render();
  });
  api.onCopyProgress((p) => {
    if (S.view.v !== "copying" || p.device_id !== S.view.id) return;
    S.copyProgress = p.progress;
    render();
  });
  api.onVerifyProgress((p) => {
    if (p.device_id !== S.verifyFor) return;
    S.verifyProgress = p.progress;
    if (S.view.v === "verify") render();
  });
  root().addEventListener("click", (ev) => {
    const el = (ev.target as HTMLElement).closest<HTMLElement>("[data-act]");
    if (el && !(el as HTMLButtonElement).disabled) {
      ev.preventDefault();
      void act(el).catch((e) => toast(String(e)));
    }
  });
  root().addEventListener("input", (ev) => {
    const t = ev.target as HTMLInputElement;
    if (t.id === "hwsearch") {
      S.hwQuery = t.value;
      clearTimeout(searchTimer);
      searchTimer = window.setTimeout(async () => {
        S.hardware = await api.hardware(S.hwQuery);
        renderHwGrid();
      }, 150);
    }
  });
  render();
  // A drive that's already plugged in when the app opens goes straight to its check.
  const first = S.devices.find((d) => d.is_usb);
  if (first && S.devices.filter((d) => d.is_usb).length === 1) await openDevice(first.device.id);
}
