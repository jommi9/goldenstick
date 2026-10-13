// The Library screens: the folders the user's music lives in, what the last
// scan of each found, and how the tracks stand against the selected players.
// State lives here; app.ts routes views and actions to this module.

import type { Api } from "./api";
import type { CollectPlan, LibraryCollectProgress, LibraryLocation, LibraryScanProgress, LibrarySource, LibraryTrack, Preset, StateCounts, TrackState } from "./types";
import { gb, html, num, plural, raw, Raw, when } from "./ui";

type Filter = "all" | TrackState;

const PAGE = 100;

interface Host {
  api: Api;
  render: () => void;
  toast: (msg: string) => void;
  targets: () => string[];
  presets: () => Preset[];
  presetId: () => string | null;
  setPreset: (id: string) => void;
  /** The Library view that is on screen, if any. */
  showing: () => { v: "library" } | { v: "library-source"; id: string } | null;
  openSource: (id: string) => void;
  openLibrary: () => void;
}

const L = {
  host: null as unknown as Host,
  sources: [] as LibrarySource[],
  loaded: false,
  path: "",
  error: null as string | null,
  adding: false,
  progress: null as LibraryScanProgress | null,
  removeAsk: null as string | null,
  filter: "all" as Filter,
  query: "",
  tracks: [] as LibraryTrack[],
  total: 0,
  tracksFor: null as string | null,
  loadingTracks: false,
  /** The collect panel for one source: its plan, the run and the result. */
  collect: null as null | {
    sourceId: string;
    plan: CollectPlan | null;
    name: string;
    running: boolean;
    progress: LibraryCollectProgress | null;
    error: string | null;
    done: { id: string; name: string; tracks: number } | null;
  },
};

const PHASE_LABEL: Record<LibraryCollectProgress["phase"], string> = {
  reading: "Reading the tracks",
  copying: "Copying",
  verifying: "Checking every copy",
};

/** Each phase reads all the bytes once, so the bar gives them equal thirds. */
const PHASE_START: Record<LibraryCollectProgress["phase"], number> = { reading: 0, copying: 1 / 3, verifying: 2 / 3 };

const STATE_ORDER: TrackState[] = ["ready", "cloud_only", "blocked", "unchecked", "unreadable"];

const STATE_LABEL: Record<TrackState, string> = {
  ready: "Ready",
  cloud_only: "In the cloud",
  blocked: "Won't play everywhere",
  unchecked: "No data",
  unreadable: "Unreadable",
};

const STATE_HELP: Record<TrackState, string> = {
  ready: "On this computer and playable on every selected player.",
  cloud_only: "Still online. The format can't be checked until the file is on this computer.",
  blocked: "At least one selected player is documented not to play this file.",
  unchecked: "No selected player rejects it, but at least one has no data for this format.",
  unreadable: "Damaged, or not an audio file.",
};

export function locationLabel(l: LibraryLocation): string {
  switch (l.kind) {
    case "computer":
      return "This computer";
    case "external":
      return `External drive: ${l.name}`;
    case "cloud":
      return `${l.provider} folder`;
    default:
      return "Other storage";
  }
}

const source = (id: string) => L.sources.find((s) => s.id === id);

/** The folder name of a source, for the path bar. */
export function librarySourceName(id: string): string {
  return source(id)?.name ?? "Source";
}

export function librarySummary(): { sources: number; tracks: number } {
  return { sources: L.sources.length, tracks: L.sources.reduce((n, s) => n + (s.totals?.tracks ?? 0), 0) };
}

export async function refreshLibrary() {
  try {
    L.sources = await L.host.api.librarySources(L.host.targets());
    L.loaded = true;
  } catch (e) {
    L.error = String(e);
  }
}

async function loadTracks(id: string, more = false) {
  L.loadingTracks = true;
  try {
    const offset = more && L.tracksFor === id ? L.tracks.length : 0;
    const page = await L.host.api.libraryTracks(id, L.host.targets(), L.filter, L.query, offset, PAGE);
    L.tracks = offset ? [...L.tracks, ...page.tracks] : page.tracks;
    L.total = page.total;
    L.tracksFor = id;
  } catch (e) {
    L.error = String(e);
  } finally {
    L.loadingTracks = false;
  }
}

