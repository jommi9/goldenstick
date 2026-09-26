// Typed access to the Rust commands. In a plain browser (UI development,
// screenshots) the same interface is served by a mock built from real
// engine output, loaded lazily so the desktop build never fetches it.

import type {
  AppInfo,
  Candidate,
  ConfirmationDetails,
  DemoStickInfo,
  DeviceCard,
  DeviceEvent,
  DriveAssessment,
  DriveSummary,
  EjectResult,
  ExportStatus,
  Filesystem,
  GigProfile,
  HardwareCard,
  Identification,
  KitPlan,
  KnownMedia,
  Preset,
  Role,
  VerifyProgress,
  VerifyReport,
} from "./types";

export interface Api {
  appInfo(): Promise<AppInfo>;
  setDemoMode(enabled: boolean): Promise<void>;
  listDevices(): Promise<DeviceCard[]>;
  analyze(deviceId: string): Promise<DriveSummary>;
  identityCandidates(deviceId: string, query?: string): Promise<Candidate[]>;
  confirmIdentity(deviceId: string, catalogId: string): Promise<Identification>;
  presets(): Promise<Preset[]>;
  hardware(query?: string): Promise<HardwareCard[]>;
  assess(deviceId: string, targets: string[]): Promise<DriveAssessment>;
  plan(targets: string[], redundancy: boolean, deviceId?: string): Promise<KitPlan>;
  confirmation(deviceId: string): Promise<ConfirmationDetails>;
  prepare(deviceId: string, token: string, filesystem: Filesystem, role: Role | null, targets: string[]): Promise<string>;
  removeAppleDouble(deviceId: string): Promise<number>;
  exportStatus(deviceId: string): Promise<ExportStatus>;
  openRekordbox(): Promise<void>;
  verify(deviceId: string, full: boolean): Promise<VerifyReport>;
  cancelVerify(): Promise<void>;
  eject(deviceId: string): Promise<EjectResult>;
  knownMedia(): Promise<KnownMedia[]>;
  profiles(): Promise<GigProfile[]>;
  saveProfile(name: string, preset: string | null, targets: string[], redundancy: boolean): Promise<GigProfile>;
  demoAvailable(): Promise<DemoStickInfo[]>;
  demoInsert(name: string): Promise<void>;
  demoRemove(name: string): Promise<void>;
  demoReset(): Promise<void>;
  demoSimulateExport(deviceId: string, legacySafe: boolean): Promise<void>;
  onDeviceEvent(cb: (e: DeviceEvent) => void): void;
  onScanProgress(cb: (p: { device_id: string; files: number; bytes: number; current: string }) => void): void;
  onPrepareProgress(cb: (p: { step: string; detail: string }) => void): void;
  onVerifyProgress(cb: (p: { device_id: string; progress: VerifyProgress }) => void): void;
}

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

export const inTauri = typeof window !== "undefined" && window.__TAURI_INTERNALS__ !== undefined;

async function tauriApi(): Promise<Api> {
  const { invoke } = await import("@tauri-apps/api/core");
  const { listen } = await import("@tauri-apps/api/event");
  const on = <T>(name: string) => (cb: (p: T) => void) => {
    void listen<T>(name, (e) => cb(e.payload));
  };
  return {
    appInfo: () => invoke("app_info"),
    setDemoMode: (enabled) => invoke("set_demo_mode", { enabled }),
    listDevices: () => invoke("list_devices"),
    analyze: (deviceId) => invoke("analyze", { deviceId }),
    identityCandidates: (deviceId, query) => invoke("identity_candidates", { deviceId, query: query ?? null }),
    confirmIdentity: (deviceId, catalogId) => invoke("confirm_identity", { deviceId, catalogId }),
    presets: () => invoke("presets"),
    hardware: (query) => invoke("hardware", { query: query ?? null }),
    assess: (deviceId, targets) => invoke("assess", { deviceId, targets }),
    plan: (targets, redundancy, deviceId) => invoke("plan", { targets, redundancy, deviceId: deviceId ?? null, essentialGb: null }),
    confirmation: (deviceId) => invoke("confirmation", { deviceId }),
    prepare: (deviceId, token, filesystem, role, targets) => invoke("prepare", { deviceId, token, filesystem, role, targets }),
    removeAppleDouble: (deviceId) => invoke("remove_apple_double", { deviceId }),
    exportStatus: (deviceId) => invoke("export_status", { deviceId }),
    openRekordbox: () => invoke("open_rekordbox"),
    verify: (deviceId, full) => invoke("verify", { deviceId, full }),
    cancelVerify: () => invoke("cancel_verify"),
    eject: (deviceId) => invoke("eject", { deviceId }),
    knownMedia: () => invoke("known_media"),
    profiles: () => invoke("profiles"),
    saveProfile: (name, preset, targets, redundancy) => invoke("save_profile", { name, preset, targets, redundancy }),
    demoAvailable: () => invoke("demo_available"),
    demoInsert: (name) => invoke("demo_insert", { name }),
    demoRemove: (name) => invoke("demo_remove", { name }),
    demoReset: () => invoke("demo_reset"),
    demoSimulateExport: (deviceId, legacySafe) => invoke("demo_simulate_export", { deviceId, legacySafe }),
    onDeviceEvent: on("device-event"),
    onScanProgress: on("scan-progress"),
    onPrepareProgress: on("prepare-progress"),
    onVerifyProgress: on("verify-progress"),
  };
}

export async function createApi(): Promise<Api> {
  if (inTauri) return tauriApi();
  const { createMockApi } = await import("./mock");
  return createMockApi();
}
