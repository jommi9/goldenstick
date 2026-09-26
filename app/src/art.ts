// Drawn illustrations. The PRD calls for product photography from a
// licensed catalog (§96); until that exists we draw recognisable shapes
// and never imply a photo is proof of what's inside a drive.

import type { HardwareCard, LayerStatus, Verdict } from "./types";

const shade = (hex: string, amt: number) => {
  const n = parseInt(hex.replace("#", ""), 16);
  const c = (v: number) => Math.max(0, Math.min(255, v + amt));
  const r = c((n >> 16) & 255);
  const g = c((n >> 8) & 255);
  const b = c(n & 255);
  return `#${((r << 16) | (g << 8) | b).toString(16).padStart(6, "0")}`;
};

/** A USB drive illustration for a catalog image key. */
export function usbArt(image: string, color: string, size = 160, label = "USB drive"): string {
  const c = color || "#6b7280";
  const dark = shade(c, -40);
  const metal = "var(--metal)";
  const plug = `<rect x="44" y="6" width="32" height="30" rx="3" fill="${metal}"/><rect x="51" y="14" width="7" height="7" rx="1" fill="var(--bg-2)"/><rect x="62" y="14" width="7" height="7" rx="1" fill="var(--bg-2)"/>`;
  let body = "";
  switch (image) {
    case "stick-nano":
      body = `${plug}<rect x="38" y="34" width="44" height="30" rx="8" fill="${c}"/><rect x="38" y="34" width="44" height="8" rx="4" fill="${dark}"/>`;
      break;
    case "stick-blade":
      body = `${plug}<path d="M36 34 h48 l-6 96 h-36z" fill="${c}"/><rect x="48" y="48" width="24" height="54" rx="4" fill="${dark}" opacity=".5"/>`;
      break;
    case "stick-metal":
      body = `${plug}<rect x="34" y="34" width="52" height="92" rx="10" fill="${c}"/><rect x="34" y="34" width="52" height="92" rx="10" fill="url(#sheen)" opacity=".5"/><circle cx="60" cy="110" r="5" fill="${dark}"/>`;
      break;
    case "stick-cap":
      body = `<rect x="38" y="4" width="44" height="42" rx="8" fill="${dark}"/>${""}<rect x="34" y="40" width="52" height="88" rx="10" fill="${c}"/><rect x="42" y="112" width="36" height="8" rx="4" fill="${dark}"/>`;
      break;
    case "ssd-portable":
      return `<svg role="img" aria-label="${label}" width="${size}" height="${size}" viewBox="0 0 120 140"><defs><linearGradient id="sheen" x1="0" x2="1"><stop offset="0" stop-color="#fff" stop-opacity=".4"/><stop offset="1" stop-color="#fff" stop-opacity="0"/></linearGradient></defs><rect x="22" y="20" width="76" height="104" rx="14" fill="${c}"/><rect x="22" y="20" width="76" height="104" rx="14" fill="url(#sheen)" opacity=".4"/><rect x="50" y="26" width="20" height="5" rx="2.5" fill="${dark}"/><text x="60" y="80" text-anchor="middle" font-size="12" font-weight="700" fill="${shade(c, 60)}">SSD</text></svg>`;
    case "stick-slider":
      body = `${plug}<rect x="34" y="34" width="52" height="92" rx="10" fill="${c}"/><rect x="48" y="48" width="24" height="40" rx="6" fill="${dark}"/><rect x="52" y="54" width="16" height="12" rx="3" fill="${shade(c, 40)}"/>`;
      break;
    default:
      body = `${plug}<rect x="34" y="34" width="52" height="92" rx="10" fill="${c}"/><text x="60" y="88" text-anchor="middle" font-size="14" font-weight="700" fill="${shade(c, 70)}">?</text>`;
  }
  return `<svg role="img" aria-label="${label}" width="${size}" height="${size}" viewBox="0 0 120 140"><defs><linearGradient id="sheen" x1="0" x2="1"><stop offset="0" stop-color="#fff" stop-opacity=".5"/><stop offset=".5" stop-color="#fff" stop-opacity="0"/></linearGradient></defs><g transform="rotate(-18 60 70)">${body}</g></svg>`;
}