export async function initLibrary(host: Host) {
  L.host = host;
  host.api.onLibraryScanProgress((p) => {
    L.progress = p;
    if (host.showing()) host.render();
  });
  host.api.onLibraryCollectProgress((p) => {
    if (!L.collect?.running || L.collect.sourceId !== p.source_id) return;
    L.collect.progress = p;
    if (host.showing()) host.render();
  });
  await refreshLibrary();
}

async function planCollect(name: string) {
  const c = L.collect;
  if (!c) return;
  try {
    c.plan = await L.host.api.libraryCollectPlan(c.sourceId, L.host.targets(), L.filter, L.query, name);
    c.name = c.plan.name;
    c.error = null;
  } catch (e) {
    c.error = String(e);
  }
}

async function runCollect() {
  const c = L.collect;
  if (!c || c.running) return;
  c.running = true;
  c.error = null;
  c.progress = null;
  L.host.render();
  try {
    const id = await L.host.api.libraryCollect(c.sourceId, L.host.targets(), L.filter, L.query, c.name);
    c.done = { id, name: c.name, tracks: c.plan?.tracks ?? 0 };
  } catch (e) {
    c.error = String(e);
  }
  c.running = false;
  c.progress = null;
  await refreshLibrary();
  L.host.render();
}

export async function openLibrarySource(id: string) {
  L.collect = null;
  L.filter = "all";
  L.query = "";
  L.tracks = [];
  L.total = 0;
  L.tracksFor = null;
  await loadTracks(id);
}

async function scan(id: string) {
  const s = source(id);
  if (!s) return;
  L.error = null;
  // A non-null progress marks this source as scanning until the call returns.
  L.progress = { source_id: id, files: 0, bytes: 0, cloud_only: 0 };
  L.host.render();
  try {
    await L.host.api.libraryScan(id);
  } catch (e) {
    L.error = String(e);
  }
  L.progress = null;
  await refreshLibrary();
  const view = L.host.showing();
  if (view?.v === "library-source" && view.id === id) await loadTracks(id);
  L.host.render();
}

async function add(path: string) {
  const folder = path.trim();
  if (!folder) {
    L.error = "Choose a folder or type its path first.";
    L.host.render();
    return;
  }
  L.error = null;
  L.adding = true;
  L.host.render();
  let id: string | null = null;
  try {
    id = await L.host.api.libraryAdd(folder);
    L.path = "";
  } catch (e) {
    L.error = String(e);
  }
  L.adding = false;
  await refreshLibrary();
  L.host.render();
  if (id) await scan(id);
}

/** Handle a click. Returns false when the action isn't a Library one. */
export async function libraryAct(a: string, id: string): Promise<boolean> {
  const host = L.host;
  switch (a) {
    case "open-library":
      L.error = null;
      L.removeAsk = null;
      host.openLibrary();
      await refreshLibrary();
      host.render();
      return true;
    case "library-open":
      host.openSource(id);
      await openLibrarySource(id);
      host.render();
      return true;
    case "library-pick": {
      let picked: string | null = null;
      try {
        picked = await host.api.libraryPickFolder();
      } catch (e) {
        L.error = String(e);
        host.render();
      }
      if (picked) await add(picked);
      return true;
    }
    case "library-add":
      await add(L.path);
      return true;
    case "library-scan":
      await scan(id);
      return true;
    case "library-stop":
      await host.api.libraryCancelScan();
      return true;
    case "library-remove-ask":
      L.removeAsk = id;
      host.render();
      return true;
    case "library-remove-keep":
      L.removeAsk = null;
      host.render();
      return true;
    case "library-remove": {
      const name = source(id)?.name ?? "The folder";
      await host.api.libraryRemove(id);
      L.removeAsk = null;
      await refreshLibrary();
      host.toast(`${name} was removed from your library. Its files are untouched.`);
      if (host.showing()?.v === "library-source") host.openLibrary();
      host.render();
      return true;
    }
    case "library-filter":
      L.filter = id as Filter;
      if (L.tracksFor) await loadTracks(L.tracksFor);
      if (L.collect && !L.collect.running && !L.collect.done) await planCollect(L.collect.name);
      host.render();
      return true;
    case "library-collect-open":
      L.collect = { sourceId: id, plan: null, name: "", running: false, progress: null, error: null, done: null };
      await planCollect("");
      host.render();
      return true;
    case "library-collect-start":
      await runCollect();
      return true;
    case "library-collect-stop":
      await host.api.libraryCancelCollect();
      return true;
    case "library-collect-close":
      L.collect = null;
      host.render();
      return true;
    case "library-more":
      if (L.tracksFor) await loadTracks(L.tracksFor, true);
      host.render();
      return true;
    case "library-copy-path": {
      const path = source(id)?.path ?? "";
      try {
        await navigator.clipboard.writeText(path);
        host.toast("Folder path copied.");
      } catch {
        host.toast(path);
      }
      return true;
    }
    case "library-dismiss-error":
      L.error = null;
      host.render();
      return true;
  }
  return false;
}

