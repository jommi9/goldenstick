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
  Preset,
  Role,
  VerifyProgress,
  VerifyReport,
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
}

type Stage = "initial" | "prepared" | "exported";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export async function createMockApi(): Promise<Api> {
  const data = (await import("./mock-data.json")).default as unknown as Snapshot;
  const params = new URLSearchParams(location.search);
  const inserted = new Set<string>((params.get("insert") ?? "").split(",").filter(Boolean).map((n) => `demo:${n}`));
  const stage = new Map<string, Stage>();
  const verified = new Set<string>();
  const labels = new Map<string, string>();
  const roles = new Map<string, Role | null>();
  let copyCancelled = false;
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
    onDeviceEvent: on("device-event"),
    onScanProgress: on("scan-progress"),
    onPrepareProgress: on("prepare-progress"),
    onVerifyProgress: on("verify-progress"),
    onCopyProgress: on("copy-progress"),
  };
}