/** A front-facing silhouette of DJ hardware, recognisable by shape. */
export function hardwareArt(h: HardwareCard, size = 120): string {
  const label = `${h.manufacturer} ${h.model}`;
  const engine = h.family === "engine";
  const accent = engine ? "#e11d48" : h.legacy ? "#94a3b8" : "#ff7a1a";
  const body = "var(--hw-body)";
  const face = "var(--hw-face)";
  if (h.kind === "software") {
    return `<svg role="img" aria-label="${label}" width="${size}" height="${size * 0.8}" viewBox="0 0 150 120"><rect x="25" y="14" width="100" height="66" rx="6" fill="${body}"/><rect x="31" y="20" width="88" height="54" rx="3" fill="${face}"/><circle cx="55" cy="47" r="14" fill="none" stroke="${accent}" stroke-width="3"/><circle cx="95" cy="47" r="14" fill="none" stroke="${accent}" stroke-width="3"/><path d="M12 86 h126 l-8 12 h-110z" fill="${body}"/></svg>`;
  }
  if (h.kind === "standalone") {
    return `<svg role="img" aria-label="${label}" width="${size}" height="${size * 0.8}" viewBox="0 0 150 120"><rect x="6" y="18" width="138" height="84" rx="8" fill="${body}"/><circle cx="36" cy="72" r="21" fill="${face}"/><circle cx="36" cy="72" r="8" fill="${accent}"/><circle cx="114" cy="72" r="21" fill="${face}"/><circle cx="114" cy="72" r="8" fill="${accent}"/><rect x="58" y="26" width="34" height="22" rx="2" fill="${accent}" opacity=".85"/><rect x="62" y="54" width="4" height="38" rx="2" fill="${face}"/><rect x="73" y="54" width="4" height="38" rx="2" fill="${face}"/><rect x="84" y="54" width="4" height="38" rx="2" fill="${face}"/></svg>`;
  }
  // Player: screen on top, big jog wheel, transport buttons.
  const screenW = h.legacy ? 34 : 56;
  return `<svg role="img" aria-label="${label}" width="${size}" height="${size}" viewBox="0 0 120 120"><rect x="14" y="6" width="92" height="108" rx="8" fill="${body}"/><rect x="${60 - screenW / 2}" y="14" width="${screenW}" height="${h.legacy ? 20 : 28}" rx="2" fill="${accent}" opacity=".9"/><circle cx="60" cy="72" r="28" fill="${face}"/><circle cx="60" cy="72" r="${h.legacy ? 9 : 14}" fill="${h.legacy ? "var(--hw-body)" : accent}" opacity=".85"/><circle cx="26" cy="104" r="5" fill="#16a34a"/><circle cx="26" cy="90" r="5" fill="#f59e0b"/></svg>`;
}

const ICONS: Record<LayerStatus, [string, string]> = {
  pass: ["var(--ok)", '<path d="M7 12.5l3 3 7-7" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"/>'],
  fail: ["var(--bad)", '<path d="M8 8l8 8M16 8l-8 8" stroke="currentColor" stroke-width="2.6" stroke-linecap="round"/>'],
  warn: ["var(--warn)", '<path d="M12 7v6" stroke="currentColor" stroke-width="2.6" stroke-linecap="round"/><circle cx="12" cy="17" r="1.6" fill="currentColor"/>'],
  unknown: ["var(--unk)", '<path d="M9.5 9.5a2.5 2.5 0 1 1 3.4 2.3c-.6.3-.9.8-.9 1.4v.3" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"/><circle cx="12" cy="17" r="1.4" fill="currentColor"/>'],
  info: ["var(--muted)", '<circle cx="12" cy="12" r="2" fill="currentColor"/>'],
};

export const STATUS_WORD: Record<LayerStatus, string> = {
  pass: "OK",
  fail: "Not OK",
  warn: "Warning",
  unknown: "Unknown",
  info: "Not present",
};

/** Status icon with a hidden text label so it never relies on colour alone. */
export function statusIcon(s: LayerStatus, size = 22): string {
  const [color, path] = ICONS[s];
  return `<span class="status-icon" style="color:${color}"><svg aria-hidden="true" width="${size}" height="${size}" viewBox="0 0 24 24"><circle cx="12" cy="12" r="11" fill="currentColor" opacity=".16"/>${path}</svg><span class="sr-only">${STATUS_WORD[s]}</span></span>`;
}

export function verdictStatus(v: Verdict): LayerStatus {
  switch (v) {
    case "ready":
      return "pass";
    case "expected_to_work":
      return "pass";
    case "partial":
      return "warn";
    case "at_risk":
      return "warn";
    case "unknown":
      return "unknown";
    default:
      return "fail";
  }
}

export const VERDICT_LABEL: Record<Verdict, string> = {
  ready: "Ready",
  expected_to_work: "Expected to work",
  partial: "Partly ready",
  unknown: "Not enough data",
  at_risk: "At risk",
  fix_needed: "Fix needed",
};