let searchTimer = 0;

/** Handle typing. Returns false when the field isn't a Library one. */
export function libraryInput(t: HTMLInputElement): boolean {
  if (t.id === "library-path") {
    L.path = t.value;
    return true;
  }
  if (t.id === "library-collect-name") {
    if (L.collect) L.collect.name = t.value;
    return true;
  }
  if (t.id === "library-search") {
    L.query = t.value;
    clearTimeout(searchTimer);
    searchTimer = window.setTimeout(async () => {
      if (L.tracksFor) await loadTracks(L.tracksFor);
      renderTracks();
    }, 180);
    return true;
  }
  return false;
}

/** Handle a changed select. Returns false when it isn't a Library one. */
export async function libraryChange(t: HTMLInputElement | HTMLSelectElement): Promise<boolean> {
  if (t.id === "library-collect-name") {
    // The name decides the folder, so the plan is fetched again for it.
    await planCollect(t.value);
    L.host.render();
    return true;
  }
  if (t.id !== "library-preset") return false;
  L.host.setPreset(t.value);
  await refreshLibrary();
  if (L.tracksFor && L.host.showing()?.v === "library-source") await loadTracks(L.tracksFor);
  L.host.render();
  return true;
}

// ------------------------------------------------------------------ views

function playersControl(): Raw {
  const host = L.host;
  const current = host.presetId();
  const count = host.targets().length;
  return html`<div class="library-players">
    <label class="field-label" for="library-preset">Checked against</label>
    <select id="library-preset" class="select">
      ${current ? "" : html`<option value="" selected>Your selected players (${count})</option>`}
      ${host.presets().map((p) => html`<option value="${p.id}" ${p.id === current ? "selected" : ""}>${p.title} (${plural(p.devices.length, "player")})</option>`)}
    </select>
  </div>`;
}

function stateBar(c: StateCounts): Raw {
  const total = STATE_ORDER.reduce((n, k) => n + c[k], 0);
  if (!total) return html``;
  return html`<div class="statebar" role="img" aria-label="${STATE_ORDER.filter((k) => c[k]).map((k) => `${STATE_LABEL[k]}: ${num(c[k])}`).join(", ")}">
      ${STATE_ORDER.filter((k) => c[k]).map((k) => html`<span class="seg ${k}" style="flex-grow:${c[k]}"></span>`)}
    </div>
    <div class="statelegend">${STATE_ORDER.filter((k) => c[k]).map((k) => html`<span title="${STATE_HELP[k]}"><i class="dot ${k}"></i>${STATE_LABEL[k]} <b>${num(c[k])}</b></span>`)}</div>`;
}

