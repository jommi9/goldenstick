// The BoothReady UI: one state object, one render function, and actions
// wired through data-act attributes. The window follows
// docs/design/LIBRARY-REDESIGN.md: a player strip, an icon rail, a tree of
// playlists and sticks, the track list, an inspector and a status bar. The
// check, erase, copy and verify flows (PRD §11-§43) open as sheets over it.

import { inTauri } from "./api";
import type { Api } from "./api";
import { hardwareArt, statusIcon, usbArt, VERDICT_LABEL, verdictStatus } from "./art";
import {
  closeLibrarySheet,
  focusSearch,
  initLibrary,
  libraryAct,
  libraryChange,
  libraryInput,
  libraryInspector,
  libraryList,
  libraryListKey,
  librarySortKey,
  libraryClickMods,
  librarySelectedRows,
  libraryDragStart,
  libraryDragOver,
  libraryDragEnd,
  libraryDrop,
  libraryPlayKey,
  libraryScrolled,
  librarySelection,
  librarySheet,
  libraryStatus,
  libraryTree,
  playerStrip,
  refreshLibrary,
  paintWaveforms,
  playersControl,
  compareSection,
  setCompareTarget,
  compareTarget,
} from "./library";
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
  RealTestResult,
  TestScenario,
  VirtualTestSuite,
  VerifyProgress,
  VerifyReport,
} from "./types";
import {
  EVIDENCE_LABEL,
  FS_LABEL,
  gb,
  esc,
  html,
  LIB_LABEL,
  num,
  plural,
  primaryVolume,
  raw,
  Raw,
  ROLE_LABEL,
  RULES_STATUS,
  when,
  isFolded,
  secHead,
  toggleFold,
  unfold,
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

/** What fills the window behind the rail: the library, or one of the two tool pages. */
type Panel = "library" | "testlab" | "devtools";

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
  /** The open sheet; "home" means none. */
  view: { v: "home" } as View,
  panel: "library" as Panel,
  /** The stick selected in the tree, if any. A library node is selected otherwise. */
  node: null as string | null,
  /** The inspector shown or hidden by hand; null follows the window width. */
  insp: null as boolean | null,
  /** Every player BoothReady knows, for names; `hardware` holds the current search. */
  allHardware: [] as HardwareCard[],
  devices: [] as DeviceCard[],
  summaries: new Map<string, DriveSummary>(),
  scanning: new Map<string, { files: number; total?: number; bytes: number }>(),
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
  testScenarios: [] as TestScenario[],
  testLabScenario: "all",
  testLabRunning: false,
  testLabSuite: null as VirtualTestSuite | null,
  testLabError: null as string | null,
  testLabKeepFixtures: false,
  testLabDeviceId: "",
  testLabRealRunning: false,
  testLabReal: null as RealTestResult | null,
  testLabBaselinePath: null as string | null,
  devLog: [] as string[],
  demoEpoch: 0,
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
  const opened = S.view.v !== view.v || ("id" in S.view && "id" in view && S.view.id !== view.id);
  S.view = view;
  S.cell = null;
  render();
  // A new step starts at the top of its sheet.
  if (opened) document.querySelector(".sheet-body")?.scrollTo({ top: 0 });
}

const card = (id: string) => S.devices.find((d) => d.device.id === id);
const summary = (id: string) => S.summaries.get(id);
const nameOf = (id: string) => summary(id)?.identification.display_name ?? card(id)?.identification.display_name ?? "USB drive";
/** The name of something a copy comes from: a drive, or an export built on this computer. */
const copySourceName = (id: string) => (id.startsWith("export:") ? (S.copySources.find((c) => c.device_id === id)?.display_name ?? id.slice(7)) : nameOf(id));
const preset = () => S.presets.find((p) => p.id === S.presetId) ?? null;
const hwName = (id: string) => S.allHardware.find((h) => h.id === id)?.model ?? S.hardware.find((h) => h.id === id)?.model ?? id;
const devEnabled = () => Boolean(S.info?.demo || !inTauri);

function devLog(message: string) {
  if (!devEnabled()) return;
  const stamp = new Date().toLocaleTimeString([], { hour12: false });
  S.devLog.unshift(`${stamp} ${message}`);
  if (S.devLog.length > 80) S.devLog.length = 80;
}

function devSnapshot(): string {
  return JSON.stringify(
    {
      captured_at: new Date().toISOString(),
      app: S.info,
      view: S.view,
      devices: S.devices.map((d) => ({
        id: d.device.id,
        name: d.identification.display_name,
        bus: d.device.bus,
        size_bytes: d.device.size_bytes,
        is_system: d.device.is_system,
        is_usb: d.is_usb,
        partition_scheme: d.device.partition_scheme,
        volumes: d.device.volumes.map((v) => ({ label: v.label, filesystem: v.filesystem, mount_point: v.mount_point, efi_system: v.efi_system })),
      })),
      summaries: [...S.summaries.entries()].map(([id, s]) => ({
        id,
        name: s.identification.display_name,
        scheme: s.scheme,
        filesystem: s.filesystem,
        headline: s.headline,
        verified: s.verified,
        interrupted: s.interrupted,
        connection: s.connection,
      })),
      scanning: [...S.scanning.entries()],
      assessments: [...S.assessments.entries()].map(([id, a]) => ({ id, overall: a.overall, devices: a.devices.map((d) => ({ id: d.device_id, model: d.model, verdict: d.verdict })) })),
      demo_sticks: S.demoSticks,
      done: S.done.map((d) => ({ role: d.role, device_id: d.deviceId, name: d.name, tracks: d.tracks, playlists: d.playlists, passed: d.report.passed })),
      test_lab: {
        scenario: S.testLabScenario,
        running: S.testLabRunning || S.testLabRealRunning,
        suite: S.testLabSuite,
        real: S.testLabReal
          ? { device_id: S.testLabReal.device.id, verdict: S.testLabReal.assessment.overall, report: S.testLabReal.report }
          : null,
        baseline: S.testLabBaselinePath,
      },
      alert: S.alert,
    },
    null,
    2,
  );
}

function setPreset(id: string) {
  const p = S.presets.find((x) => x.id === id);
  if (!p) return;
  S.presetId = id;
  S.targets = [...p.devices];
  S.redundancy = p.redundancy;
}

const testableDevices = () => S.devices.filter((d) => d.is_usb && !d.device.is_system);

async function runVirtualTests() {
  const epoch = S.demoEpoch;
  S.testLabRunning = true;
  S.testLabError = null;
  S.testLabSuite = null;
  render();
  const selected = S.testLabScenario === "all" ? undefined : S.testLabScenario;
  devLog(`virtual test lab started${selected ? `: ${selected}` : ""}`);
  try {
    const suite = await S.api.runVirtualTests(selected, S.testLabKeepFixtures);
    if (epoch !== S.demoEpoch) return;
    S.testLabSuite = suite;
    devLog(`virtual test lab finished: ${suite.passed ? "PASS" : "FAIL"}`);
  } catch (e) {
    if (epoch !== S.demoEpoch) return;
    S.testLabError = String(e);
    devLog(`virtual test lab failed: ${String(e)}`);
  }
  S.testLabRunning = false;
  render();
}

async function runRealUsbTest() {
  const id = S.testLabDeviceId;
  if (!id) {
    toast("Select a USB drive first.");
    return;
  }
  const d = card(id);
  if (!d || !d.is_usb || d.device.is_system) {
    toast("Select a removable USB drive. Internal and system disks are excluded.");
    return;
  }
  const epoch = S.demoEpoch;
  S.testLabRealRunning = true;
  S.testLabError = null;
  S.testLabReal = null;
  render();
  devLog(`real USB test started: ${id}`);
  try {
    const result = await S.api.runRealUsbTest(id, S.targets);
    if (epoch !== S.demoEpoch) return;
    S.testLabReal = result;
    devLog(`real USB test finished: ${id} (${result.assessment.overall})`);
  } catch (e) {
    if (epoch !== S.demoEpoch) return;
    S.testLabError = String(e);
    devLog(`real USB test failed: ${id}: ${String(e)}`);
  }
  S.testLabRealRunning = false;
  render();
}

async function saveTestLabDiagnostics() {
  try {
    S.testLabBaselinePath = await S.api.saveTestLabDiagnostics("baseline");
    devLog(`saved test lab baseline: ${S.testLabBaselinePath}`);
    toast(`Baseline diagnostics saved to ${S.testLabBaselinePath}`);
    render();
  } catch (e) {
    S.testLabError = String(e);
    devLog(`baseline diagnostics failed: ${String(e)}`);
    render();
  }
}

async function copyTestLabReport() {
  const value = S.testLabReal ?? S.testLabSuite;
  if (!value) {
    toast("Run a test first.");
    return;
  }
  try {
    await navigator.clipboard.writeText(JSON.stringify(value, null, 2));
    toast("Test-lab report copied to the clipboard.");
  } catch (e) {
    toast(`Couldn't copy the test-lab report: ${e}`);
  }
}

// ------------------------------------------------------------------ data flow

