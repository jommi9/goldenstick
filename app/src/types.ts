// Mirrors of the Rust types the app commands return (see src-tauri/src/dto.rs
// and boothready-core). Field names match serde's output exactly.

export type Evidence = "unknown" | "inferred" | "community" | "vendor" | "lab";
export type LayerStatus = "pass" | "warn" | "fail" | "unknown" | "info";
export type Verdict = "ready" | "expected_to_work" | "partial" | "unknown" | "at_risk" | "fix_needed";
export type Layer = "physical" | "partition" | "filesystem" | "library" | "audio";
export type Confidence = "unknown" | "probable" | "strong" | "exact";
export type LibraryFormat =
  | "rekordbox_device_library"
  | "rekordbox_one_library"
  | "engine_database"
  | "engine_prime_legacy"
  | "serato_library";
export type Role = "main" | "legacy_rescue" | "backup";
export type Filesystem = "fat12" | "fat16" | "fat32" | "exfat" | "ntfs" | "hfs_plus" | "apfs" | "ext" | "unknown";

export interface AppInfo {
  version: string;
  demo: boolean;
  platform: string;
  rules_version: number;
  rules_review_status: string;
  rules_review_note: string | null;
}

export interface Volume {
  os_path: string;
  mount_point: string | null;
  label: string | null;
  filesystem: Filesystem | null;
  size_bytes: number;
  offset_bytes: number | null;
  uuid: string | null;
  efi_system: boolean;
}

export interface PhysicalDevice {
  id: string;
  os_path: string;
  bus: string | { other: string };
  removable: boolean;
  is_system: boolean;
  size_bytes: number;
  logical_sector_size: number;
  storage_vendor: string | null;
  storage_model: string | null;
  usb: { vendor_id: number | null; product_id: number | null; manufacturer: string | null; product: string | null; serial: string | null } | null;
  partition_scheme: string | null;
  volumes: Volume[];
}

export interface Candidate {
  catalog_id: string;
  display_name: string;
  image: string;
  color: string;
}

export interface Identification {
  confidence: Confidence;
  catalog_id: string | null;
  display_name: string;
  manufacturer: string | null;
  marketed_gb: number | null;
  image: string;
  color: string;
  candidates: Candidate[];
  explanation: string;
  confirmed_by_user: boolean;
}

export interface Eligibility {
  eligible: boolean;
  reasons: string[];
}

export interface KnownMedia {
  media_key: string;
  display_name: string;
  confirmed_catalog_id: string | null;
  nickname: string | null;
  role: string | null;
  first_seen_unix: number;
  last_seen_unix: number;
}

export interface DeviceCard {
  device: PhysicalDevice;
  identification: Identification;
  eligibility: Eligibility;
  media_key: string;
  known: KnownMedia | null;
  is_usb: boolean;
}

export interface PlaylistDto {
  id: number;
  parent_id: number;
  name: string;
  is_folder: boolean;
  tracks: number;
  bytes: number;
}

export interface DriveSummary {
  device_id: string;
  identification: Identification;
  eligibility: Eligibility;
  scheme: string;
  filesystem: string | null;
  fs_label: string | null;
  dirty: boolean | null;
  layout_source: "raw" | "os";
  layout_warnings: string[];
  size_bytes: number;
  mount_point: string | null;
  content: { files: number; used_bytes: number; last_modified_unix: number | null };
  formats: LibraryFormat[];
  library_problems: string[];
  playlists: PlaylistDto[];
  library_tracks: number;
  tracks_scanned: number;
  unreadable: number;
  apple_double: number;
  headline: string;
  verified: boolean;
  interrupted: boolean;
  role: string | null;
  connection: string;
}

export interface LayerResult {
  layer: Layer;
  status: LayerStatus;
  evidence: Evidence;
  impact: Verdict;
  headline: string;
  detail: string;
  source: string | null;
}

export type FixKind =
  | { kind: "reformat"; scheme: string; filesystem: Filesystem }
  | { kind: "export_rekordbox"; formats: LibraryFormat[] }
  | { kind: "reexport_rekordbox" }
  | { kind: "export_engine" }
  | { kind: "repair_filesystem" }
  | { kind: "review_tracks"; count: number }
  | { kind: "remove_apple_double"; count: number }
  | { kind: "use_another_usb" };

export type Fix = FixKind & { title: string; detail: string; destructive: boolean };

export interface TrackIssue {
  path: string;
  reason: string;
}

