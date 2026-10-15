// Browser-only stand-in for the Rust backend. Every response comes from
// mock-data.json, which `cargo run -p boothready-app --example mock_snapshot`
// generates by running the real engine over the demo drives.

import type { Api } from "./api";
import type {
  Candidate,
  ConfirmationDetails,
  DeviceCard,
  DeviceEvent,
  DriveAssessment,
  DriveSummary,
  HardwareCard,
  KitPlan,
  LibrarySource,
  FolderNode,
  LibraryTrack,
  Preset,
  StateCounts,
  TrackState,
  Role,
  VerifyProgress,
  VerifyReport,
  TestScenario,
  VirtualTestSuite,
} from "./types";

interface DriveState {
  card: DeviceCard;
  summary: DriveSummary;
  assessments: Record<string, DriveAssessment>;
  confirmation: ConfirmationDetails;
  candidates: Candidate[];
}

interface Snapshot {
  rules: { version: number; review_status: string; review_note: string | null };
  presets: Preset[];
  hardware: HardwareCard[];
  search: Record<string, HardwareCard[]>;
  demo_sticks: { name: string; title: string; size_gb: number; description: string }[];
  drives: Record<string, { initial: DriveState; prepared: DriveState; exported: DriveState }>;
  plan_unknown_club: KitPlan;
  /** The simulated music library, judged once per preset. */
  library: Record<string, { sources: LibrarySource[]; tracks: Record<string, LibraryTrack[]>; collected: { source: LibrarySource; tracks: LibraryTrack[] } }>;
}

type Stage = "initial" | "prepared" | "exported";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

const tones = new Map<string, string>();

/**
 * A quiet 30-second stand-in for a demo track: a soft tone pulsing at 124
 * BPM, pitched from the path so each track sounds different.
 */
function toneUrl(path: string): string {
  const cached = tones.get(path);
  if (cached) return cached;
  let hash = 0;
  for (const c of path) hash = (hash * 31 + c.charCodeAt(0)) >>> 0;
  const rate = 8000;
  const samples = rate * 30;
  const pitch = 110 * 2 ** ((hash % 12) / 12);
  const beat = (60 / 124) * rate;
  const wav = new DataView(new ArrayBuffer(44 + samples));
  const text = (at: number, s: string) => [...s].forEach((c, i) => wav.setUint8(at + i, c.charCodeAt(0)));
  text(0, "RIFF");
  wav.setUint32(4, 36 + samples, true);
  text(8, "WAVEfmt ");
  wav.setUint32(16, 16, true);
  wav.setUint16(20, 1, true);
  wav.setUint16(22, 1, true);
  wav.setUint32(24, rate, true);
  wav.setUint32(28, rate, true);
  wav.setUint16(32, 1, true);
  wav.setUint16(34, 8, true);
  text(36, "data");
  wav.setUint32(40, samples, true);
  for (let i = 0; i < samples; i++) {
    const envelope = Math.exp(-((i % beat) / beat) * 5);
    wav.setUint8(44 + i, 128 + Math.round(24 * envelope * Math.sin((2 * Math.PI * pitch * i) / rate)));
  }
  const url = URL.createObjectURL(new Blob([wav.buffer], { type: "audio/wav" }));
  tones.set(path, url);
  return url;
}