async function refreshDevices() {
  try {
    S.devices = await S.api.listDevices();
    const selectable = S.devices.find((d) => d.is_usb && !d.device.is_system);
    if (!S.testLabDeviceId || !S.devices.some((d) => d.device.id === S.testLabDeviceId)) S.testLabDeviceId = selectable?.device.id ?? "";
    devLog(`listed ${S.devices.length} device${S.devices.length === 1 ? "" : "s"}`);
  } catch (e) {
    S.alert = { kind: "bad", text: String(e) };
    devLog(`device listing failed: ${String(e)}`);
  }
  if (S.info?.demo) S.demoSticks = await S.api.demoAvailable().catch(() => []);
}

async function scan(id: string) {
  const epoch = S.demoEpoch;
  devLog(`scan started: ${id}`);
  S.scanning.set(id, { files: 0, bytes: 0 });
  S.scanErrors.delete(id);
  render();
  try {
    const s = await S.api.analyze(id);
    if (epoch !== S.demoEpoch) {
      S.scanning.delete(id);
      return;
    }
    S.summaries.set(id, s);
    if (s.identification.confirmed_by_user) S.confirmedId.add(id);
    S.assessments.set(id, await S.api.assess(id, S.targets));
    devLog(`scan finished: ${id} (${s.headline})`);
  } catch (e) {
    S.scanErrors.set(id, String(e));
    devLog(`scan failed: ${id}: ${String(e)}`);
  }
  S.scanning.delete(id);
  if (S.advanceWhenScanned.has(id) && S.view.v === "identify" && S.view.id === id) {
    S.advanceWhenScanned.delete(id);
    go({ v: "assessment", id });
    return;
  }
  render();
  // A selected stick, or the one already compared, is compared again with what it now holds.
  if (S.summaries.has(id) && (S.node === id || compareTarget() === id)) {
    const d = card(id);
    void setCompareTarget(id, d ? stickLabel(d) : id);
  }
}

/** Opens a stick's check as a sheet, picking up a scan that already ran in the background. */
async function openDevice(id: string) {
  S.alert = null;
  S.showTracks = false;
  S.node = id;
  if (S.summaries.has(id) && S.confirmedId.has(id)) {
    go({ v: "assessment", id });
    return;
  }
  go({ v: "identify", id });
  if (!S.summaries.has(id) && !S.scanning.has(id)) await scan(id);
}

/** Reads a stick that was plugged in, so the tree can show its space, libraries and readiness. */
async function scanQuietly(id: string) {
  if (S.summaries.has(id) || S.scanning.has(id)) return;
  await scan(id);
}

async function reassess(id: string) {
  if (!S.targets.length) return;
  const epoch = S.demoEpoch;
  const assessment = await S.api.assess(id, S.targets);
  if (epoch !== S.demoEpoch) return;
  S.assessments.set(id, assessment);
  const plan = await S.api.plan(S.targets, S.redundancy, id).catch(() => null);
  if (epoch !== S.demoEpoch) return;
  S.plan = plan;
}

function roleForDevice(id: string): RolePlan | undefined {
  if (S.role) return S.plan?.roles.find((r) => r.role === S.role);
  return S.plan?.roles.find((r) => r.assigned_drive === id);
}