function sourceCard(s: LibrarySource): Raw {
  const t = s.totals;
  const scanning = s.scanning || L.progress?.source_id === s.id;
  const p = L.progress?.source_id === s.id ? L.progress : null;
  const asking = L.removeAsk === s.id;
  return html`<section class="card source" data-source="${s.id}">
    <div class="source-head">
      <div class="grow">
        <h2>${s.name}</h2>
        <div class="source-meta">${s.collected_from ? html`<span class="pill collection">Local collection</span>` : ""}<span class="pill ${s.location.kind}">${locationLabel(s.location)}</span>${s.connected ? "" : html`<span class="pill offline">Not connected</span>`}</div>
        ${s.collected_from ? html`<div class="small muted">A verified copy of tracks from ${s.collected_from}.</div>` : ""}
        <div class="small faint source-path">${s.path}</div>
      </div>
      <div class="source-actions">
        ${t && !scanning ? html`<button class="btn" data-act="library-open" data-id="${s.id}">Open</button>` : ""}
        ${scanning ? html`<button class="btn" data-act="library-stop">Stop</button>` : html`<button class="btn ${t ? "" : "primary"}" data-act="library-scan" data-id="${s.id}" ${s.connected ? "" : "disabled"}>${t ? "Scan again" : "Scan"}</button>`}
        ${scanning ? "" : html`<button class="btn link small" data-act="library-remove-ask" data-id="${s.id}">Remove</button>`}
      </div>
    </div>
    ${scanning
      ? html`<div class="source-scan"><span class="spinner" aria-hidden="true"></span><span>Scanning. ${p && p.files ? html`${plural(p.files, "track")} found${p.cloud_only ? html`, ${num(p.cloud_only)} still in the cloud` : ""}.` : "Reading the folder."}</span></div>`
      : t
        ? html`<div class="source-totals"><b>${plural(t.tracks, "track")}</b> · ${gb(t.bytes)} · scanned ${when(s.scanned_unix)}</div>
            ${s.states ? stateBar(s.states) : html`<p class="small muted">The track list from this scan isn't saved on this computer. Scan again to check it against your players.</p>`}
            ${s.skipped ? html`<p class="small muted">${plural(s.skipped, "folder or file", "folders or files")} couldn't be read and ${s.skipped === 1 ? "was" : "were"} left out.</p>` : ""}
            ${s.connected ? "" : html`<p class="small muted">This is the last scan. Connect the drive to scan it again.</p>`}`
        : html`<p class="small muted">${s.connected ? "Not scanned yet." : "This folder can't be reached right now."}</p>`}
    ${asking ? html`<div class="banner"><div style="flex:1">Remove <b>${s.name}</b> from your library? The folder and its files stay exactly where they are.</div><button class="btn" data-act="library-remove" data-id="${s.id}">Remove</button><button class="btn link" data-act="library-remove-keep">Keep</button></div>` : ""}
  </section>`;
}

function errorBanner(): Raw {
  return L.error ? html`<div class="banner bad" role="alert"><div style="flex:1">${L.error}</div><button class="btn link" data-act="library-dismiss-error">Dismiss</button></div>` : html``;
}

export function libraryView(): Raw {
  const all: StateCounts = { ready: 0, cloud_only: 0, blocked: 0, unchecked: 0, unreadable: 0 };
  let tracks = 0;
  let bytes = 0;
  for (const s of L.sources) {
    tracks += s.totals?.tracks ?? 0;
    bytes += s.totals?.bytes ?? 0;
    if (s.states) for (const k of STATE_ORDER) all[k] += s.states[k];
  }
  const attention = all.blocked + all.unchecked + all.unreadable;
  return html`<div class="library">
    <div class="devtools-heading">
      <div><div class="eyebrow">Music library</div><h1>Your music</h1><p class="muted" style="max-width:640px">Add the folders your music lives in: on this computer, on an external drive or in a synced cloud folder. BoothReady reads them where they are. It never changes, moves or downloads anything.</p></div>
      <button class="btn" data-act="home">Back</button>
    </div>
    ${errorBanner()}
    ${L.sources.length
      ? html`<div class="library-top">
            <div class="stats library-stats">
              <div class="stat"><b>${num(tracks)}</b><span class="small muted">tracks in ${plural(L.sources.length, "source")}</span></div>
              <div class="stat"><b>${gb(bytes)}</b><span class="small muted">in total</span></div>
              <div class="stat"><b>${num(all.ready)}</b><span class="small muted">ready for the selected players</span></div>
              <div class="stat"><b>${num(all.cloud_only)}</b><span class="small muted">still in the cloud</span></div>
              <div class="stat"><b>${num(attention)}</b><span class="small muted">need attention</span></div>
            </div>
            ${playersControl()}
          </div>
          ${all.cloud_only ? html`<div class="banner cloud"><div><b>${plural(all.cloud_only, "track")} ${all.cloud_only === 1 ? "is" : "are"} still in the cloud.</b> BoothReady lists them without downloading them, so their format is unknown. To check them, make the folder available offline in your cloud app, then scan again.</div></div>` : ""}
          <div class="stack">${L.sources.map(sourceCard)}</div>`
      : L.loaded
        ? html`<div class="card library-empty"><h2>No music sources yet</h2><p class="muted">Start with the folder you keep your DJ music in. You can add more than one.</p></div>`
        : ""}
    <section class="card library-add">
      <div class="eyebrow">Add a source</div>
      <h2>Add a music folder</h2>
      <p class="small muted">A scan lists audio files and reads the first few kilobytes of each one that is on this computer. Files that are still in the cloud are listed from their name and size only.</p>
      <div class="library-add-row">
        <button class="btn primary" data-act="library-pick" ${L.adding ? "disabled" : ""}>Choose folder…</button>
        <span class="small faint">or</span>
        <input id="library-path" class="search" type="text" placeholder="Paste a folder path" value="${L.path}" aria-label="Folder path" autocomplete="off" spellcheck="false">
        <button class="btn" data-act="library-add" ${L.adding ? "disabled" : ""}>Add</button>
      </div>
    </section>
  </div>`;
}