export async function createMockApi(): Promise<Api> {
  const data = (await import("./mock-data.json")).default as unknown as Snapshot;
  const params = new URLSearchParams(location.search);
  const inserted = new Set<string>((params.get("insert") ?? "").split(",").filter(Boolean).map((n) => `demo:${n}`));
  const stage = new Map<string, Stage>();
  const verified = new Set<string>();
  const labels = new Map<string, string>();
  const roles = new Map<string, Role | null>();
  let copyCancelled = false;

  const testScenarios: TestScenario[] = [
    { id: "acceptance-main", title: "PRD acceptance drive", description: "GPT + exFAT with a Device Library and intentionally problematic tracks" },
    { id: "empty-drive", title: "Empty new drive", description: "A blank GPT + exFAT stick with no DJ library" },
    { id: "dual-library-export", title: "Complete rekordbox export", description: "Device Library and OneLibrary with clean audio" },
    { id: "prepare-fat32", title: "Safe fixture preparation", description: "Run the real helper request against an image-backed demo stick" },
    { id: "copy-and-verify", title: "Backup copy and read-back", description: "Copy a complete export to another fixture and verify every file" },
  ];

  // URL parameters make a browser-only reproduction easy to share:
  //   ?insert=sandisk-128&stage=sandisk-128:exported&verified=sandisk-128
  // A bare stage applies to every inserted fixture.
  const stageParam = params.get("stage") ?? "";
  for (const item of stageParam.split(",").filter(Boolean)) {
    const [name, value] = item.includes(":") ? item.split(":", 2) : ["*", item];
    if (!["initial", "prepared", "exported"].includes(value)) continue;
    for (const id of name === "*" ? inserted : [`demo:${name}`]) stage.set(id, value as Stage);
  }
  for (const name of (params.get("verified") ?? "").split(",").filter(Boolean)) verified.add(`demo:${name}`);
  const listeners: Record<string, ((p: any) => void)[]> = {};
  const emit = (name: string, payload: unknown) => (listeners[name] ?? []).forEach((cb) => cb(payload));
  const on = (name: string) => (cb: (p: any) => void) => {
    (listeners[name] ??= []).push(cb);
  };
  const state = (id: string): DriveState => {
    const d = data.drives[id];
    if (!d) throw new Error("That USB drive is no longer connected.");
    return d[stage.get(id) ?? "initial"];
  };
  const withVerified = (id: string, s: DriveSummary): DriveSummary => ({ ...s, verified: verified.has(id), fs_label: labels.get(id) ?? s.fs_label });

  const assessFor = (id: string, targets: string[]): DriveAssessment => {
    const all = state(id).assessments;
    const key = [...targets].sort().join(",");
    const preset = data.presets.find((p) => [...p.devices].sort().join(",") === key);
    if (preset && all[preset.id]) return all[preset.id];
    // Custom selection: cut the full assessment down to the chosen devices.
    const full = all["max_compat"];
    const devices = full.devices.filter((d) => targets.includes(d.device_id));
    const order = ["ready", "expected_to_work", "partial", "unknown", "at_risk", "fix_needed"];
    const overall = devices.reduce((w, d) => (order.indexOf(d.verdict) > order.indexOf(w) ? d.verdict : w), "ready" as DriveAssessment["overall"]);
    return { ...full, devices, overall, fixes: full.fixes.filter((f) => devices.some((d) => d.fixes.some((x) => x.kind === f.kind))) };
  };

  const deviceEvent = (e: DeviceEvent) => emit("device-event", e);

  // The library mock keeps its own list so adding, scanning and removing
  // behave like the desktop app. `?library=empty` starts with no sources.
  const libraryFor = (targets: string[]) => {
    const key = [...targets].sort().join(",");
    const preset = data.presets.find((p) => [...p.devices].sort().join(",") === key);
    return data.library[preset?.id ?? "max_compat"];
  };
  const removedSources = new Set<string>(params.get("library") === "empty" ? data.library["unknown_club"].sources.map((s) => s.id) : []);
  /** Folders added in this session: id, record, and the demo source whose tracks stand in for theirs. */
  const addedSources: { source: LibrarySource; scanned: boolean }[] = [];
  let libraryCancelled = false;
  /** Collections made in this session: the tracks picked from a source, by path. */
  const collections: { id: string; name: string; origin: string; paths: Set<string> }[] = [];
  const collectionTracks = (c: { origin: string; paths: Set<string> }, targets: string[]): LibraryTrack[] => {
    const lib = libraryFor(targets);
    // The engine collected the whole cloud folder for the snapshot, so its
    // tracks are shown as they really read once they are on the computer.
    const pool = c.origin === "demo-cloud" ? lib.collected.tracks : tracksOf(c.origin, targets);
    return pool.filter((t) => c.paths.has(t.path));
  };
  const inFolder = (path: string, folder: string) => !folder || path.startsWith(`${folder}/`);
  const pick = (id: string, targets: string[], filter: string, query: string, folder = ""): LibraryTrack[] => {
    const q = query.trim().toLowerCase();
    return tracksOf(id, targets).filter((t) => inFolder(t.path, folder) && (filter === "all" || t.state === filter) && (!q || t.path.toLowerCase().includes(q)));
  };
  /** Every source's tracks, with the source's name in front of the folder, as the engine does for "*". */
  const allSourcesTracks = (targets: string[]): LibraryTrack[] =>
    listSources(targets).flatMap((s) => tracksOf(s.id, targets).map((t) => ({ ...t, source: s.id, folder: t.folder ? `${s.name}/${t.folder}` : s.name })));
  const foldersOf = (id: string): FolderNode[] => {
    const counts = new Map<string, number>();
    for (const t of tracksOf(id, [])) {
      const parts = t.path.split("/");
      for (let end = 1; end < parts.length; end++) {
        const key = parts.slice(0, end).join("/");
        counts.set(key, (counts.get(key) ?? 0) + 1);
      }
    }
    const byParts = (a: string, b: string) => {
      const x = a.split("/");
      const y = b.split("/");
      for (let i = 0; i < Math.min(x.length, y.length); i++) if (x[i] !== y[i]) return x[i] < y[i] ? -1 : 1;
      return x.length - y.length;
    };
    return [...counts.keys()].sort(byParts).map((path) => ({ path, name: path.split("/").pop() ?? path, depth: path.split("/").length - 1, tracks: counts.get(path) ?? 0 }));
  };
  const collectPlan = (id: string, targets: string[], filter: string, query: string, name: string, folder = "") => {
    const origin = listSources(targets).find((s) => s.id === id);
    if (!origin) throw new Error("That music source was removed.");
    const matching = pick(id, targets, filter, query, folder);
    const tracks = matching.filter((t) => t.state !== "unreadable");
    const online = tracks.filter((t) => t.state === "cloud_only");
    const sum = (list: LibraryTrack[]) => list.reduce((n, t) => n + t.size, 0);
    const clean = (name.trim() || `${origin.name} 2026-09-21`).replace(/[^\p{L}\p{N} \-_().,&']/gu, "-").trim();
    if (!clean) throw new Error("Give the collection a name.");
    return {
      plan: {
        tracks: tracks.length,
        bytes: sum(tracks),
        cloud_only: online.length,
        cloud_only_bytes: sum(online),
        unreadable: matching.length - tracks.length,
        name: clean,
        destination: `/Users/dj/Music/BoothReady/${clean}`,
        exists: collections.some((c) => c.name === clean),
        needed_bytes: sum(tracks) + 4 * 1024 * 1024,
        free_bytes: 212_000_000_000,
      },
      origin,
      tracks,
    };
  };
  const countStates = (tracks: LibraryTrack[]): StateCounts => {
    const counts: StateCounts = { ready: 0, cloud_only: 0, blocked: 0, unchecked: 0, unreadable: 0 };
    for (const t of tracks) counts[t.state as TrackState] += 1;
    return counts;
  };
  const tracksOf = (id: string, targets: string[]): LibraryTrack[] => {
    const lib = libraryFor(targets);
    const made = collections.find((c) => c.id === id);
    const list = lib.tracks[id] ?? (made ? collectionTracks(made, targets) : addedSources.find((a) => a.source.id === id && a.scanned) ? lib.tracks["demo-computer"] : []);
    // Stand-in tracks borrowed from another source still belong to this one.
    return list.some((t) => t.source !== id) ? list.map((t) => ({ ...t, source: id })) : list;
  };
  const listSources = (targets: string[]): LibrarySource[] => {
    const lib = libraryFor(targets);
    const added = addedSources.map(({ source, scanned }) => {
      if (!scanned) return { ...source };
      const tracks = tracksOf(source.id, targets);
      const bytes = tracks.reduce((n, t) => n + t.size, 0);
      return { ...source, scanned_unix: Math.floor(Date.now() / 1000), totals: { tracks: tracks.length, bytes, cloud_only: 0, cloud_only_bytes: 0, unreadable: tracks.filter((t) => t.state === "unreadable").length }, states: countStates(tracks) };
    });
    const made = collections.map((c): LibrarySource => {
      const tracks = collectionTracks(c, targets);
      const origin = [...lib.sources, ...added].find((s) => s.id === c.origin);
      return {
        ...lib.collected.source,
        id: c.id,
        name: c.name,
        path: `/Users/dj/Music/BoothReady/${c.name}/files`,
        collected_from: origin?.name ?? lib.collected.source.collected_from,
        totals: { tracks: tracks.length, bytes: tracks.reduce((n, t) => n + t.size, 0), cloud_only: 0, cloud_only_bytes: 0, unreadable: 0 },
        states: countStates(tracks),
      };
    });
    return [...lib.sources, ...added, ...made].filter((s) => !removedSources.has(s.id));
  };

  return {
    appInfo: async () => ({
      version: "0.1.0",
      demo: true,
      platform: "browser mock",
      rules_version: data.rules.version,
      rules_review_status: data.rules.review_status,
      rules_review_note: data.rules.review_note,
    }),
    setDemoMode: async () => {},
    listDevices: async () => [...inserted].map((id) => state(id).card),
    analyze: async (id) => {
      const s = state(id);
      const total = s.summary.tracks_scanned;
      for (let i = 1; i <= total; i++) {
        await sleep(90);
        emit("scan-progress", { device_id: id, files: i, bytes: i * 4_200_000, current: "" });
      }
      await sleep(250);
      return withVerified(id, s.summary);
    },
    identityCandidates: async (id) => {
      const s = state(id);
      const all = Object.values(data.drives).map((d) => d.initial.card.identification);
      const extra: Candidate[] = all
        .filter((i) => i.catalog_id && i.catalog_id !== s.summary.identification.catalog_id)
        .map((i) => ({ catalog_id: i.catalog_id!, display_name: i.display_name, image: i.image, color: i.color }));
      return [...s.candidates, ...extra];
    },
    confirmIdentity: async (id, catalogId) => {
      const s = state(id);
      const other = Object.values(data.drives).map((d) => d.initial.card.identification).find((i) => i.catalog_id === catalogId);
      return { ...s.summary.identification, ...(other ?? {}), confirmed_by_user: true, confidence: s.summary.identification.confidence };
    },
    presets: async () => data.presets,
    hardware: async (q) => {
      const query = (q ?? "").trim().toLowerCase();
      if (!query) return data.hardware;
      if (data.search[query]) return data.search[query];
      return data.hardware.filter((h) => `${h.manufacturer} ${h.model} ${h.id}`.toLowerCase().includes(query));
    },
    assess: async (id, targets) => assessFor(id, targets),
    plan: async () => {
      const plan: KitPlan = JSON.parse(JSON.stringify(data.plan_unknown_club));
      for (const r of plan.roles) {
        if (r.assigned_drive && !inserted.has(r.assigned_drive)) r.assigned_drive = null;
      }
      return plan;
    },
    confirmation: async (id) => state(id).confirmation,
    prepare: async (id, _token, _fs, role) => {
      labels.set(id, role === "legacy_rescue" ? "BR_LEGACY" : role === "backup" ? "BR_BACKUP" : "BR_MAIN");
      roles.set(id, role);
      for (const [step, detail] of [
        ["checking", "Confirming this is the drive you selected"],
        ["unmounting", "Unmounting the drive"],
        ["partitioning", "Writing a new MBR partition table"],
        ["formatting", "Formatting as FAT32"],
        ["verifying", "Checking the new layout"],
      ]) {
        emit("prepare-progress", { step, detail });
        await sleep(700);
      }
      stage.set(id, "prepared");
      verified.delete(id);
      deviceEvent({ kind: "changed", device: state(id).card.device });
      return "{}";
    },
    removeAppleDouble: async () => 2,
    exportStatus: async (id) => {
      const st = stage.get(id) ?? "initial";
      const f = state(id).summary.formats;
      const t = st === "exported" ? 2_000_000_000 : 1_000_000_000;
      return {
        device_library: f.includes("rekordbox_device_library") ? t : null,
        one_library: f.includes("rekordbox_one_library") ? t : null,
        engine: null,
      };
    },
    openRekordbox: async () => {},
    verify: async (id, full) => {
      const s = state(id).summary;
      const total = Math.max(s.content.used_bytes, 1) * (full ? 1 : 0.3);
      const files = Math.max(s.tracks_scanned, 1);
      for (let i = 1; i <= 20; i++) {
        await sleep(120);
        const p: VerifyProgress = {
          bytes_done: Math.round((total * i) / 20),
          bytes_total: Math.round(total),
          files_done: Math.round((files * i) / 20),
          files_total: files,
          current: "",
          eta_secs: i > 5 ? Math.round((20 - i) * 0.12) : null,
        };
        emit("verify-progress", { device_id: id, progress: p });
      }
      verified.add(id);
      const report: VerifyReport = {
        mode: full ? "full" : "quick",
        passed: true,
        cancelled: false,
        checks: [
          { name: "Filesystem", passed: true, detail: `${s.scheme} + ${s.filesystem}` },
          { name: "DJ library", passed: true, detail: "rekordbox Device Library, rekordbox OneLibrary OK" },
          { name: full ? "Every file read back" : "Sampled files read back", passed: true, detail: `${files} files` },
        ],
        files_checked: files,
        bytes_checked: Math.round(total),
        failures: [],
        content_fingerprint: "mock",
        completed_unix: Math.floor(Date.now() / 1000),
      };
      return report;
    },
    cancelVerify: async () => {},
    copySources: async (id) =>
      [...verified]
        .filter((src) => src !== id && inserted.has(src))
        .map((src) => {
          const s = state(src).summary;
          const other = (stage.get(id) ?? "initial") !== "prepared" ? state(id).summary.content.files : 0;
          const problems = other ? [`The destination already holds ${other} other files. Prepare it first so two libraries don't get mixed`] : [];
          return { device_id: src, display_name: s.identification.display_name, role: roles.get(src) ?? null, files: s.tracks_scanned + 12, bytes: s.content.used_bytes, problems, needs_erase: other > 0 };
        }),
    copyDrive: async (from, to) => {
      copyCancelled = false;
      const s = state(from).summary;
      const files = s.tracks_scanned + 12;
      for (let i = 1; i <= 20; i++) {
        if (copyCancelled) return { files_copied: Math.round((files * i) / 20), bytes_copied: 0, files_skipped: 0, files_removed: 0, cancelled: true };
        await sleep(120);
        const p: VerifyProgress = {
          bytes_done: Math.round((s.content.used_bytes * i) / 20),
          bytes_total: s.content.used_bytes,
          files_done: Math.round((files * i) / 20),
          files_total: files,
          current: "",
          eta_secs: i > 5 ? Math.round((20 - i) * 0.12) : null,
        };
        emit("copy-progress", { device_id: to, progress: p });
      }
      stage.set(to, "exported");
      verified.delete(to);
      deviceEvent({ kind: "changed", device: state(to).card.device });
      return { files_copied: files, bytes_copied: s.content.used_bytes, files_skipped: 0, files_removed: 0, cancelled: false };
    },
    cancelCopy: async () => {
      copyCancelled = true;
    },
    saveDiagnostics: async () => "~/Downloads/boothready-diagnostics-demo.json",
    testLabScenarios: async () => testScenarios,
    runVirtualTests: async (scenario) => {
      await sleep(280);
      const selected = scenario && scenario !== "all" ? testScenarios.filter((s) => s.id === scenario) : testScenarios;
      if (!selected.length) throw new Error(`Unknown virtual scenario: ${scenario}`);
      const results = selected.map((s) => ({
        id: s.id,
        title: s.title,
        description: s.description,
        passed: true,
        duration_ms: 8,
        checks: [{ name: "browser-fixture", passed: true, detail: "UI mock only; run the desktop lab for engine coverage" }],
        artifact: null,
        error: null,
      }));
      return { kind: "virtual", passed: true, root: "browser-mock", scenarios: results } as VirtualTestSuite;
    },
    saveTestLabDiagnostics: async () => "~/Downloads/boothready-test-lab/diagnostics-browser-mock.json",
    runRealUsbTest: async () => {
      throw new Error("Real USB tests need the Tauri desktop backend.");
    },
    eject: async (id) => {
      inserted.delete(id);
      setTimeout(() => deviceEvent({ kind: "disappeared", id }), 300);
      return { ok: true, busy_holder: null, message: "Safe to remove." };
    },
    knownMedia: async () => [],
    profiles: async () => [],
    saveProfile: async (name, preset, targets, redundancy) => ({ id: crypto.randomUUID(), name, preset, targets, playlists: [], redundancy, updated_unix: Date.now() / 1000 }),
    demoAvailable: async () => data.demo_sticks.map((s) => ({ ...s, inserted: inserted.has(`demo:${s.name}`) })),
    demoInsert: async (name) => {
      const id = `demo:${name}`;
      inserted.add(id);
      setTimeout(() => deviceEvent({ kind: "appeared", device: state(id).card.device }), 350);
    },
    demoRemove: async (name) => {
      const id = `demo:${name}`;
      inserted.delete(id);
      deviceEvent({ kind: "disappeared", id });
    },
    demoReset: async () => {
      inserted.clear();
      stage.clear();
      verified.clear();
      labels.clear();
      roles.clear();
    },
    demoSimulateExport: async (id) => {
      stage.set(id, "exported");
      verified.delete(id);
      await sleep(400);
      deviceEvent({ kind: "changed", device: state(id).card.device });
    },
    librarySources: async (targets) => listSources(targets),
    libraryPickFolder: async () => "/Volumes/Crate SSD/Sets 2026",
    libraryAdd: async (path) => {
      const folder = path.replace(/\/+$/, "");
      if (listSources([]).some((s) => s.path === folder)) throw new Error("That folder is already in your library.");
      const volume = /^\/Volumes\/([^/]+)/.exec(folder);
      const cloud = /Google Drive|GoogleDrive|My Drive/.test(folder) ? "Google Drive" : /Dropbox/.test(folder) ? "Dropbox" : null;
      const id = `added-${addedSources.length + 1}`;
      addedSources.push({
        scanned: false,
        source: {
          id,
          name: folder.split("/").pop() || folder,
          path: folder,
          location: cloud ? { kind: "cloud", provider: cloud } : volume ? { kind: "external", name: volume[1] } : { kind: "computer" },
          connected: true,
          scanning: false,
          scanned_unix: null,
          totals: null,
          states: null,
          skipped: 0,
          collected_from: null,
        },
      });
      return id;
    },
    libraryCollectPlan: async (id, targets, filter, query, name, folder) => collectPlan(id, targets, filter, query, name, folder).plan,
    libraryCollect: async (id, targets, filter, query, name, folder) => {
      const { plan, origin, tracks } = collectPlan(id, targets, filter, query, name, folder);
      if (!origin.connected) throw new Error("The source folder can't be reached.");
      if (!plan.tracks) throw new Error("There are no readable tracks to collect.");
      if (plan.exists) throw new Error("The collection couldn't be made: source changed or destination belongs to another collection.");
      libraryCancelled = false;
      for (const phase of ["reading", "copying", "verifying"] as const) {
        for (let done = 0; done <= plan.tracks; done += Math.max(1, Math.ceil(plan.tracks / 4))) {
          if (libraryCancelled) throw new Error("Stopped. Start again with the same name to pick up where it left off.");
          const files = Math.min(done, plan.tracks);
          emit("library-collect-progress", { source_id: id, phase, files_done: files, files_total: plan.tracks, bytes_done: Math.round((plan.bytes * files) / plan.tracks), bytes_total: plan.bytes });
          await sleep(110);
        }
      }
      const made = { id: `collected-${collections.length + 1}`, name: plan.name, origin: id, paths: new Set(tracks.map((t) => t.path)) };
      collections.push(made);
      return made.id;
    },
    libraryCancelCollect: async () => {
      libraryCancelled = true;
    },
    onLibraryCollectProgress: on("library-collect-progress"),
    libraryRemove: async (id) => {
      removedSources.add(id);
    },
    libraryScan: async (id) => {
      const source = listSources([]).find((s) => s.id === id);
      if (!source) throw new Error("That music source was removed.");
      if (!source.connected) throw new Error("The folder couldn't be read: it isn't connected.");
      libraryCancelled = false;
      const total = tracksOf(id, []).length || data.library["unknown_club"].tracks["demo-computer"].length;
      for (let files = 1; files <= total; files += 3) {
        if (libraryCancelled) throw new Error("Scan stopped. The earlier results were kept.");
        emit("library-scan-progress", { source_id: id, files, bytes: files * 9_000_000, cloud_only: 0 });
        await sleep(120);
      }
      const added = addedSources.find((a) => a.source.id === id);
      if (added) added.scanned = true;
    },
    libraryCancelScan: async () => {
      libraryCancelled = true;
    },
    libraryTracks: async (id, targets, filter, query, offset, limit, folder = "") => {
      const q = query.trim().toLowerCase();
      const scoped = (id === "*" ? allSourcesTracks(targets) : tracksOf(id, targets)).filter((t) => inFolder(t.path, folder) && (!q || t.path.toLowerCase().includes(q)));
      const matching = scoped.filter((t) => filter === "all" || t.state === filter);
      return { total: matching.length, tracks: matching.slice(offset, offset + limit), states: countStates(scoped) };
    },
    libraryFolders: async (id) => foldersOf(id),
    libraryPlayUrl: async (id, path) => {
      const t = tracksOf(id, []).find((x) => x.path === path);
      if (!t || t.state === "unreadable") throw new Error("That isn't a track in this source.");
      if (params.get("player") === "unsupported") return "data:audio/x-aiff;base64,AAAA";
      return toneUrl(path);
    },
    onLibraryScanProgress: on("library-scan-progress"),
    onDeviceEvent: on("device-event"),
    onScanProgress: on("scan-progress"),
    onPrepareProgress: on("prepare-progress"),
    onVerifyProgress: on("verify-progress"),
    onCopyProgress: on("copy-progress"),
  };
}
