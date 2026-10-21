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
  /** Tracks that differ from the export in their audio as well as size (at most 200). */
  mismatched_tracks: string[];
  /** Findings that need no action, such as tags changed after the export. */
  library_notes: string[];
  engine_tracks: number | null;
  serato_crates: number | null;
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
  /** A Device Library export built on this computer; `device_id` is then "export:<name>". */
  export: boolean;
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
  read_issues: string[];
  device: TestLabDevice;
  summary: DriveSummary;
  assessment: DriveAssessment;
  diagnostics_before: string;
  diagnostics_after: string;
  report: string;
}

// ---- music library

export type TrackState = "ready" | "cloud_only" | "blocked" | "unchecked" | "unreadable";

export type LibraryLocation =
  | { kind: "computer" }
  | { kind: "external"; name: string }
  | { kind: "cloud"; provider: string }
  | { kind: "unknown" };

export interface SourceTotals {
  tracks: number;
  bytes: number;
  cloud_only: number;
  cloud_only_bytes: number;
  unreadable: number;
}

export type StateCounts = Record<TrackState, number>;

export interface LibrarySource {
  id: string;
  name: string;
  path: string;
  location: LibraryLocation;
  /** The folder can be read right now. */
  connected: boolean;
  scanning: boolean;
  scanned_unix: number | null;
  totals: SourceTotals | null;
  /** Against the selected players. Null until a scan is saved on this computer. */
  states: StateCounts | null;
  skipped: number;
  /** For a local collection: the folder name of the source its tracks came from. */
  collected_from: string | null;
  /** The last scan read the tags of the tracks on this computer. */
  tags_read: boolean;
}

/** What collecting the shown tracks would copy, and where to. */
export interface CollectPlan {
  tracks: number;
  bytes: number;
  cloud_only: number;
  cloud_only_bytes: number;
  /** Matching tracks left out because they are damaged or not audio. */
  unreadable: number;
  name: string;
  destination: string;
  /** A folder with that name is already there. The same selection resumes it. */
  exists: boolean;
  needed_bytes: number;
  free_bytes: number | null;
}

export interface LibraryCollectProgress {
  source_id: string;
  phase: "reading" | "copying" | "verifying";
  files_done: number;
  files_total: number;
  bytes_done: number;
  bytes_total: number;
}

export interface PlayerIssue {
  device: string;
  model: string;
  reason: string;
}

export interface LibraryTrack {
  /** The id of the source the track is in. */
  source: string;
  path: string;
  name: string;
  folder: string;
  size: number;
  format: string;
  detail: string;
  state: TrackState;
  note: string | null;
  issues: PlayerIssue[];
  /** From the file's tags. The file name stands in when there is no title. */
  title: string | null;
  artist: string | null;
  album: string | null;
  genre: string | null;
  year: number | null;
  bpm: number | null;
  /** Standard notation ("Am") and its Camelot position ("8A"). */
  key: string | null;
  camelot: string | null;
  /** Length in seconds, from the header. */
  duration: number | null;
}

export interface LibraryPage {
  total: number;
  tracks: LibraryTrack[];
  /** States of every track in the folder and search, before the state filter. */
  states: StateCounts;
}

/** A folder inside a source, listed parent first. */
export interface FolderNode {
  path: string;
  name: string;
  depth: number;
  tracks: number;
}

export interface LibraryScanProgress {
  source_id: string;
  files: number;
  bytes: number;
  cloud_only: number;
}

// ---- export to USB

export interface PlannedPlaylist {
  folder: string;
  name: string;
  tracks: number;
}

/** What exporting the ticked folders involves, before anything is written. */
export interface ExportPlan {
  playlists: PlannedPlaylist[];
  /** Every track once, however many playlists hold it. */
  tracks: number;
  bytes: number;
  cloud_only: number;
  cloud_only_bytes: number;
  /** Tracks left out: damaged, not audio, or a format a Device Library can't hold. */
  left_out: number;
  /** Tracks whose analysis is already on this computer from an earlier export. */
  analysed: number;
  to_analyse: number;
  /** Roughly how long the analysis takes, from the tracks still to analyse. */
  estimate_secs: number;
  name: string;
  destination: string;
  /** A folder with that name is already there; an export never goes into one. */
  exists: boolean;
  needed_bytes: number;
  free_bytes: number | null;
}

export interface LibraryExportProgress {
  source_id: string;
  phase: "analyze" | "probe" | "copy" | "database" | "install";
  done: number;
  total: number;
  current: string;
  /** Tracks analysed afresh so far, out of `to_analyse`. */
  analysed: number;
  to_analyse: number;
}

/** A finished Device Library export on this computer. */
export interface LocalExport {
  name: string;
  path: string;
  tracks: number;
  playlists: number;
  analysis_files: number;
  bytes: number;
  created_unix: number;
  /** The name of the source the tracks came from. */
  source: string;
  warnings: string[];
}

/** One track's analysis, as the window draws it. Waveform columns are [low, mid, high], 0 to 127. */
export interface TrackAnalysis {
  tempo_bpm: number;
  key: string;
  camelot: string | null;
  duration_secs: number;
  preview: [number, number, number][];
  overview: [number, number, number][];
  phrases: { start_secs: number; kind: string }[];
  mood: string | null;
}

/** How a library folder and a stick playlist of the same name relate. */
export type CompareState = "same" | "differs" | "library_only" | "stick_only";

export interface PlaylistCompare {
  name: string;
  /** `<source id>|<folder path>`, the tree's folder key, when there is a folder. */
  folder: string | null;
  state: CompareState;
  library_tracks: number;
  stick_tracks: number;
  in_both: number;
  missing_on_stick: string[];
  only_on_stick: string[];
}

export interface StickComparison {
  playlists: PlaylistCompare[];
  orphans: { id: number; title: string; artist: string | null; path: string; playlists: string[] }[];
  stick_tracks: number;
  matched_by_size: number;
  matched_by_name: number;
}

export type SortOp = "add" | "move" | "remove" | "new-folder";

export interface SortOutcome {
  /** Files copied, moved or put back. */
  changed: number;
  can_undo: boolean;
}