export interface DeviceAssessment {
  device_id: string;
  manufacturer: string;
  model: string;
  legacy: boolean;
  verdict: Verdict;
  summary: string;
  layers: LayerResult[];
  audio: { total: number; supported: number; unsupported: number; unknown: number; unreadable: number; issues: TrackIssue[] };
  notes: string[];
  fixes: Fix[];
}

export interface FormatRecommendation {
  scheme: string;
  filesystem: Filesystem;
  evidence: Evidence;
  covers: string[];
  not_covered: string[];
  why: string;
}

export interface DriveAssessment {
  overall: Verdict;
  headline: string;
  checks: { label: string; status: LayerStatus }[];
  tracks: { scanned: number; compatible_everywhere: number; need_attention: number; unreadable: number };
  devices: DeviceAssessment[];
  fixes: Fix[];
  recommended_format: FormatRecommendation;
}

export interface Preset {
  id: string;
  title: string;
  subtitle: string;
  devices: string[];
  redundancy: boolean;
}

export interface HardwareCard {
  id: string;
  manufacturer: string;
  model: string;
  family: string;
  kind: string;
  released: number;
  legacy: boolean;
}

export interface RolePlan {
  role: Role;
  volume_label: string;
  targets: string[];
  target_models: string[];
  format: FormatRecommendation;
  library_formats: LibraryFormat[];
  content: "full_library" | "essential_playlists";
  audio: "as_is" | "target_safe_only";
  min_capacity_bytes: number;
  ideal_capacity: [number, number];
  purpose: string[];
  why: string;
  assigned_drive: string | null;
  drive_notes: string[];
}

export interface KitPlan {
  roles: RolePlan[];
  not_covered: string[];
  notes: string[];
}

export interface ConfirmationDetails {
  token: string;
  display_name: string;
  image: string;
  color: string;
  size_bytes: number;
  volume_label: string | null;
  serial_tail: string | null;
  files: number;
  used_bytes: number;
  last_modified_unix: number | null;
  eligible: boolean;
  reasons: string[];
}

export interface VerifyProgress {
  bytes_done: number;
  bytes_total: number;
  files_done: number;
  files_total: number;
  current: string;
  eta_secs: number | null;
}

export interface VerifyReport {
  mode: "quick" | "full";
  passed: boolean;
  cancelled: boolean;
  checks: { name: string; passed: boolean; detail: string }[];
  files_checked: number;
  bytes_checked: number;
  failures: { path: string; problem: string }[];
  content_fingerprint: string;
  completed_unix: number;
}

export interface EjectResult {
  ok: boolean;
  busy_holder: string | null;
  message: string;
}

export interface DemoStickInfo {
  name: string;
  title: string;
  size_gb: number;
  inserted: boolean;
  description: string;
}

export interface GigProfile {
  id: string;
  name: string;
  preset: string | null;
  targets: string[];
  playlists: string[];
  redundancy: boolean;
  updated_unix: number;
}

export type DeviceEvent =
  | { kind: "appeared"; device: PhysicalDevice }
  | { kind: "changed"; device: PhysicalDevice }
  | { kind: "disappeared"; id: string };

export interface ExportStatus {
  device_library: number | null;
  one_library: number | null;
  engine: number | null;
}

/** A verified drive the app can copy onto the one being prepared. */
export interface CopySource {
  device_id: string;
  display_name: string;
  role: Role | null;
  files: number;
  bytes: number;
  /** Why the copy can't start, in plain words. Empty when it can. */
  problems: string[];
  /** The destination holds other files, so erasing it first clears the way. */
  needs_erase: boolean;
}

export interface CopyReport {
  files_copied: number;
  bytes_copied: number;
  files_skipped: number;
  files_removed: number;
  cancelled: boolean;
}

export interface TestScenario {
  id: string;
  title: string;
  description: string;
}

export interface TestCheck {
  name: string;
  passed: boolean;
  detail: string;
}

export interface VirtualTestResult {
  id: string;
  title: string;
  description: string;
  passed: boolean;
  duration_ms: number;
  checks: TestCheck[];
  artifact: string | null;
  error: string | null;
}

export interface VirtualTestSuite {
  kind: "virtual";
  passed: boolean;
  root: string;
  scenarios: VirtualTestResult[];
}

export interface TestLabDevice {
  id: string;
  display_name: string;
  model: string | null;
  size_bytes: number;
  is_usb: boolean;
  is_system: boolean;
  mount_point: string | null;
}

export interface RealTestResult {
  kind: "real_read_only";
  read_ok: boolean;
  device: TestLabDevice;
  summary: DriveSummary;
  assessment: DriveAssessment;
  diagnostics_before: string;
  diagnostics_after: string;
  report: string;
}