function trackRow(t: LibraryTrack, i: number): Raw {
  const shown = t.issues.slice(0, 3);
  const rest = t.issues.length - shown.length;
  const kind = t.format.toLowerCase().replace(/[^a-z0-9]/g, "") || "other";
  return html`<tr class="${t.state}">
    <td class="idx">${String(i + 1).padStart(3, "0")}</td>
    <td class="art"><span class="arttile f-${kind}" aria-hidden="true">${t.format.slice(0, 4)}</span></td>
    <td><b>${t.name}</b>${t.folder ? html`<div class="small faint">${t.folder}</div>` : ""}
      ${shown.length ? html`<ul class="track-issues">${shown.map((i) => html`<li><b>${i.model}:</b> ${i.reason}</li>`)}${rest > 0 ? html`<li>and ${plural(rest, "more player")}</li>` : ""}</ul>` : ""}
      ${t.note ? html`<div class="small track-note">${t.note}</div>` : ""}
    </td>
    <td>${t.format}${t.detail ? html`<div class="small faint">${t.detail}</div>` : t.state === "cloud_only" ? html`<div class="small faint">not checked</div>` : ""}</td>
    <td class="num">${gb(t.size)}</td>
    <td><span class="trackstate ${t.state}" title="${STATE_HELP[t.state]}"><i class="dot ${t.state}"></i>${STATE_LABEL[t.state]}</span></td>
  </tr>`;
}

function trackTable(): Raw {
  if (L.loadingTracks && !L.tracks.length) return html`<p class="muted">Loading tracks.</p>`;
  if (!L.tracks.length) return html`<p class="muted">${L.query || L.filter !== "all" ? "No tracks match." : "No audio files were found in this folder."}</p>`;
  return html`<table class="tracks">
      <thead><tr><th class="idx">#</th><th class="art"></th><th>Track</th><th>Format</th><th class="num">Size</th><th>Status</th></tr></thead>
      <tbody>${L.tracks.map(trackRow)}</tbody>
    </table>
    <div class="tracks-foot"><span class="small muted">Showing ${num(L.tracks.length)} of ${num(L.total)}</span>${L.tracks.length < L.total ? html`<button class="btn" data-act="library-more" ${L.loadingTracks ? "disabled" : ""}>Show more</button>` : ""}</div>`;
}