async function onDeviceEvent(e: { kind: string; id?: string; device?: { id: string; bus: unknown } }) {
  devLog(`device event: ${e.kind} ${e.id ?? e.device?.id ?? ""}`.trim());
  await refreshDevices();
  const id = e.kind === "disappeared" ? e.id! : e.device!.id;
  const view = S.view;
  const viewId = "id" in view ? view.id : null;
  if (e.kind === "appeared") {
    const c = card(id);
    // The next stick of a kit is what that sheet waits for; any other stick
    // appears in the tree with a toast and never takes over the window.
    if (c?.is_usb && view.v === "kit-next") {
      S.role = view.role;
      await openDevice(id);
      return;
    }
    if (c?.is_usb) {
      toast(`${c.identification.display_name} connected. It is listed under Devices.`);
      void scanQuietly(id);
    }
  } else if (e.kind === "disappeared") {
    S.summaries.delete(id);
    S.assessments.delete(id);
    if (S.node === id) S.node = null;
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
  const epoch = S.demoEpoch;
  const list = await S.api.copySources(id).catch(() => [] as CopySource[]);
  if (epoch !== S.demoEpoch) return;
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
  const epoch = S.demoEpoch;
  devLog(`copy started: ${from} -> ${id}`);
  S.copyRunning = true;
  S.copyProgress = null;
  S.copyReport = null;
  S.copyError = null;
  go({ v: "copying", id, from });
  try {
    const r = await S.api.copyDrive(from, id);
    if (epoch !== S.demoEpoch) return;
    S.copyReport = r;
    devLog(`copy finished: ${from} -> ${id} (${r.files_copied} files)`);
    if (!r.cancelled) {
      S.copyRunning = false;
      await scan(id);
      if (epoch !== S.demoEpoch) return;
      await reassess(id);
      if (epoch !== S.demoEpoch) return;
      S.verifyMode = "full";
      S.verifyFor = null;
      go({ v: "verify", id });
      toast(`Copied ${plural(r.files_copied, "file")}. A full verification now checks each one against the original.`);
      return;
    }
  } catch (e) {
    S.copyError = String(e);
    devLog(`copy failed: ${from} -> ${id}: ${String(e)}`);
  }
  S.copyRunning = false;
  render();
}

async function startExportWatch(id: string) {
  const epoch = S.demoEpoch;
  S.exportDetected = false;
  void loadCopySources(id);
  const st = await S.api.exportStatus(id).catch(() => null);
  if (epoch !== S.demoEpoch) return;
  S.exportBaseline = JSON.stringify(st);
  clearInterval(S.exportTimer);
  S.exportTimer = window.setInterval(() => void checkExport(), 2000);
}

async function checkExport() {
  const epoch = S.demoEpoch;
  const view = S.view;
  if (view.v !== "export" || S.exportDetected) return;
  const st = await S.api.exportStatus(view.id).catch(() => null);
  if (epoch !== S.demoEpoch) return;
  if (!st || JSON.stringify(st) === S.exportBaseline) return;
  S.exportDetected = true;
  clearInterval(S.exportTimer);
  toast("rekordbox export detected. Checking USB…");
  await scan(view.id);
  if (epoch !== S.demoEpoch) return;
  await reassess(view.id);
  if (epoch !== S.demoEpoch) return;
  const a = S.assessments.get(view.id);
  const libOk = a?.devices.every((d) => d.layers.find((l) => l.layer === "library")?.status === "pass");
  go(libOk ? { v: "verify", id: view.id } : { v: "report", id: view.id });
}

async function prepare(id: string, fs: Filesystem) {
  const epoch = S.demoEpoch;
  const conf = S.confirmation;
  if (!conf) return;
  S.prepareSteps = [];
  S.prepareError = null;
  devLog(`prepare started: ${id} as ${fs}`);
  go({ v: "preparing", id });
  try {
    await S.api.prepare(id, conf.token, fs, S.role ?? roleForDevice(id)?.role ?? "main", S.targets);
    if (epoch !== S.demoEpoch) return;
    S.done = S.done.filter((d) => d.deviceId !== id);
    if (S.verifyFor === id) S.verifyFor = null;
    await refreshDevices();
    if (epoch !== S.demoEpoch) return;
    await scan(id);
    if (epoch !== S.demoEpoch) return;
    await reassess(id);
    if (epoch !== S.demoEpoch) return;
    go({ v: "export", id });
    await startExportWatch(id);
    devLog(`prepare finished: ${id}`);
  } catch (e) {
    S.prepareError = String(e);
    devLog(`prepare failed: ${id}: ${String(e)}`);
    render();
  }
}

async function runVerify(id: string) {
  const epoch = S.demoEpoch;
  devLog(`verification started: ${id} (${S.verifyMode})`);
  S.verifyFor = id;
  S.verifyRunning = true;
  S.verifyReport = null;
  S.verifyProgress = null;
  render();
  try {
    const r = await S.api.verify(id, S.verifyMode === "full");
    if (epoch !== S.demoEpoch) return;
    S.verifyReport = r;
    devLog(`verification finished: ${id} (${r.passed ? "passed" : "failed"})`);
    if (r.passed) {
      await scan(id);
      if (epoch !== S.demoEpoch) return;
      const role = S.role ?? roleForDevice(id)?.role ?? null;
      const targets = role && S.plan ? S.plan.roles.find((x) => x.role === role)?.targets ?? S.targets : S.targets;
      const a = await S.api.assess(id, targets).catch(() => null);
      if (epoch !== S.demoEpoch) return;
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
    if (epoch !== S.demoEpoch) return;
    S.alert = { kind: "bad", text: String(e) };
    devLog(`verification failed: ${id}: ${String(e)}`);
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
  if (await libraryAct(a, id)) return;
  const view = S.view;
  const vid = "id" in view ? view.id : "";
  switch (a) {
    case "home":
      S.role = null;
      S.panel = "library";
      go({ v: "home" });
      await refreshDevices();
      render();
      break;
    case "close-sheet":
      closeSheet();
      break;
    case "open-devtools":
      if (devEnabled()) {
        S.panel = "devtools";
        go({ v: "home" });
      }
      break;
    case "open-test-lab":
      S.panel = "testlab";
      go({ v: "home" });
      break;
    case "panel-library":
      S.panel = "library";
      render();
      break;
    case "focus-search":
      S.panel = "library";
      render();
      focusSearch();
      break;
    case "toggle-inspector":
      S.insp = !inspectorOpen();
      render();
      break;
    case "rail-devices": {
      S.panel = "library";
      unfold("sec:devices");
      const first = S.devices.find((d) => d.is_usb);
      if (first && !S.node) S.node = first.device.id;
      render();
      document.getElementById("tree-devices")?.scrollIntoView({ block: "start" });
      break;
    }
    case "rail-kit":
      if (S.done.length) go({ v: "ready" });
      else {
        S.panel = "library";
        render();
        document.getElementById("tree-kit")?.scrollIntoView({ block: "start" });
        toast("No stick of this kit is verified yet. Select a stick under Devices to check it.");
      }
      break;
    case "fullscreen":
      await toggleFullscreen();
      break;
    case "fold":
      toggleFold(id);
      render();
      break;
    case "device-select":
      // A stick card works like a drawer: each click folds or unfolds it,
      // and it becomes the stick the inspector shows.
      toggleFold(`dev:${id}`);
      S.panel = "library";
      S.node = id;
      S.insp = true;
      // The inspector offers "Check this stick"; folding never opens a sheet.
      render();
      if (S.summaries.has(id)) {
        const d = card(id);
        void setCompareTarget(id, d ? stickLabel(d) : id);
      }
      break;
    case "device-check":
      await openDevice(id);
      break;
    case "device-report":
      S.node = id;
      await reassess(id);
      go({ v: "report", id });
      break;
    case "device-export":
      S.node = id;
      go({ v: "export", id });
      await startExportWatch(id);
      break;
    case "device-verify":
      S.node = id;
      S.verifyFor = null;
      go({ v: "verify", id });
      break;
    case "run-virtual-tests":
      await runVirtualTests();
      break;
    case "save-testlab-diagnostics":
      await saveTestLabDiagnostics();
      break;
    case "run-real-test":
      await runRealUsbTest();
      break;
    case "copy-testlab-report":
      await copyTestLabReport();
      break;
    case "dismiss-testlab-error":
      S.testLabError = null;
      render();
      break;
    case "testlab-refresh":
      await refreshDevices();
      render();
      break;
    case "copy-dev-snapshot":
      try {
        await navigator.clipboard.writeText(devSnapshot());
        toast("Developer snapshot copied to the clipboard.");
        devLog("copied state snapshot");
      } catch (e) {
        toast(`Couldn't copy the snapshot: ${e}`);
      }
      break;
    case "refresh-devices":
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
      devLog("enabled demo mode");
      await refreshDevices();
      await refreshLibrary();
      render();
      break;
    case "demo-off":
      await S.api.setDemoMode(false);
      S.info = await S.api.appInfo();
      devLog("disabled demo mode");
      await refreshDevices();
      await refreshLibrary();
      render();
      break;
    case "demo-insert":
      await S.api.demoInsert(id);
      devLog(`inserted demo drive: ${id}`);
      S.demoSticks = await S.api.demoAvailable();
      render();
      break;
    case "demo-remove":
      await S.api.demoRemove(id);
      devLog(`removed demo drive: ${id}`);
      S.demoSticks = await S.api.demoAvailable();
      render();
      break;
    case "demo-reset":
      S.demoEpoch += 1;
      await S.api.demoReset();
      clearInterval(S.exportTimer);
      S.exportTimer = 0;
      devLog("reset demo drives");
      await refreshLibrary();
      S.done = [];
      S.summaries.clear();
      S.scanning.clear();
      S.scanErrors.clear();
      S.confirmedId.clear();
      S.advanceWhenScanned.clear();
      S.assessments.clear();
      S.plan = null;
      S.role = null;
      S.targets = [];
      S.presetId = "unknown_club";
      S.redundancy = true;
      S.hardware = [];
      S.hwQuery = "";
      S.candidates = [];
      S.candQuery = "";
      S.cell = null;
      S.showTracks = false;
      S.showWhy = false;
      S.confirmation = null;
      S.prepareSteps = [];
      S.prepareError = null;
      S.exportBaseline = null;
      S.exportDetected = false;
      S.verifyRunning = false;
      S.verifyProgress = null;
      S.verifyReport = null;
      S.verifyFor = null;
      S.copyRunning = false;
      S.copyProgress = null;
      S.copyReport = null;
      S.copyError = null;
      S.copySources = [];
      S.copySourcesFor = null;
      S.ejectMessages.clear();
      S.alert = null;
      S.busy = false;
      S.testLabRunning = false;
      S.testLabRealRunning = false;
      S.testLabSuite = null;
      S.testLabReal = null;
      S.testLabError = null;
      S.testLabBaselinePath = null;
      S.testLabScenario = "all";
      S.node = null;
      await refreshDevices();
      go({ v: "home" });
      break;
    case "dismiss-alert":
      S.alert = null;
      render();
      break;
    case "save-diagnostics":
      try {
        toast(`Diagnostics saved to ${await S.api.saveDiagnostics()}`);
      } catch (e) {
        toast(`Couldn't save diagnostics: ${e}`);
      }
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

/** Where each sheet sits, shown like a player's browse path after the stick's name. */
const SHEET: Record<Exclude<View["v"], "home">, { step: string; narrow: boolean; closable: boolean }> = {
  buying: { step: "Buying guide", narrow: false, closable: true },
  identify: { step: "Identify", narrow: false, closable: true },
  "looks-different": { step: "Identify", narrow: false, closable: true },
  assessment: { step: "Check", narrow: true, closable: true },
  targets: { step: "Players", narrow: true, closable: true },
  hardware: { step: "Players", narrow: false, closable: true },
  report: { step: "Report", narrow: false, closable: true },
  confirm: { step: "Erase", narrow: true, closable: true },
  preparing: { step: "Preparing", narrow: true, closable: false },
  export: { step: "Export", narrow: true, closable: true },
  copying: { step: "Copy", narrow: true, closable: true },
  verify: { step: "Verify", narrow: true, closable: true },
  ready: { step: "Gig kit", narrow: false, closable: true },
  "kit-next": { step: "Next USB", narrow: true, closable: true },
};

/** Ruleset review status in a few words for the status bar; the full note is its tooltip. */
const RULES_SHORT: Record<string, string> = {
  seed: "seed data, not reviewed",
  desk_reviewed: "desk reviewed, not yet tested on hardware",
};

const ICON = {
  library: '<svg class="i" viewBox="0 0 24 24"><path d="M4 5h4v14H4zM10 5h4v14h-4zM16.5 5.5l3.5 1-3 13-3.5-1z"/></svg>',
  search: '<svg class="i" viewBox="0 0 24 24"><circle cx="11" cy="11" r="6"/><path d="M20 20l-4.5-4.5"/></svg>',
  sources: '<svg class="i" viewBox="0 0 24 24"><path d="M3 7h6l2 2h10v10H3z"/><path d="M12 12v5M9.5 14.5h5"/></svg>',
  usb: '<svg class="i" viewBox="0 0 24 24"><rect x="7" y="9" width="10" height="12" rx="2"/><path d="M9 9V4h6v5M10.5 6h0M13.5 6h0"/></svg>',
  kit: '<svg class="i" viewBox="0 0 24 24"><rect x="3" y="7" width="18" height="13" rx="2"/><path d="M9 7V5h6v2"/></svg>',
  lab: '<svg class="i" viewBox="0 0 24 24"><path d="M9 3h6M10 3v6l-5 9a2 2 0 0 0 2 3h10a2 2 0 0 0 2-3l-5-9V3"/></svg>',
  dev: '<svg class="i" viewBox="0 0 24 24"><path d="M8 8l-4 4 4 4M16 8l4 4-4 4M13.5 5l-3 14"/></svg>',
  full: '<svg class="i" viewBox="0 0 24 24" style="width:14px;height:14px"><path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5"/></svg>',
  close: '<svg class="i" viewBox="0 0 24 24" style="width:16px;height:16px"><path d="M6 6l12 12M18 6L6 18"/></svg>',
  folder: '<svg class="i" viewBox="0 0 24 24"><path d="M3 7h6l2 2h10v10H3z"/></svg>',
};

// The window scales with the screen: it is designed at 1900 px wide, so a
// 2560 px display draws everything about 35% larger instead of leaving empty
// space, and a laptop panel draws it slightly smaller, never below 90%.
const DESIGN_WIDTH = 1900;
let zoom = 1;

function applyZoom() {
  zoom = Math.min(1.4, Math.max(0.9, window.innerWidth / DESIGN_WIDTH));
  document.documentElement.style.setProperty("zoom", String(zoom));
}

/** The inspector follows the window until it is toggled: hidden below 1700 px of layout width. */
function inspectorOpen(): boolean {
  return S.insp ?? window.innerWidth / zoom >= 1700;
}

let fullscreen = false;

async function toggleFullscreen() {
  if (inTauri) {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    const w = getCurrentWindow();
    fullscreen = !(await w.isFullscreen());
    await w.setFullscreen(fullscreen);
  } else if (document.fullscreenElement) {
    await document.exitFullscreen();
    fullscreen = false;
  } else {
    await document.documentElement.requestFullscreen().catch(() => {});
    fullscreen = !!document.fullscreenElement;
  }
  render();
}

function closeSheet() {
  if (S.view.v !== "home") {
    if (!SHEET[S.view.v].closable) return;
    if (S.view.v === "kit-next") S.role = null;
    go({ v: "home" });
    return;
  }
  if (closeLibrarySheet()) render();
}

const usbCount = () => S.devices.filter((d) => d.is_usb).length;

function titleBar(): Raw {
  const n = usbCount();
  return html`<header class="titlebar">
    <div class="brand"><b>BOOTH</b>READY</div>
    <span class="pill"><i class="dot ${n ? "ok" : ""}"></i>${n ? `${plural(n, "stick")} connected` : "No stick connected"}</span>
    ${S.info?.demo ? html`<span class="pill accent">Demo mode: simulated USB drives</span>` : ""}
    <span class="spacer"></span>
    <label class="pill judge-pill" title="Every track and stick is checked against these players. Pick the gear you expect at the booth.">Judging against ${playersControl("library-preset-title")}</label>
    ${S.done.length ? html`<button class="pill" data-act="to-ready">Gig kit <b>${S.done.length} ready</b></button>` : ""}
    <button class="pill" data-act="fullscreen" title="Full screen (F)">${raw(ICON.full)}${fullscreen ? "Exit full screen" : "Full screen"}</button>
  </header>`;
}

function rail(): Raw {
  const btn = (act: string, label: string, ic: string, on = false, extra: Raw | string = "") =>
    html`<button data-act="${act}" title="${label}" aria-label="${label}" ${on ? raw('aria-current="page"') : ""}>${raw(ic)}${extra}</button>`;
  return html`<nav class="rail" aria-label="Sections">
    ${btn("panel-library", "Library", ICON.library, S.panel === "library")}
    ${btn("focus-search", "Search (Cmd+F)", ICON.search)}
    ${btn("open-library", "Music sources", ICON.sources)}
    ${btn("rail-devices", "Devices (Cmd+3)", ICON.usb, false, usbCount() ? html`<span class="badge-n">${usbCount()}</span>` : "")}
    ${btn("rail-kit", "Gig kit", ICON.kit)}
    <div class="grow"></div>
    ${btn("open-test-lab", "Test lab", ICON.lab, S.panel === "testlab")}
    ${devEnabled() ? btn("open-devtools", "Dev tools", ICON.dev, S.panel === "devtools") : ""}
  </nav>`;
}

/** The label a player would show for a stick: its volume name, else the product. */
function stickLabel(d: DeviceCard): string {
  const s = summary(d.device.id);
  return s?.fs_label ?? primaryVolume(d.device)?.label ?? d.identification.display_name;
}

const BADGES: [string, (f: string[]) => boolean][] = [
  ["RB", (f) => f.includes("rekordbox_device_library")],
  ["ONELIB", (f) => f.includes("rekordbox_one_library")],
  ["ENGINE", (f) => f.includes("engine_database") || f.includes("engine_prime_legacy")],
  ["SERATO", (f) => f.includes("serato_library")],
];

const STATUS_DOT: Record<string, string> = { pass: "ok", warn: "warn", fail: "bad", unknown: "", info: "" };

function deviceNode(d: DeviceCard): Raw {
  const id = d.device.id;
  const s = summary(id);
  const a = S.assessments.get(id);
  const v = primaryVolume(d.device);
  const used = s?.content.used_bytes ?? null;
  const pct = used === null ? 0 : Math.min(100, (100 * used) / Math.max(1, d.device.size_bytes));
  const fs = s?.filesystem ?? (v?.filesystem ? FS_LABEL[v.filesystem] : null);
  const scanning = S.scanning.get(id);
  const verdict = a ? VERDICT_LABEL[a.overall] : null;
  const on = S.node === id && S.panel === "library";
  const shut = isFolded(`dev:${id}`);
  const playlists = s?.playlists.filter((p) => !p.is_folder) ?? [];
  if (shut) {
    return html`<button class="dev shut ${on ? "on" : ""}" data-act="device-select" data-id="${id}" data-dev="${id}" aria-expanded="false" title="Show this stick">
      <div class="row1"><span class="fold-caret" aria-hidden="true"></span>${raw(ICON.usb)}<span class="lab">${stickLabel(d)}</span>${scanning ? raw('<span class="spinner" aria-hidden="true"></span>') : html`<i class="dot ${a ? STATUS_DOT[verdictStatus(a.overall)] : ""}" title="${verdict ?? "Not checked yet"}"></i>`}</div>
    </button>`;
  }
  return html`<button class="dev ${on ? "on" : ""}" data-act="device-select" data-id="${id}" data-dev="${id}" aria-expanded="true" title="Hide this stick">
      <div class="row1"><span class="fold-caret" aria-hidden="true"></span>${raw(ICON.usb)}<span class="lab">${stickLabel(d)}</span>${scanning ? raw('<span class="spinner" aria-hidden="true"></span>') : html`<i class="dot ${a ? STATUS_DOT[verdictStatus(a.overall)] : ""}" title="${verdict ?? "Not checked yet"}"></i>`}</div>
      <div class="mdl">${[d.identification.display_name, s?.scheme, fs].filter(Boolean).join(" · ")}</div>
      <div class="cap"><i style="width:${used === null ? 0 : Math.max(pct, 1.5)}%"></i></div>
      <div class="minicap num"><span>${used === null ? `${gb(d.device.size_bytes)}` : `${gb(used)} of ${gb(d.device.size_bytes)}`}</span><span>${scanning ? `reading${scanning.files ? `, ${plural(scanning.files, "track")}` : ""}` : s ? plural(s.library_tracks || s.tracks_scanned, "track") : "not read yet"}</span></div>
      <div class="badges">${BADGES.map(([l, has]) => html`<span class="badge ${s && has(s.formats) ? "y" : ""}">${l}</span>`)}</div>
    </button>
    ${playlists.length ? html`<div class="kids">${playlists.slice(0, 40).map((p) => html`<div class="node dim" style="--depth:1"><span class="caret"></span><span class="label">${raw(ICON.folder)}<span class="nm">${p.name}</span><span class="ct">${num(p.tracks)}</span></span></div>`)}</div>` : ""}`;
}

function devicesSection(): Raw {
  const usbs = S.devices.filter((d) => d.is_usb);
  const others = S.devices.filter((d) => !d.is_usb);
  const head = secHead("devices", "Devices", usbs.length ? "" : html`<button class="btn link" data-act="buying">Buying a stick</button>`, 'id="tree-devices"');
  if (isFolded("sec:devices")) return head;
  return html`${head}
    ${usbs.length ? usbs.map(deviceNode) : html`<div class="hint">Plug in a USB stick and it appears here.</div>`}
    ${S.info?.demo
      ? html`<div class="sec"><span class="lbl">Simulated sticks</span>${inTauri ? html`<button class="btn link" data-act="demo-off">Use real drives</button>` : ""}</div>
        ${S.demoSticks.map((d) => html`<div class="sim"><span class="nm" title="${d.description}">${d.title} · ${d.size_gb} GB</span>${d.inserted ? html`<button class="btn small" data-act="demo-remove" data-id="${d.name}">Pull out</button>` : html`<button class="btn small" data-act="demo-insert" data-id="${d.name}">Plug in</button>`}</div>`)}`
      : html`<div class="hint">No stick handy? <button class="btn link" data-act="demo-on">Try simulated drives</button></div>`}
    ${others.length ? html`<div class="hint" title="${others.map((d) => `${d.identification.display_name}: ${d.eligibility.reasons.join(" ")}`).join("\n")}">${plural(others.length, "other disk")} hidden. They are not USB sticks and are never offered for erasing.</div>` : ""}`;
}

function kitSectionTree(): Raw {
  const roles = S.plan?.roles ?? [];
  const head = secHead("kit", "Gig kit", S.done.length ? html`<button class="btn link" data-act="to-ready">Open</button>` : "", 'id="tree-kit"');
  if (isFolded("sec:kit")) return head;
  return html`${head}
    ${roles.map((r) => {
      const done = S.done.find((d) => d.role === r.role);
      const assigned = r.assigned_drive ? card(r.assigned_drive) : undefined;
      const name = done?.name ?? (assigned ? stickLabel(assigned) : "not assigned");
      return html`<div class="node ${done ? "" : "dim"}"><span class="caret"></span><button class="label" ${done ? raw('data-act="to-ready"') : assigned ? raw(`data-act="switch-device" data-id="${esc(assigned.device.id)}" data-role="${r.role}"`) : ""}>${raw(ICON.usb)}<span class="nm">${ROLE_LABEL[r.role]}</span><span class="ct">${done ? "verified" : name}</span></button></div>`;
    })}
    ${S.profiles.map((p) => html`<div class="node"><span class="caret"></span><button class="label" data-act="load-profile" data-id="${p.id}" title="${plural(p.targets.length, "player")}${p.redundancy ? ", with backup" : ""}">${raw(ICON.kit)}<span class="nm">${p.name}</span><span class="ct">use</span></button></div>`)}
    ${S.known.length ? html`<div class="hint">Known sticks: ${S.known.map((k) => k.nickname ?? k.display_name).join(", ")}</div>` : ""}`;
}

function tree(): Raw {
  return html`<aside class="tree" aria-label="Library and devices" data-keep-scroll="tree">
    ${libraryTree()}
    ${devicesSection()}
    ${kitSectionTree()}
  </aside>`;
}

/** The stick inspector. Step 2 of the redesign adds the readiness matrix and the sync queue here. */
function deviceInspector(id: string): Raw {
  const d = card(id);
  if (!d) return html`<div class="empty">That stick was removed.</div>`;
  const s = summary(id);
  const a = S.assessments.get(id);
  const used = s?.content.used_bytes ?? 0;
  const libs = s?.formats.map((f) => LIB_LABEL[f] ?? f) ?? [];
  const scanning = S.scanning.get(id);
  return html`<div class="hd"><div class="lbl">Stick</div><h2 class="stick">${stickLabel(d)}</h2><div class="by">${d.identification.display_name}</div>
      ${s ? html`<div class="cap" style="height:8px;margin-top:12px"><i style="width:${Math.min(100, (100 * used) / Math.max(1, d.device.size_bytes))}%"></i></div>
        <div class="minicap num"><span>${gb(used)} used</span><span>${gb(Math.max(0, d.device.size_bytes - used))} free</span></div>` : ""}
    </div>
    ${s ? compareSection(id) : ""}
    ${a ? html`<section><span class="lbl">Readiness</span><div><span class="verdict ${verdictStatus(a.overall)}">${VERDICT_LABEL[a.overall]}.</span> ${a.headline}</div>
        <div class="hint" style="margin-top:6px">For ${preset()?.title.toLowerCase() ?? S.targets.map(hwName).join(", ")}.</div></section>` : ""}
    <section><dl class="kv">
      <dt>Device</dt><dd><code>${id}</code></dd>
      <dt>Size</dt><dd class="num">${gb(d.device.size_bytes)}</dd>
      <dt>Layout</dt><dd>${s ? [s.scheme, s.filesystem].filter(Boolean).join(" + ") : (d.device.partition_scheme?.toUpperCase() ?? "not read yet")}</dd>
      <dt>Libraries</dt><dd>${s ? (libs.length ? libs.join(", ") : "none") : "not read yet"}</dd>
      ${s ? html`<dt>Tracks</dt><dd class="num">${num(s.tracks_scanned)} scanned${s.library_tracks ? `, ${num(s.library_tracks)} in rekordbox` : ""}</dd>` : ""}
      ${s ? html`<dt>Connection</dt><dd>${s.connection}</dd>` : ""}
    </dl>
    ${scanning ? html`<div class="hint" style="margin-top:8px"><span class="spinner" aria-hidden="true"></span> Reading the stick${scanning.files ? `: ${plural(scanning.files, "track")}` : ""}.</div>` : ""}
    </section>
    <section><div class="actions" style="margin-top:0">
      <button class="btn primary" data-act="device-check" data-id="${id}">Check this stick</button>
      ${s ? html`<button class="btn" data-act="device-report" data-id="${id}">Report</button><button class="btn" data-act="device-export" data-id="${id}">Put music on it</button><button class="btn" data-act="device-verify" data-id="${id}">Verify</button>` : ""}
    </div></section>`;
}

function inspector(): Raw {
  return html`<aside class="insp" id="insp" aria-label="Inspector" data-keep-scroll="insp">${S.node && card(S.node) ? deviceInspector(S.node) : libraryInspector()}</aside>`;
}

function statusBar(): Raw {
  const info = S.info;
  const scanning = [...S.scanning.entries()][0];
  const sel = S.panel === "library" && !S.node ? librarySelection() : "";
  return html`<footer class="status foot">
    ${libraryStatus()}
    ${scanning ? html`<span><span class="spinner" aria-hidden="true"></span>Reading ${nameOf(scanning[0])}${scanning[1].files ? `: ${plural(scanning[1].files, "track")}` : ""}</span>` : html`<span>${plural(usbCount(), "stick")} connected</span>`}
    <span class="spacer"></span>
    ${sel ? html`<span>${sel}</span>` : ""}
    <span title="Compatibility rules v${info?.rules_version ?? "?"}${RULES_STATUS[info?.rules_review_status ?? ""] ?? ""}">Compatibility rules v${info?.rules_version ?? "?"}, ${RULES_SHORT[info?.rules_review_status ?? ""] ?? info?.rules_review_status ?? ""}</span>
    <span title="Nothing about your music leaves this computer.">Runs locally</span>
    <button class="btn link" data-act="save-diagnostics" title="Saves how BoothReady reads your drives to a file in Downloads, for bug reports. No file names.">Save diagnostics</button>
    <span>BoothReady ${info?.version ?? ""}</span>
  </footer>`;
}

function sheet(): Raw {
  let crumbs: string[];
  let content: Raw;
  let narrow: boolean;
  let closable: boolean;
  if (S.view.v !== "home") {
    const meta = SHEET[S.view.v];
    const id = "id" in S.view ? S.view.id : null;
    crumbs = id ? [nameOf(id), meta.step] : S.view.v === "kit-next" || S.view.v === "ready" ? ["Gig kit", meta.step] : ["USB", meta.step];
    if (S.view.v === "ready") crumbs = ["Gig kit"];
    content = body();
    narrow = meta.narrow;
    closable = meta.closable;
  } else {
    const lib = librarySheet();
    if (!lib) return html``;
    ({ crumbs, narrow, closable } = lib);
    content = lib.body;
  }
  const dedup = crumbs.filter((c, i) => c !== crumbs[i - 1]);
  return html`<div class="veil" data-veil>
    <div class="sheet ${narrow ? "narrow" : ""}" role="dialog" aria-modal="true" aria-label="${dedup.join(", ")}">
      <div class="sheet-head"><span class="crumbs">${dedup.map((c, i) => html`${i ? html`<i>▸</i>` : ""}<span>${c}</span>`)}</span>${closable ? html`<button class="sheet-close" data-act="close-sheet" aria-label="Close" title="Close (Esc)">${raw(ICON.close)}</button>` : ""}</div>
      <div class="sheet-body" id="sheet-body" data-keep-scroll="sheet-${S.view.v}">
        ${S.alert ? html`<div class="banner ${S.alert.kind}" role="alert" style="margin-bottom:16px"><div>${S.alert.text}</div><button class="btn link" data-act="dismiss-alert">Dismiss</button></div>` : ""}
        ${content}
      </div>
    </div>
  </div>`;
}

function shell(): Raw {
  const open = inspectorOpen();
  const page = S.panel !== "library";
  return html`<div class="app">
    ${titleBar()}
    ${playerStrip()}
    <div class="main ${page ? "page" : open ? "" : "noinsp"}" id="main">
      ${rail()}
      ${page
        ? html`<div class="pagewrap" data-keep-scroll="page-${S.panel}">${S.alert && S.view.v === "home" ? html`<div class="banner ${S.alert.kind}" role="alert" style="margin:16px 28px 0"><div>${S.alert.text}</div><button class="btn link" data-act="dismiss-alert">Dismiss</button></div>` : ""}${S.panel === "testlab" ? testLabView() : developerView()}</div>`
        : html`${tree()}${libraryList(open)}${open ? inspector() : ""}`}
    </div>
    ${statusBar()}
    ${sheet()}
    <div aria-live="polite" class="sr-only" id="live">${S.toast ?? ""}</div>
    ${S.toast ? html`<div class="toast" role="status">${S.toast}</div>` : ""}
  </div>`;
}

function developerView(): Raw {
  const devices = S.devices;
  return html`<div class="devtools">
    <div class="devtools-heading">
      <div><div class="eyebrow">Local development</div><h1>Developer tools</h1><p class="muted">Inspect the current UI state and drive the simulated USBs without leaving the app.</p></div>
      <button class="btn" data-act="home">Back to app</button>
    </div>
    <div class="dev-grid">
      <section class="card">
        <div class="eyebrow">Environment</div>
        <dl class="facts">
          <dt>API</dt><dd>${S.info?.demo ? "Tauri demo backend" : "Browser mock backend"}</dd>
          <dt>Platform</dt><dd>${S.info?.platform ?? "unknown"}</dd>
          <dt>App version</dt><dd>${S.info?.version ?? "unknown"}</dd>
          <dt>Rules</dt><dd>v${S.info?.rules_version ?? "?"} ${S.info?.rules_review_status ?? ""}</dd>
          <dt>Current view</dt><dd>${S.view.v}</dd>
          <dt>Connected</dt><dd>${plural(devices.length, "device")}</dd>
        </dl>
      </section>
      <section class="card">
        <div class="eyebrow">Quick controls</div>
        <div class="actions" style="justify-content:flex-start;margin-top:10px"><button class="btn primary" data-act="demo-reset">Reset demo state</button><button class="btn" data-act="refresh-devices">Refresh devices</button></div>
        ${S.info?.demo ? html`<p class="small muted" style="margin-bottom:0">The reset removes simulated drive state and returns this panel to a clean starting point.</p>` : html`<p class="small muted" style="margin-bottom:0">The browser mock is active. Use the generated snapshot to reproduce UI states in Playwright.</p>`}
      </section>
    </div>
    <section class="card">
      <div class="devtools-section-heading"><div><div class="eyebrow">Simulated drives</div><h2>Insert or remove fixtures</h2></div>${inTauri ? html`<button class="btn link small" data-act="demo-off">Use real drives</button>` : ""}</div>
      ${S.demoSticks.length ? html`<div class="dev-drive-list">${S.demoSticks.map((s) => html`<div class="dev-drive-row"><div class="grow"><b>${s.title}</b><div class="small muted">${s.name} · ${s.size_gb} GB · ${s.description}</div></div>${s.inserted ? html`<button class="btn" data-act="demo-remove" data-id="${s.name}">Pull out</button>` : html`<button class="btn primary" data-act="demo-insert" data-id="${s.name}">Plug in</button>`}</div>`)}</div>` : html`<p class="muted">Demo controls are unavailable for this backend.</p>`}
    </section>
    <section class="card">
      <div class="devtools-section-heading"><div><div class="eyebrow">Connected devices</div><h2>Open a fixture or inspect the host</h2></div><span class="small muted">Device IDs are safe to share in a bug report.</span></div>
      ${devices.length ? html`<div class="dev-device-list">${devices.map((d) => html`<div class="dev-device-row"><div class="grow"><b>${d.identification.display_name}</b><div class="small muted">${d.device.id} · ${gb(d.device.size_bytes)} · ${d.device.bus === "usb" ? "USB" : "non-USB"}${d.device.is_system ? " · system" : ""}</div></div><button class="btn" data-act="open-device" data-id="${d.device.id}">Open</button></div>`)}</div>` : html`<p class="muted">No devices are currently connected.</p>`}
    </section>
    <section class="card">
      <div class="devtools-section-heading"><div><div class="eyebrow">State snapshot</div><h2>Copy a compact reproduction</h2></div><button class="btn primary" data-act="copy-dev-snapshot">Copy JSON</button></div>
      <pre class="dev-json">${devSnapshot()}</pre>
    </section>
    <section class="card">
      <div class="devtools-section-heading"><div><div class="eyebrow">Event log</div><h2>Recent actions</h2></div><span class="small muted">Newest first</span></div>
      <pre class="dev-log">${S.devLog.length ? S.devLog.join("\n") : "No events yet."}</pre>
    </section>
  </div>`;
}

function testLabView(): Raw {
  const usbDevices = testableDevices();
  const selected = card(S.testLabDeviceId);
  const selectedModel = selected?.device.storage_model ?? selected?.device.usb?.product ?? selected?.identification.display_name ?? "USB drive";
  const targetNames = S.targets.map(hwName).join(", ");
  const suite = S.testLabSuite;
  const real = S.testLabReal;
  return html`<div class="devtools test-lab">
    <div class="devtools-heading">
      <div><div class="eyebrow">Local development</div><h1>Test lab</h1><p class="muted">Run the engine's virtual matrix, save diagnostics and inspect one real USB without changing it.</p></div>
      <button class="btn" data-act="home">Back to app</button>
    </div>
    <div class="banner warn"><div><b>Safety boundary:</b> every test here is read-only for real hardware. The real USB test never prepares, copies or ejects. Internal and system disks are excluded.</div></div>
    ${S.testLabError ? html`<div class="banner bad" role="alert"><div>${S.testLabError}</div><button class="btn link" data-act="dismiss-testlab-error">Dismiss</button></div>` : ""}
    <div class="dev-grid">
      <section class="card">
        <div class="eyebrow">Virtual suite</div>
        <h2>Exercise isolated fixtures</h2>
        <p class="small muted">These scenarios use the same core engine and helper as BoothReady. Fixture writes stay under the report folder.</p>
        <label class="field-label" for="testlab-scenario">Scenario</label>
        <select id="testlab-scenario" class="select" ${S.testLabRunning ? "disabled" : ""}>
          <option value="all" ${S.testLabScenario === "all" ? "selected" : ""}>All scenarios</option>
          ${S.testScenarios.map((s) => html`<option value="${s.id}" ${S.testLabScenario === s.id ? "selected" : ""}>${s.title}</option>`)}
        </select>
        <label class="check-row"><input id="testlab-keep" type="checkbox" ${S.testLabKeepFixtures ? "checked" : ""} ${S.testLabRunning ? "disabled" : ""}><span>Keep fixture folders for inspection</span></label>
        <div class="actions" style="justify-content:flex-start"><button class="btn primary" data-act="run-virtual-tests" ${S.testLabRunning ? "disabled" : ""}>${S.testLabRunning ? "Running…" : "Run virtual tests"}</button><button class="btn" data-act="copy-testlab-report" ${suite ? "" : "disabled"}>Copy report JSON</button></div>
        ${suite ? html`<p class="small muted">Reports: <code>${suite.root}</code></p>` : html`<p class="small faint">No virtual run yet.</p>`}
      </section>
      <section class="card">
        <div class="eyebrow">Diagnostics</div>
        <h2>Capture a baseline</h2>
        <p class="small muted">Save this before inserting a real stick, then save another report if a read looks wrong.</p>
        <button class="btn" data-act="save-testlab-diagnostics">Save baseline diagnostics</button>
        ${S.testLabBaselinePath ? html`<p class="small muted" style="margin-top:12px"><b>Saved:</b><br><code>${S.testLabBaselinePath}</code></p>` : html`<p class="small faint" style="margin-top:12px">No baseline saved in this session.</p>`}
      </section>
    </div>
    <section class="card">
      <div class="devtools-section-heading"><div><div class="eyebrow">Virtual results</div><h2>${suite ? suite.passed ? "All selected scenarios passed" : "A scenario needs attention" : "Waiting for a run"}</h2></div>${suite ? html`<span class="pill ${suite.passed ? "accent" : ""}">${suite.scenarios.filter((s) => s.passed).length}/${suite.scenarios.length} passed</span>` : ""}</div>
      ${suite ? html`<div class="test-results">${suite.scenarios.map((r) => html`<details class="test-result" ${r.passed ? "" : "open"}><summary><span class="test-result-status ${r.passed ? "pass" : "fail"}">${r.passed ? "PASS" : "FAIL"}</span><span class="grow"><b>${r.title}</b><span class="small muted">${r.id} · ${r.duration_ms} ms</span></span></summary><p class="small muted">${r.description}</p>${r.checks.length ? html`<ul class="checks">${r.checks.map((c) => html`<li><span class="test-check ${c.passed ? "pass" : "fail"}">${c.passed ? "✓" : "×"}</span><span><b>${c.name}</b>: ${c.detail}</span></li>`)}</ul>` : ""}${r.error ? html`<div class="banner bad">${r.error}</div>` : ""}${r.artifact ? html`<p class="small muted"><code>${r.artifact}</code></p>` : ""}</details>`)}</div>` : html`<p class="muted">Run the virtual suite to see scenario checks here.</p>`}
    </section>
    <section class="card">
      <div class="devtools-section-heading"><div><div class="eyebrow">Real USB</div><h2>Read-only device test</h2></div><button class="btn" data-act="testlab-refresh">Refresh devices</button></div>
      <p class="small muted">Select the exact stick after checking its ID, model and size. The report records diagnostics before and after the read.</p>
      ${S.info?.demo ? html`<div class="banner warn"><div>Demo mode is on. Turn it off before testing real hardware.</div><button class="btn" data-act="demo-off">Use real drives</button></div>` : ""}
      ${usbDevices.length ? html`<label class="field-label" for="testlab-device">USB drive</label><select id="testlab-device" class="select" ${S.testLabRealRunning || S.info?.demo ? "disabled" : ""}>${usbDevices.map((d) => html`<option value="${d.device.id}" ${S.testLabDeviceId === d.device.id ? "selected" : ""}>${d.identification.display_name} · ${d.device.id} · ${gb(d.device.size_bytes)}</option>`)}</select>
        ${selected ? html`<dl class="facts test-device-facts"><dt>Device ID</dt><dd><code>${selected.device.id}</code></dd><dt>Model</dt><dd>${selectedModel}</dd><dt>Size</dt><dd>${gb(selected.device.size_bytes)}</dd><dt>Mount</dt><dd>${primaryVolume(selected.device)?.mount_point ?? "not mounted"}</dd><dt>Targets</dt><dd>${targetNames || "none selected"}</dd></dl>` : ""}
        <div class="actions" style="justify-content:flex-start"><button class="btn primary" data-act="run-real-test" ${S.testLabRealRunning || S.info?.demo ? "disabled" : ""}>${S.testLabRealRunning ? "Reading…" : "Run read-only test"}</button><button class="btn" data-act="open-device" data-id="${S.testLabDeviceId}" ${S.testLabRealRunning ? "disabled" : ""}>Open in BoothReady</button></div>` : html`<div class="banner"><div>No removable USB drives are available. Internal and system disks are deliberately omitted.</div></div>`}
      ${real ? html`<div class="test-real-result"><div class="devtools-section-heading"><h3>${real.read_ok ? "Content scan complete" : "Content scan incomplete"}</h3><span class="verdict ${verdictStatus(real.assessment.overall)}">${VERDICT_LABEL[real.assessment.overall]}</span></div><p class="small muted">${real.device.display_name} · ${real.device.id} · ${gb(real.device.size_bytes)}</p><p class="small muted">Checks directory access and audio headers. Full file read-back and playback are separate tests.</p>${real.read_issues.map((issue) => html`<p class="small">${issue}</p>`)}<dl class="facts"><dt>Connection</dt><dd>${real.summary.connection}</dd><dt>Before</dt><dd><code>${real.diagnostics_before}</code></dd><dt>After</dt><dd><code>${real.diagnostics_after}</code></dd><dt>Report</dt><dd><code>${real.report}</code></dd></dl><button class="btn" data-act="copy-testlab-report">Copy result JSON</button></div>` : ""}
    </section>
    <section class="card">
      <div class="devtools-section-heading"><div><div class="eyebrow">Scenario catalog</div><h2>What is covered</h2></div><span class="small muted">${plural(S.testScenarios.length, "scenario")}</span></div>
      <div class="test-catalog">${S.testScenarios.map((s) => html`<div><b>${s.title}</b><div class="small muted"><code>${s.id}</code> · ${s.description}</div></div>`)}</div>
    </section>
  </div>`;
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
      <div class="card"><h3>Main USB</h3><ul><li>Big enough for your whole library plus 10 %</li><li>A stick that fits your setup; a USB-C adapter or hub is fine when your computer needs one</li><li>A plain flash drive, not a portable SSD, because SSDs can draw more power than older players supply</li></ul></div>
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
        <dt>Status</dt><dd>${err ? html`<span style="color:var(--bad)">${err}</span>` : scanning ? html`<span class="spinner" aria-hidden="true"></span> Scanning… ${scanning.files ? (scanning.total ? `${num(scanning.files)} of ${plural(scanning.total, "track")}` : plural(scanning.files, "track")) : ""}` : "Scanned"}</dd>
      </dl>
      <p class="small faint" style="margin-bottom:0">This is the link BoothReady negotiated with the computer. A USB-C adapter or hub may be in between; BoothReady checks the storage device itself.</p>
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

/** The DJ libraries on a drive and how many tracks or crates each holds. */
function librarySizes(s: DriveSummary): string {
  const parts: string[] = [];
  if (s.library_tracks) parts.push(`rekordbox ${plural(s.library_tracks, "track")}`);
  if (s.engine_tracks !== null) parts.push(`Engine DJ ${plural(s.engine_tracks, "track")}`);
  if (s.serato_crates !== null) parts.push(`Serato ${plural(s.serato_crates, "crate")}`);
  return parts.length > 1 || s.serato_crates !== null ? `Libraries on this USB: ${parts.join(", ")}.` : "";
}

/** Denon players read only the Engine library, so a smaller one shows them less music. */
function engineGap(s: DriveSummary): string {
  if (s.engine_tracks === null || !s.library_tracks) return "";
  const gap = s.library_tracks - s.engine_tracks;
  if (gap <= Math.max(2, s.library_tracks * 0.02)) return "";
  return `The Engine DJ library lists ${plural(gap, "track")} fewer than the rekordbox export, so Denon players see less of your music.`;
}

function assessmentView(id: string): Raw {
  const s = summary(id);
  if (!s) return html`<p><span class="spinner"></span> Scanning…</p>`;
  const a = S.assessments.get(id);
  const idn = s.identification;
  // Preparing erases the drive, so it is only the main action when the
  // engine says the drive has to be rebuilt.
  const rebuild = !a || a.fixes.some((f) => f.kind === "reformat");
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
        ${s.library_problems.map((p) => html`<li>${raw(statusIcon("warn"))} <span>${p}</span></li>`)}
        ${s.library_notes.map((p) => html`<li>${raw(statusIcon("info"))} <span>${p}</span></li>`)}
        ${engineGap(s) ? html`<li>${raw(statusIcon("warn"))} <span>${engineGap(s)}</span></li>` : ""}
        ${s.dirty ? html`<li>${raw(statusIcon("warn"))} <span>Not ejected safely last time</span></li>` : ""}
        ${s.apple_double ? html`<li>${raw(statusIcon("warn"))} <span>${plural(s.apple_double, "hidden macOS '._' file")} next to your music</span></li>` : ""}
      </ul>
      <div class="stats">
        <div class="stat"><b>${num(a?.tracks.scanned ?? s.tracks_scanned)}</b><span class="small muted">tracks scanned</span></div>
        <div class="stat"><b>${num(a?.tracks.compatible_everywhere ?? 0)}</b><span class="small muted">play on typical club gear</span></div>
        <div class="stat"><b>${num(a?.tracks.need_attention ?? 0)}</b><span class="small muted">in a format some gear can't play</span></div>
      </div>
      ${librarySizes(s) ? html`<p class="small muted" style="margin:10px 0 0">${librarySizes(s)}</p>` : ""}
      <p class="small faint" style="margin-top:10px">${num(s.content.files)} files, ${gb(s.content.used_bytes)} used${s.content.last_modified_unix ? `, last changed ${when(s.content.last_modified_unix)}` : ""}. ${s.layout_source === "os" ? "Layout as reported by the operating system." : ""}</p>
      ${rebuild
        ? html`<div class="actions">
            <button class="btn primary" data-act="prepare-this">Prepare this USB</button>
            <button class="btn" data-act="detailed-report">See detailed report</button>
          </div>`
        : html`<div class="actions">
            <button class="btn primary" data-act="detailed-report">See detailed report</button>
            <button class="btn" data-act="prepare-this">Prepare this USB</button>
          </div>
          <p class="small faint" style="margin:10px 0 0">This drive is already formatted the way the selected players need. Preparing it would erase everything on it.</p>`}
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
            ${f.kind === "reexport_rekordbox" && s.mismatched_tracks.length ? html`<details class="small" style="margin-top:6px"><summary>Show the ${plural(s.mismatched_tracks.length, "track")} that ${s.mismatched_tracks.length === 1 ? "differs" : "differ"}</summary><ul class="issue-list">${s.mismatched_tracks.map((p) => html`<li>${p}</li>`)}</ul></details>` : ""}
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
  // Exports built on this computer come first, then verified drives; a
  // Device Library export carries no OneLibrary, so an Engine DJ role gets no offer.
  const offers = S.copySourcesFor === id ? S.copySources.filter((c) => copyFits(c.role, role) && !(c.export && engine)) : [];
  const software = engine ? "Engine DJ" : "rekordbox";
  const offer = (src: CopySource) => html`<div class="copy-offer" data-copy-source="${src.device_id}">
      <h2>${src.export ? `Copy the export ${src.display_name}` : `Copy from ${src.display_name}`}</h2>
      <p class="muted">${src.export
        ? "BoothReady built this rekordbox Device Library on this computer from your music. Copying it onto this USB puts the tracks, playlists and analysis in place, and every file is then checked against the original."
        : `It passed verification${src.role ? ` as your ${ROLE_LABEL[src.role]}` : ""}. BoothReady can copy its whole export onto this USB instead of a second export from ${software}, then check every file against the original.`}</p>
      ${src.problems.length ? html`<div class="banner ${src.needs_erase ? "warn" : "bad"}"><div>${src.problems.join(". ")}.</div></div>` : ""}
      <div class="actions">${src.needs_erase
        ? html`<button class="btn primary" data-act="go-confirm" data-fs="${S.assessments.get(id)?.recommended_format.filesystem ?? "fat32"}">Erase this USB first</button>`
        : html`<button class="btn primary" data-act="copy-from" data-id="${src.device_id}" ${src.problems.length ? "disabled" : ""}>Copy ${plural(src.files, "file")}, ${gb(src.bytes)}</button>`}</div>
    </div>`;
  return html`<section class="card" style="max-width:720px;margin:0 auto">
    <div class="eyebrow">${nameOf(id)}${role ? ` · ${ROLE_LABEL[role]}` : ""}</div>
    ${offers.length
      ? html`<h1>Put your music on this USB</h1>
        ${offers.map(offer)}
        <h2>${engine ? "Or add your music in Engine DJ" : "Or export from rekordbox"}</h2>`
      : html`<h1>${engine ? "Now add your music in Engine DJ" : "Now export from rekordbox"}</h1>`}
    <p class="muted">${offers.some((o) => o.export) ? "rekordbox can also write this USB itself, and BoothReady checks the result either way." : "BoothReady builds rekordbox Device Libraries with Export to USB in the library; otherwise rekordbox does that part, then BoothReady checks the result."}</p>
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
    <h1>${S.copyError ? "The copy stopped" : r?.cancelled ? "Copy paused" : `Copying ${from.startsWith("export:") ? "the export" : "from"} ${copySourceName(from)}`}</h1>
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
    <div class="plug-art">${raw(usbArt("stick-cap", "#8f8f98", 130, "A USB drive"))}</div>
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
      return html``;
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
  applyZoom();
  const active = document.activeElement as HTMLElement | null;
  const focusId = active?.id;
  const caret = active instanceof HTMLInputElement && active.type !== "checkbox" && active.type !== "range" ? [active.selectionStart, active.selectionEnd] : null;
  // Scrolling regions keep their place across a re-render, so ticking a
  // folder deep in a long list or selecting a row doesn't jump to the top.
  const scrolled = [...root().querySelectorAll<HTMLElement>("[data-keep-scroll]")].map((el) => [el.dataset.keepScroll, el.scrollTop] as const);
  root().innerHTML = shell().s;
  paintWaveforms();
  for (const [key, top] of scrolled) {
    const el = root().querySelector<HTMLElement>(`[data-keep-scroll="${key}"]`);
    if (el) el.scrollTop = top;
  }
  if (focusId) {
    const el = document.getElementById(focusId);
    el?.focus({ preventScroll: true });
    if (caret && el instanceof HTMLInputElement) {
      try {
        el.setSelectionRange(caret[0], caret[1]);
      } catch {
        // Some input types have no caret.
      }
    }
  }
}

/** Selecting a row redraws only the row highlight, the inspector and the status bar, so a double-click lands on the same row. */
function refreshSelection() {
  const list = document.getElementById("list");
  if (!list) return render();
  const rows = list.querySelectorAll<HTMLElement>("tr[data-row]");
  rows.forEach((r) => r.classList.remove("sel"));
  const t = document.createElement("template");
  for (const i of librarySelectedRows()) rows[i]?.classList.add("sel");
  const insp = document.getElementById("insp");
  if (insp && !S.node) {
    t.innerHTML = inspector().s;
    insp.replaceWith(t.content.firstElementChild!);
  }
  const status = root().querySelector("footer.status");
  if (status) {
    t.innerHTML = statusBar().s;
    status.replaceWith(t.content.firstElementChild!);
  }
}

function onKey(ev: KeyboardEvent) {
  const mod = ev.metaKey || ev.ctrlKey;
  const target = ev.target as HTMLElement | null;
  const typing = !!target?.closest("input, textarea, select, [contenteditable]");
  if (S.view.v !== "home" || librarySheet()) {
    if (ev.key === "Escape") {
      ev.preventDefault();
      closeSheet();
    }
    return;
  }
  if (S.panel === "library" && !S.node && librarySortKey(ev)) return;
  if (ev.key === "Escape" && typing) {
    target?.blur();
    return;
  }
  if (mod && ev.key.toLowerCase() === "f") {
    ev.preventDefault();
    void act(actionEl("focus-search"));
  } else if (mod && ev.key.toLowerCase() === "i") {
    ev.preventDefault();
    void act(actionEl("toggle-inspector"));
  } else if (mod && ev.key.toLowerCase() === "e") {
    ev.preventDefault();
    void act(actionEl("library-export-open"));
  } else if (mod && ev.key === "1") {
    ev.preventDefault();
    void act(actionEl("library-open", "*"));
  } else if (mod && ev.key === "2") {
    ev.preventDefault();
    const first = document.querySelector<HTMLElement>('.tree [data-act="library-folder"]');
    if (first) void act(first);
  } else if (mod && ev.key === "3") {
    ev.preventDefault();
    void act(actionEl("rail-devices"));
  } else if (!mod && !typing && (ev.key === "f" || ev.key === "F") && !ev.altKey) {
    ev.preventDefault();
    void toggleFullscreen();
  } else if (S.panel === "library" && !S.node && libraryListKey(ev)) {
    return;
  } else {
    libraryPlayKey(ev);
  }
}

/** A detached element carrying an action, for keyboard shortcuts that do what a button does. */
function actionEl(a: string, id = ""): HTMLElement {
  const el = document.createElement("button");
  el.dataset.act = a;
  if (id) el.dataset.id = id;
  return el;
}

let searchTimer = 0;

export async function start(api: Api) {
  S.api = api;
  S.info = await api.appInfo();
  S.presets = await api.presets();
  S.hardware = await api.hardware();
  S.allHardware = S.hardware;
  S.testScenarios = await api.testLabScenarios().catch(() => []);
  setPreset("unknown_club");
  [S.known, S.profiles] = await Promise.all([api.knownMedia(), api.profiles()]);
  S.plan = await api.plan(S.targets, S.redundancy).catch(() => null);
  await refreshDevices();
  await initLibrary({
    api,
    render,
    toast,
    targets: () => S.targets,
    presets: () => S.presets,
    presetId: () => S.presetId,
    setPreset,
    players: () => S.targets.map((id) => ({ id, model: hwName(id) })),
    deviceSelected: () => S.node !== null,
    selectLibrary: () => {
      S.node = null;
      S.panel = "library";
    },
    refreshSelection,
  });
  api.onDeviceEvent((e) => void onDeviceEvent(e as never));
  api.onScanProgress((p) => {
    if (S.scanning.has(p.device_id)) {
      S.scanning.set(p.device_id, { files: p.files, total: p.total, bytes: p.bytes });
      scanProgressSoon(p.device_id);
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
      libraryClickMods(ev);
      // A double-click on a row loads it into the player.
      if (el.dataset.act === "library-select" && ev.detail >= 2) {
        void libraryAct("library-play-row", el.dataset.id ?? "").catch((e) => toast(String(e)));
        return;
      }
      void act(el).catch((e) => toast(String(e)));
      return;
    }
    // A click on the dimmed window around a sheet closes it, as Escape does.
    if ((ev.target as HTMLElement).matches("[data-veil]")) closeSheet();
  });
  root().addEventListener("scroll", (ev) => libraryScrolled(ev.target as HTMLElement), true);
  root().addEventListener("dragstart", (ev) => libraryDragStart(ev));
  root().addEventListener("dragover", (ev) => libraryDragOver(ev));
  root().addEventListener("dragend", () => libraryDragEnd());
  root().addEventListener("drop", (ev) => void libraryDrop(ev).catch((e) => toast(String(e))));
  root().addEventListener("input", (ev) => {
    const t = ev.target as HTMLInputElement;
    if (libraryInput(t)) return;
    if (t.id === "hwsearch") {
      S.hwQuery = t.value;
      clearTimeout(searchTimer);
      searchTimer = window.setTimeout(async () => {
        S.hardware = await api.hardware(S.hwQuery);
        renderHwGrid();
      }, 150);
    }
  });
  root().addEventListener("change", (ev) => {
    const t = ev.target as HTMLInputElement | HTMLSelectElement;
    if (t.id.startsWith("library-")) {
      void libraryChange(t);
    } else if (t.id === "testlab-scenario") {
      S.testLabScenario = t.value;
      render();
    } else if (t.id === "testlab-device") {
      S.testLabDeviceId = t.value;
      render();
    } else if (t.id === "testlab-keep") {
      S.testLabKeepFixtures = (t as HTMLInputElement).checked;
    }
  });
  document.addEventListener("keydown", onKey);
  window.addEventListener("resize", () => render());
  document.addEventListener("fullscreenchange", () => {
    fullscreen = !!document.fullscreenElement;
    render();
  });
  render();
  // Sticks already plugged in are read in the background, so the tree shows
  // their space, libraries and readiness without opening anything.
  for (const d of S.devices.filter((x) => x.is_usb)) await scanQuietly(d.device.id);
}

/** Scan progress can arrive per file, so it redraws at most every 100 ms. */
/** Scan progress redraws only the stick's card, an open identify sheet and the
 *  status bar, so reading a stick never re-renders the window under the pointer. */
let scanTick = 0;
function scanProgressSoon(id: string) {
  if (scanTick) return;
  scanTick = window.setTimeout(() => {
    scanTick = 0;
    if (S.view.v !== "home" && "id" in S.view && S.view.id === id) return render();
    const t = document.createElement("template");
    const dev = root().querySelector<HTMLElement>(`.dev[data-dev="${CSS.escape(id)}"]`);
    const d = card(id);
    if (dev && d) {
      t.innerHTML = deviceNode(d).s.trim();
      const fresh = t.content.querySelector(".dev");
      if (fresh) dev.replaceWith(fresh);
    }
    const status = root().querySelector("footer.status");
    if (status) {
      t.innerHTML = statusBar().s;
      status.replaceWith(t.content.firstElementChild!);
    }
    if (S.node === id && inspectorOpen()) {
      const insp = document.getElementById("insp");
      if (insp) {
        t.innerHTML = inspector().s;
        insp.replaceWith(t.content.firstElementChild!);
      }
    }
  }, 400);
}

