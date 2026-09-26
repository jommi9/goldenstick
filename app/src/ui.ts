// Tiny rendering helpers: escaped templates and formatting.

import type { PhysicalDevice, Volume } from "./types";

export class Raw {
  constructor(public readonly s: string) {}
  toString() {
    return this.s;
  }
}

export const raw = (s: string) => new Raw(s);

export function esc(v: unknown): string {
  return String(v ?? "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function part(v: unknown): string {
  if (v instanceof Raw) return v.s;
  if (Array.isArray(v)) return v.map(part).join("");
  if (v === false || v === null || v === undefined) return "";
  return esc(v);
}

/** Tagged template: interpolations are escaped unless wrapped in `raw()`. */
export function html(strings: TemplateStringsArray, ...vals: unknown[]): Raw {
  let out = strings[0];
  vals.forEach((v, i) => {
    out += part(v) + strings[i + 1];
  });
  return new Raw(out);
}

export function gb(bytes: number): string {
  if (bytes >= 1e12) return `${(bytes / 1e12).toFixed(1).replace(/\.0$/, "")} TB`;
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1).replace(/\.0$/, "")} GB`;
  if (bytes >= 1e6) return `${(bytes / 1e6).toFixed(1)} MB`;
  if (bytes >= 1e3) return `${Math.round(bytes / 1e3)} KB`;
  return `${bytes} bytes`;
}

export function num(n: number): string {
  return n.toLocaleString("en-US");
}

export function when(unix: number | null | undefined): string {
  if (!unix) return "unknown";
  const d = new Date(unix * 1000);
  const now = new Date();
  const time = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  if (d.toDateString() === now.toDateString()) return `Today, ${time}`;
  const y = new Date(now);
  y.setDate(now.getDate() - 1);
  if (d.toDateString() === y.toDateString()) return `Yesterday, ${time}`;
  return d.toLocaleDateString([], { day: "numeric", month: "short", year: "numeric" });
}

export function plural(n: number, one: string, many = `${one}s`): string {
  return `${num(n)} ${n === 1 ? one : many}`;
}

/** The volume a DJ player would mount, skipping the EFI partition macOS puts first on GPT drives. */
export function primaryVolume(d: PhysicalDevice): Volume | undefined {
  return d.volumes.find((v) => !v.efi_system) ?? d.volumes[0];
}

export const LIB_LABEL: Record<string, string> = {
  rekordbox_device_library: "rekordbox Device Library",
  rekordbox_one_library: "rekordbox OneLibrary",
  engine_database: "Engine DJ library",
  engine_prime_legacy: "Engine Prime library",
  serato_library: "Serato crates",
};

export const EVIDENCE_LABEL: Record<string, string> = {
  vendor: "Vendor documented",
  lab: "BoothReady lab verified",
  community: "Community verified",
  inferred: "Inferred",
  unknown: "No reliable evidence",
};

export const ROLE_LABEL: Record<string, string> = {
  main: "Main",
  legacy_rescue: "Legacy Rescue",
  backup: "Independent Backup",
};

export const FS_LABEL: Record<string, string> = {
  fat12: "FAT12",
  fat16: "FAT16",
  fat32: "FAT32",
  exfat: "exFAT",
  ntfs: "NTFS",
  hfs_plus: "HFS+",
  apfs: "APFS",
  ext: "ext4",
  unknown: "Unknown",
};