function collectPanel(s: LibrarySource): Raw {
  const c = L.collect;
  if (!c || c.sourceId !== s.id) return html``;
  const p = c.plan;
  if (c.done) {
    return html`<section class="card collect"><div class="banner ok"><div style="flex:1"><b>Collected and verified ${plural(c.done.tracks, "track")}.</b> They are on this computer as ${c.done.name}, checked against the selected players. Open the collection for the next step: exporting it to your USB.</div><button class="btn primary" data-act="library-open" data-id="${c.done.id}">Open the collection</button><button class="btn link" data-act="library-collect-close">Close</button></div></section>`;
  }
  if (!p) return html`<section class="card collect">${c.error ? html`<div class="banner bad" role="alert"><div style="flex:1">${c.error}</div><button class="btn link" data-act="library-collect-close">Close</button></div>` : html`<p class="muted">Working out what to copy.</p>`}</section>`;
  const short = p.free_bytes !== null && p.free_bytes < p.needed_bytes;
  const blocked = short || p.exists || !p.tracks;
  if (c.running) {
    const g = c.progress;
    const within = g && g.bytes_total ? Math.min(1, g.bytes_done / g.bytes_total) : 0;
    const pct = g ? Math.round((PHASE_START[g.phase] + within / 3) * 100) : 0;
    const label = g ? (g.phase === "reading" && p.cloud_only ? "Reading and downloading" : PHASE_LABEL[g.phase]) : "Starting";
    return html`<section class="card collect" aria-live="polite">
      <div class="eyebrow">Collecting into ${p.name}</div>
      <h2>${label}</h2>
      <div class="bar" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${pct}"><span style="width:${pct}%"></span></div>
      <div class="collect-progress"><span>${g ? html`${num(g.files_done)} of ${plural(g.files_total, "track")} · ${gb(g.bytes_done)} of ${gb(g.bytes_total)}` : "Reading the folder."}</span><button class="btn" data-act="library-collect-stop">Stop</button></div>
      ${p.cloud_only ? html`<p class="small muted">Your cloud app downloads ${plural(p.cloud_only, "track")} as they are read, so this step depends on your connection.</p>` : ""}
    </section>`;
  }
  return html`<section class="card collect">
    <div class="eyebrow">Collect</div>
    <h2>Bring ${plural(p.tracks, "track")} onto this computer</h2>
    <p class="small muted">BoothReady copies them into a folder of their own and checks every copy against the original. The source is left as it is.</p>
    <dl class="facts collect-facts">
      <dt>Tracks</dt><dd>${num(p.tracks)}, ${gb(p.bytes)}</dd>
      ${p.cloud_only ? html`<dt>From the cloud</dt><dd>${num(p.cloud_only)}, ${gb(p.cloud_only_bytes)}. Your cloud app downloads them as they are read.</dd>` : ""}
      ${p.unreadable ? html`<dt>Left out</dt><dd>${plural(p.unreadable, "unreadable file")}</dd>` : ""}
      <dt>Folder</dt><dd class="source-path">${p.destination}</dd>
      <dt>Free space</dt><dd>${p.free_bytes === null ? "unknown" : html`${gb(p.free_bytes)} free, ${gb(p.needed_bytes)} needed`}</dd>
    </dl>
    <label class="field-label" for="library-collect-name">Collection name</label>
    <input id="library-collect-name" class="search" type="text" value="${c.name}" autocomplete="off" spellcheck="false">
    ${c.error ? html`<div class="banner bad" role="alert"><div>${c.error}</div></div>` : ""}
    ${short ? html`<div class="banner bad"><div>There isn't enough free space on this computer for these tracks.</div></div>` : ""}
    ${p.exists ? html`<div class="banner warn"><div>A collection with this name already exists. Choose another name.</div></div>` : ""}
    <div class="actions"><button class="btn primary" data-act="library-collect-start" ${blocked ? "disabled" : ""}>Collect ${plural(p.tracks, "track")}</button><button class="btn" data-act="library-collect-close">Cancel</button></div>
  </section>`;
}

/**
 * What to do with a finished collection. BoothReady does not write DJ
 * libraries, so the export is done in rekordbox and checked back here.
 */
function nextStep(s: LibrarySource): Raw {
  return html`<section class="card nextstep">
    <div class="eyebrow">Next step</div>
    <h2>Export this collection to your USB with rekordbox</h2>
    <ol class="nextsteps">
      <li>Open rekordbox and choose File, Import, Import Folder, then pick this folder:<div class="source-path">${s.path}</div></li>
      <li>Connect your USB and export the tracks or playlists to it, with both Device Library and OneLibrary turned on.</li>
      <li>Come back to the USB tab. BoothReady checks the drive and shows where it will play.</li>
    </ol>
    <div class="actions"><button class="btn primary" data-act="open-rekordbox">Open rekordbox</button><button class="btn" data-act="library-copy-path" data-id="${s.id}">Copy folder path</button><button class="btn" data-act="home">Go to the USB tab</button></div>
  </section>`;
}

/** Re-render only the track list, so typing in the search box keeps focus. */
function renderTracks() {
  const el = document.getElementById("library-tracks");
  if (el) el.innerHTML = trackTable().s;
  else L.host.render();
}

export function librarySourceView(id: string): Raw {
  const s = source(id);
  if (!s) return html`<div class="library"><p class="muted">That music source was removed.</p><button class="btn" data-act="open-library">Back to your music</button></div>`;
  const c = s.states;
  const total = c ? STATE_ORDER.reduce((n, k) => n + c[k], 0) : 0;
  const scanning = s.scanning || L.progress?.source_id === s.id;
  return html`<div class="library">
    <div class="devtools-heading">
      <div><div class="eyebrow">${locationLabel(s.location)}${s.connected ? "" : " · not connected"}</div><h1>${s.name}</h1><div class="small faint source-path">${s.path}</div></div>
      <div class="source-actions"><button class="btn" data-act="open-library">Back to your music</button>${scanning ? html`<button class="btn" data-act="library-stop">Stop</button>` : html`<button class="btn" data-act="library-scan" data-id="${s.id}" ${s.connected ? "" : "disabled"}>Scan again</button>`}${c && !s.collected_from && !L.collect ? html`<button class="btn primary" data-act="library-collect-open" data-id="${s.id}" ${s.connected && !scanning ? "" : "disabled"}>Collect</button>` : ""}</div>
    </div>
    ${errorBanner()}
    ${scanning ? html`<div class="banner"><span class="spinner" aria-hidden="true"></span><div>Scanning. ${L.progress?.files ? html`${plural(L.progress.files, "track")} found.` : ""}</div></div>` : ""}
    ${s.connected ? "" : html`<div class="banner"><div>This drive isn't connected. You're looking at the scan from ${when(s.scanned_unix)}.</div></div>`}
    <div class="library-top">
      <div class="small muted">${s.totals ? html`<b>${plural(s.totals.tracks, "track")}</b> · ${gb(s.totals.bytes)} · scanned ${when(s.scanned_unix)}` : "Not scanned yet."}</div>
      ${playersControl()}
    </div>
    ${c ? stateBar(c) : ""}
    ${collectPanel(s)}
    ${s.collected_from ? nextStep(s) : ""}
    ${c
      ? html`<div class="browse"><div class="library-filters" role="group" aria-label="Filter tracks">
            <button class="chip ${L.filter === "all" ? "on" : ""}" data-act="library-filter" data-id="all">All <b>${num(total)}</b></button>
            ${STATE_ORDER.filter((k) => c[k] || L.filter === k).map((k) => html`<button class="chip ${k} ${L.filter === k ? "on" : ""}" data-act="library-filter" data-id="${k}" title="${STATE_HELP[k]}"><i class="dot ${k}"></i>${STATE_LABEL[k]} <b>${num(c[k])}</b></button>`)}
          </div>
          <div class="browse-main"><input id="library-search" class="search" type="search" placeholder="Search by track or folder name" value="${L.query}" aria-label="Search tracks" autocomplete="off" spellcheck="false">
          <div id="library-tracks" class="card tracks-card">${trackTable()}</div></div></div>`
      : html`<p class="muted">${s.totals ? "The track list from the last scan isn't saved on this computer. Scan again to see it." : "Scan this folder to see its tracks."}</p>`}
  </div>`;
}

/** The Library entry on the home screen. */
export function libraryHomeRow(): Raw {
  const { sources, tracks } = librarySummary();
  return html`<section><div class="eyebrow">2 · Build one from your music</div><div class="devlist"><button class="devrow" data-act="open-library">
    <div class="grow"><b>${sources ? "Music library" : "Set up your music library"}</b><div class="small muted">${sources ? `${plural(sources, "source")}, ${plural(tracks, "track")}` : "Add the folders your music lives in and see what will play where."}</div></div>
    <span class="pill">${raw(sources ? "Open" : "Add music")}</span>
  </button></div></section>`;
}
