// The music library inside the main window: the Collection and Playlists
// sections of the tree, the track list, the track and source inspector, the
// player strip, and the sheets for managing sources, collecting and
// exporting. State lives here; app.ts lays the regions out and routes
// actions to this module.

import type { Api } from "./api";
import type { CollectPlan, ExportPlan, FolderNode, LibraryCollectProgress, LibraryExportProgress, LibraryLocation, LibraryScanProgress, LibrarySource, LibraryTrack, LocalExport, Preset, StateCounts, TrackState } from "./types";
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
  /** The selected players with their model names, in the order the preset lists them. */
  players: () => { id: string; model: string }[];
  /** A stick is selected in the tree, so no library node is. */
  deviceSelected: () => boolean;
  /** Called when a library node is chosen, so the tree has one selection. */
  selectLibrary: () => void;
  /** Redraws the row highlight, the inspector and the status bar after the selection moves. */
  refreshSelection: () => void;
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
  /** The Manage sources sheet is open. */
  manage: false,
  filter: "all" as Filter,
  query: "",
  /** The column the list is ordered by; empty keeps folder order. */
  sort: "",
  desc: false,
  tracks: [] as LibraryTrack[],
  total: 0,
  tracksFor: null as string | null,
  loadingTracks: false,
  /** The selected row in the list, by its index in `tracks`. */
  sel: null as number | null,
  /** The folder selected in the tree, relative to the source. */
  folder: "",
  /** State counts for the current folder and search, from the last page. */
  states: null as StateCounts | null,
  /** Folder trees, fetched when a source is opened in the tree. */
  folders: new Map<string, FolderNode[]>(),
  /** Expanded tree nodes: a source id, or "source id|folder path". */
  open: new Set<string>(),
  /** The collect sheet for one source: its plan, the run and the result. */
  collect: null as null | {
    sourceId: string;
    plan: CollectPlan | null;
    name: string;
    running: boolean;
    progress: LibraryCollectProgress | null;
    error: string | null;
    done: { id: string; name: string; tracks: number } | null;
  },
  /** The Export to USB sheet for one source: ticked folders, the plan, the run and the result. */
  exp: null as null | {
    sourceId: string;
    folders: Set<string>;
    plan: ExportPlan | null;
    planning: boolean;
    name: string;
    running: boolean;
    /** Stop was pressed; the writer stops after the track it is on. */
    stopping: boolean;
    progress: LibraryExportProgress | null;
    error: string | null;
    done: LocalExport | null;
  },
  /** Finished exports on this computer, newest first. */
  exports: [] as LocalExport[],
  exportRemoveAsk: null as string | null,
};

const EXPORT_PHASE: Record<LibraryExportProgress["phase"], string> = {
  analyze: "Analysing",
  probe: "Reading",
  copy: "Copying",
  database: "Writing the database",
  install: "Finishing",
};

/** A rough length in words: "about 6 min", "about 1 h 10 min". */
export function roughTime(secs: number): string {
  if (secs < 45) return "under a minute";
  const min = Math.round(secs / 60);
  if (min < 60) return `about ${min} min`;
  const h = Math.floor(min / 60);
  const m = min % 60;
  return m ? `about ${h} h ${m} min` : `about ${h} h`;
}

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

/** The browser's id for every source at once. */
const ALL = "*";

// ------------------------------------------------------------------ keys

/** Camelot colours: neighbours on the wheel get neighbouring hues, minor keys a deeper shade. */
export function keyColor(camelot: string | null): string | null {
  if (!camelot) return null;
  const n = parseInt(camelot, 10);
  if (!Number.isFinite(n)) return null;
  const major = camelot.endsWith("B");
  return `hsl(${(n * 30 + 150) % 360} ${major ? 62 : 70}% ${major ? 70 : 60}%)`;
}

/** Keys that mix: the same number, or a neighbour on the same ring. */
export function keysMix(a: string | null, b: string | null): boolean {
  if (!a || !b) return false;
  const na = parseInt(a, 10);
  const nb = parseInt(b, 10);
  if (na === nb) return true;
  return a.slice(-1) === b.slice(-1) && ((na % 12) + 1 === nb || (nb % 12) + 1 === na);
}

function keyChip(camelot: string | null, big = false): Raw {
  const c = keyColor(camelot);
  return c ? html`<span class="keychip ${big ? "big" : ""}" style="background:${c}">${camelot}</span>` : html`<span class="keychip none ${big ? "big" : ""}">--</span>`;
}

// ------------------------------------------------------------------ player

/**
 * One audio element for the whole app. It lives outside the rendered markup,
 * so re-rendering the window never interrupts what is playing.
 */
const P = {
  audio: null as HTMLAudioElement | null,
  /** The loaded track, kept as it was listed when it was started. */
  now: null as LibraryTrack | null,
  /** The list it was started from, for previous and next. */
  queue: [] as LibraryTrack[],
  loading: false,
  error: null as string | null,
  /** True while the seek bar is being dragged, so playback doesn't move it. */
  seeking: false,
};

const trackKey = (t: { source: string; path: string }) => `${t.source}|${t.path}`;

function audio(): HTMLAudioElement {
  if (P.audio) return P.audio;
  const a = new Audio();
  a.preload = "auto";
  a.addEventListener("timeupdate", updateClock);
  a.addEventListener("durationchange", updateClock);
  for (const ev of ["play", "pause", "ended"]) a.addEventListener(ev, () => L.host.render());
  a.addEventListener("error", () => {
    if (!P.now || !a.getAttribute("src")) return;
    P.loading = false;
    P.error = `This ${P.now.format || "file"} file can't be played in BoothReady on this computer. That doesn't change whether your players can play it.`;
    L.host.render();
  });
  P.audio = a;
  return a;
}

const clock = (secs: number) => {
  if (!Number.isFinite(secs) || secs < 0) return "--:--";
  const s = Math.floor(secs);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
};

/** Move the clock and the seek bar without re-rendering the window. */
function updateClock() {
  const a = P.audio;
  if (!a) return;
  const elapsed = document.getElementById("player-elapsed");
  const remain = document.getElementById("player-remain");
  const seek = document.getElementById("library-seek") as HTMLInputElement | null;
  if (elapsed) elapsed.textContent = clock(a.currentTime);
  if (remain) remain.textContent = Number.isFinite(a.duration) ? `-${clock(a.duration - a.currentTime)}` : "--:--";
  if (seek && !P.seeking) {
    seek.max = String(Number.isFinite(a.duration) ? a.duration : 0);
    seek.value = String(a.currentTime);
  }
}

async function play(t: LibraryTrack) {
  const a = audio();
  if (P.now && trackKey(P.now) === trackKey(t)) {
    if (a.paused) await a.play().catch(() => {});
    else a.pause();
    return;
  }
  P.now = t;
  P.queue = L.tracks.filter((x) => x.state !== "unreadable");
  P.error = null;
  P.loading = true;
  a.pause();
  L.host.render();
  try {
    const url = await L.host.api.libraryPlayUrl(t.source, t.path);
    // A later click may have started another track while this one resolved.
    if (P.now !== t) return;
    a.src = url;
    await a.play();
  } catch (e) {
    if (P.now !== t) return;
    // The element's own error event explains format failures; this covers the rest.
    if (!P.error && !(e instanceof DOMException && e.name === "AbortError")) P.error = e instanceof DOMException ? `This ${t.format || "file"} file can't be played on this computer.` : String(e);
  } finally {
    if (P.now === t) P.loading = false;
  }
  L.host.render();
}

function step(by: number) {
  if (!P.now) return;
  // Previous restarts the track once it has played a few seconds, as on a deck.
  if (by < 0 && P.audio && P.audio.currentTime > 3) {
    P.audio.currentTime = 0;
    return;
  }
  const at = P.queue.findIndex((t) => trackKey(t) === trackKey(P.now!));
  const next = P.queue[at + by];
  if (next) void play(next);
  else if (by < 0 && P.audio) P.audio.currentTime = 0;
}

function stopPlayer() {
  P.audio?.pause();
  P.audio?.removeAttribute("src");
  P.audio?.load();
  P.now = null;
  P.error = null;
  P.loading = false;
}

function togglePlayer() {
  const a = P.audio;
  if (!P.now || !a) return;
  if (a.paused) void a.play().catch(() => {});
  else a.pause();
}

const isPlaying = (t: LibraryTrack) => !!P.now && trackKey(P.now) === trackKey(t) && !!P.audio && !P.audio.paused;

const ICON = {
  play: "M3 1.5v11l9.5-5.5z",
  pause: "M3 1.5h3v11H3zM8 1.5h3v11H8z",
  prev: "M2 1.5h2v11H2zM13 1.5v11L4.5 7z",
  next: "M12 1.5h-2v11h2zM1 1.5v11L9.5 7z",
  stop: "M2.5 2.5h9v9h-9z",
};
const icon = (d: string, size = 14) => raw(`<svg viewBox="0 0 14 14" width="${size}" height="${size}" aria-hidden="true"><path d="${d}" fill="currentColor"/></svg>`);

/**
 * The player strip across the top of the window. Step 3 of the redesign adds
 * the track's waveform and phrases from the analysis cache; this carries the
 * transport, the clock, the tempo and the key.
 */
export function playerStrip(): Raw {
  const t = P.now;
  if (!t) {
    return html`<section class="player player-empty" aria-label="Player">
      <div class="player-now">
        <div class="player-keys">
          <button class="player-key" disabled aria-label="Previous track">${icon(ICON.prev)}</button>
          <button class="player-key main" disabled aria-label="Play">${icon(ICON.play, 16)}</button>
          <button class="player-key" disabled aria-label="Next track">${icon(ICON.next)}</button>
        </div>
        <div class="player-track"><span class="player-name">Nothing loaded</span><span class="small faint">Double-click a track to load it.</span></div>
      </div>
      <div class="player-time"><span class="player-clock faint">0:00</span><span class="player-seek idle"></span><span class="player-clock faint">--:--</span></div>
      <div class="readout"><div><div class="big num faint">---.-</div><div class="sub">BPM</div></div><div>${keyChip(null, true)}</div></div>
    </section>`;
  }
  const a = P.audio;
  const playing = !!a && !a.paused;
  const s = source(t.source);
  const cloud = t.state === "cloud_only" ? (s?.location.kind === "cloud" ? s.location.provider : "your cloud app") : null;
  const at = P.queue.findIndex((x) => trackKey(x) === trackKey(t));
  const duration = a && Number.isFinite(a.duration) ? a.duration : 0;
  // Rows in "All tracks" already start their folder with the source's name.
  const where = s && !t.folder.startsWith(s.name) ? [s.name, t.folder].filter(Boolean).join(" · ") : t.folder;
  return html`<section class="player" aria-label="Player">
    <div class="player-now">
      <div class="player-keys">
        <button class="player-key" data-act="library-prev" aria-label="Previous track, or the start of this one">${icon(ICON.prev)}</button>
        <button class="player-key main ${playing ? "on" : ""}" data-act="library-toggle-play" aria-label="${playing ? "Pause" : "Play"}" ${P.error ? "disabled" : ""}>${icon(playing ? ICON.pause : ICON.play, 16)}</button>
        <button class="player-key" data-act="library-next" aria-label="Next track" ${at >= 0 && at < P.queue.length - 1 ? "" : "disabled"}>${icon(ICON.next)}</button>
      </div>
      <div class="player-track">
        <b class="player-name" title="${[where, t.name].filter(Boolean).join("/")}">${titleOf(t)}${t.artist ? html`<span class="player-artist"> · ${t.artist}</span>` : ""}</b>
        ${P.error
          ? html`<span class="small player-error" role="alert">${P.error}</span>`
          : P.loading && cloud
            ? html`<span class="small muted">${cloud} is downloading this track so it can play.</span>`
            : P.loading
              ? html`<span class="small muted">Loading.</span>`
              : html`<span class="small faint">${[where, t.detail ? `${t.format}, ${t.detail}` : t.format].filter(Boolean).join(" · ")}</span>`}
      </div>
    </div>
    <div class="player-time">
      <span id="player-elapsed" class="player-clock">${clock(a?.currentTime ?? 0)}</span>
      <input id="library-seek" class="player-seek" type="range" min="0" step="0.1" max="${duration}" value="${a?.currentTime ?? 0}" aria-label="Position" ${duration ? "" : "disabled"}>
      <span id="player-remain" class="player-clock">${duration ? `-${clock(duration - (a?.currentTime ?? 0))}` : "--:--"}</span>
      <button class="player-key" data-act="library-stop-play" aria-label="Stop and unload the track" title="Unload">${icon(ICON.stop, 12)}</button>
    </div>
    <div class="readout" title="${t.key ? `Key: ${t.key}` : "Key not read"}">
      <div><div class="big num">${t.bpm === null ? "---.-" : t.bpm.toFixed(1)}</div><div class="sub">BPM</div></div>
      <div>${keyChip(t.camelot, true)}</div>
    </div>
  </section>`;
}

// ------------------------------------------------------------------ data flow

async function loadFolders(id: string) {
  if (id === ALL || L.folders.has(id)) return;
  try {
    L.folders.set(id, await L.host.api.libraryFolders(id));
  } catch (e) {
    L.error = String(e);
  }
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
  L.exports = await L.host.api.libraryExports().catch(() => []);
  // The open source may have been removed, or a reset may have cleared it.
  if (L.tracksFor && L.tracksFor !== ALL && !source(L.tracksFor)) {
    L.tracksFor = null;
    L.tracks = [];
    L.sel = null;
  }
}

async function loadTracks(id: string, more = false) {
  L.loadingTracks = true;
  try {
    const offset = more && L.tracksFor === id ? L.tracks.length : 0;
    const page = await L.host.api.libraryTracks(id, L.host.targets(), L.filter, L.query, offset, PAGE, L.folder, L.sort, L.desc);
    L.tracks = offset ? [...L.tracks, ...page.tracks] : page.tracks;
    if (!offset) L.sel = null;
    L.total = page.total;
    L.states = page.states;
    L.tracksFor = id;
  } catch (e) {
    L.error = String(e);
  } finally {
    L.loadingTracks = false;
  }
}

/** Progress events can arrive per file, so they redraw at most every 100 ms. */
let renderPending = 0;
function renderSoon() {
  if (renderPending) return;
  renderPending = window.setTimeout(() => {
    renderPending = 0;
    L.host.render();
  }, 100);
}

export async function initLibrary(host: Host) {
  L.host = host;
  host.api.onLibraryScanProgress((p) => {
    L.progress = p;
    renderSoon();
  });
  host.api.onLibraryCollectProgress((p) => {
    if (!L.collect?.running || L.collect.sourceId !== p.source_id) return;
    L.collect.progress = p;
    renderSoon();
  });
  host.api.onLibraryExportProgress((p) => {
    if (!L.exp?.running || L.exp.sourceId !== p.source_id) return;
    L.exp.progress = p;
    renderSoon();
  });
  await refreshLibrary();
  // The window opens on the whole collection when there is one to show.
  if (L.sources.some((s) => s.states)) await openLibrarySource(ALL);
}

/** Space plays and pauses wherever the player is loaded. Returns true when it took the key. */
export function libraryPlayKey(ev: KeyboardEvent): boolean {
  if (ev.key !== " " || !P.now || ev.metaKey || ev.ctrlKey || ev.altKey) return false;
  // Space keeps its usual job in fields and on buttons.
  const el = ev.target as HTMLElement | null;
  if (el?.closest("input, textarea, select, button, summary, [contenteditable]")) return false;
  ev.preventDefault();
  togglePlayer();
  return true;
}

/** Arrow keys move the selection in the list and Return loads it. Returns true when it took the key. */
export function libraryListKey(ev: KeyboardEvent): boolean {
  const el = ev.target as HTMLElement | null;
  if (el?.closest("input, textarea, select, [contenteditable]")) return false;
  if (ev.metaKey || ev.ctrlKey || ev.altKey || !L.tracks.length) return false;
  if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
    const by = ev.key === "ArrowDown" ? 1 : -1;
    L.sel = L.sel === null ? (by > 0 ? 0 : L.tracks.length - 1) : Math.max(0, Math.min(L.tracks.length - 1, L.sel + by));
    ev.preventDefault();
    L.host.render();
    document.querySelector("tr.sel")?.scrollIntoView({ block: "nearest" });
    return true;
  }
  if (ev.key === "Enter" && L.sel !== null && !el?.closest("button, a")) {
    const t = L.tracks[L.sel];
    if (t && t.state !== "unreadable") void play(t);
    ev.preventDefault();
    return true;
  }
  return false;
}

/** Loads the next page when the list is scrolled near its end. */
export function libraryScrolled(el: HTMLElement) {
  if (el.id !== "list-scroller" || L.loadingTracks || !L.tracksFor || L.tracks.length >= L.total) return;
  if (el.scrollTop + el.clientHeight < el.scrollHeight - 200) return;
  const id = L.tracksFor;
  void loadTracks(id, true).then(() => L.host.render());
}

/** Focuses the search box (Cmd+F). */
export function focusSearch() {
  const s = document.getElementById("library-search") as HTMLInputElement | null;
  s?.focus();
  s?.select();
}

async function planCollect(name: string) {
  const c = L.collect;
  if (!c) return;
  try {
    c.plan = await L.host.api.libraryCollectPlan(c.sourceId, L.host.targets(), L.filter, L.query, name, L.folder);
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
    const id = await L.host.api.libraryCollect(c.sourceId, L.host.targets(), L.filter, L.query, c.name, L.folder);
    c.done = { id, name: c.name, tracks: c.plan?.tracks ?? 0 };
  } catch (e) {
    c.error = String(e);
  }
  c.running = false;
  c.progress = null;
  await refreshLibrary();
  L.host.render();
}

async function planExport() {
  const x = L.exp;
  if (!x) return;
  if (!x.folders.size) {
    x.plan = null;
    x.error = null;
    return;
  }
  x.planning = true;
  try {
    x.plan = await L.host.api.libraryExportPlan(x.sourceId, [...x.folders], x.name);
    x.name = x.plan.name;
    x.error = null;
  } catch (e) {
    x.plan = null;
    x.error = String(e);
  }
  x.planning = false;
}

async function runExport() {
  const x = L.exp;
  if (!x || x.running || !x.plan) return;
  x.running = true;
  x.stopping = false;
  x.error = null;
  x.progress = null;
  L.host.render();
  try {
    x.done = await L.host.api.libraryExport(x.sourceId, [...x.folders], x.name);
  } catch (e) {
    x.error = String(e);
  }
  x.running = false;
  x.stopping = false;
  x.progress = null;
  await refreshLibrary();
  L.host.render();
}

export async function openLibrarySource(id: string, folder = "", filter: Filter = "all") {
  L.collect = null;
  L.manage = false;
  if (L.exp && !L.exp.running) L.exp = null;
  L.host.selectLibrary();
  L.folder = folder;
  if (id !== ALL) L.open.add(id);
  await loadFolders(id);
  L.filter = filter;
  L.query = "";
  L.tracks = [];
  L.total = 0;
  L.sel = null;
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
  L.folders.delete(id);
  if (L.open.has(id)) await loadFolders(id);
  if (L.tracksFor === id || L.tracksFor === ALL) await loadTracks(L.tracksFor);
  else if (!L.tracksFor && source(id)?.states) await openLibrarySource(id);
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

/** Closes whichever library sheet is open. Returns false when none is, or the open one can't close yet. */
export function closeLibrarySheet(): boolean {
  if (L.exp) {
    if (L.exp.running) return false;
    L.exp = null;
    return true;
  }
  if (L.collect) {
    if (L.collect.running) return false;
    L.collect = null;
    return true;
  }
  if (L.manage) {
    L.manage = false;
    L.removeAsk = null;
    L.exportRemoveAsk = null;
    return true;
  }
  return false;
}

/** Handle a click. Returns false when the action isn't a Library one. */
export async function libraryAct(a: string, id: string): Promise<boolean> {
  const host = L.host;
  switch (a) {
    case "open-library":
      L.error = null;
      L.removeAsk = null;
      L.manage = true;
      await refreshLibrary();
      host.render();
      return true;
    case "library-open":
      await openLibrarySource(id);
      host.render();
      return true;
    case "library-collection":
      // A Collection entry: every source at once, narrowed to one state.
      await openLibrarySource(ALL, "", id as Filter);
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
      host.render();
      return true;
    }
    case "library-folder": {
      // data-id is "source id|folder path"; an empty path is the whole source.
      const [sourceId, path] = [id.slice(0, id.indexOf("|")), id.slice(id.indexOf("|") + 1)];
      await openLibrarySource(sourceId, path);
      if (path) L.open.add(`${sourceId}|${path}`);
      host.render();
      return true;
    }
    case "library-toggle":
      if (L.open.has(id)) L.open.delete(id);
      else {
        L.open.add(id);
        if (!id.includes("|")) await loadFolders(id);
      }
      host.render();
      return true;
    case "library-filter":
      L.filter = id as Filter;
      if (L.tracksFor) await loadTracks(L.tracksFor);
      if (L.collect && !L.collect.running && !L.collect.done) await planCollect(L.collect.name);
      host.render();
      return true;
    case "library-select":
      L.sel = Number(id);
      host.refreshSelection();
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
    case "library-export-open": {
      const sid = id || (L.tracksFor && L.tracksFor !== ALL ? L.tracksFor : "");
      if (!sid || !source(sid)?.states) {
        host.toast("Pick a playlist or source in the tree to export.");
        return true;
      }
      // The folder shown in the list starts ticked; the whole source has no folder to tick.
      const folders = new Set<string>(L.folder && L.tracksFor === sid ? [L.folder] : []);
      L.exp = { sourceId: sid, folders, plan: null, planning: false, name: "", running: false, stopping: false, progress: null, error: null, done: null };
      L.collect = null;
      await loadFolders(sid);
      await planExport();
      host.render();
      // Bring the ticked folder into view inside the folder list.
      const box = document.querySelector<HTMLElement>('[data-keep-scroll="export-folders"]');
      const ticked = box?.querySelector<HTMLElement>("input:checked")?.closest<HTMLElement>("label");
      if (box && ticked) box.scrollTop += ticked.getBoundingClientRect().top - box.getBoundingClientRect().top - 8;
      return true;
    }
    case "library-export-start":
      await runExport();
      return true;
    case "library-export-stop":
      if (L.exp?.running) {
        L.exp.stopping = true;
        host.render();
      }
      await host.api.libraryCancelExport();
      return true;
    case "library-export-close":
      if (L.exp?.running) return true;
      L.exp = null;
      host.render();
      return true;
    case "library-export-remove-ask":
      L.exportRemoveAsk = id;
      host.render();
      return true;
    case "library-export-remove-keep":
      L.exportRemoveAsk = null;
      host.render();
      return true;
    case "library-export-remove":
      try {
        await host.api.libraryExportRemove(id);
        host.toast(`The export ${id} was removed from this computer.`);
      } catch (e) {
        L.error = String(e);
      }
      L.exportRemoveAsk = null;
      await refreshLibrary();
      host.render();
      return true;
    case "library-sort":
      // The same column again flips the direction; "#" goes back to folder order.
      if (!id) [L.sort, L.desc] = ["", false];
      else if (L.sort === id) L.desc = !L.desc;
      else [L.sort, L.desc] = [id, false];
      if (L.tracksFor) await loadTracks(L.tracksFor);
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
    case "library-play": {
      const t = L.tracks.find((x) => trackKey(x) === id);
      if (t) await play(t);
      return true;
    }
    case "library-play-row": {
      const t = L.tracks[Number(id)];
      if (t && t.state !== "unreadable") await play(t);
      return true;
    }
    case "library-toggle-play":
      togglePlayer();
      return true;
    case "library-prev":
      step(-1);
      return true;
    case "library-next":
      step(1);
      return true;
    case "library-stop-play":
      stopPlayer();
      host.render();
      return true;
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
  if (t.id === "library-seek") {
    P.seeking = true;
    const elapsed = document.getElementById("player-elapsed");
    if (elapsed) elapsed.textContent = clock(Number(t.value));
    return true;
  }
  if (t.id === "library-path") {
    L.path = t.value;
    return true;
  }
  if (t.id === "library-collect-name") {
    if (L.collect) L.collect.name = t.value;
    return true;
  }
  if (t.id === "library-export-name") {
    if (L.exp) L.exp.name = t.value;
    return true;
  }
  if (t.id === "library-search") {
    L.query = t.value;
    clearTimeout(searchTimer);
    searchTimer = window.setTimeout(async () => {
      if (L.tracksFor) await loadTracks(L.tracksFor);
      L.host.render();
    }, 180);
    return true;
  }
  return false;
}

/** Handle a changed field. Returns false when it isn't a Library one. */
export async function libraryChange(t: HTMLInputElement | HTMLSelectElement): Promise<boolean> {
  if (t.id === "library-seek") {
    if (P.audio) P.audio.currentTime = Number(t.value);
    P.seeking = false;
    return true;
  }
  if (t.id === "library-collect-name") {
    // The name decides the folder, so the plan is fetched again for it.
    await planCollect(t.value);
    L.host.render();
    return true;
  }
  if (t.id === "library-export-name") {
    if (L.exp && !L.exp.running) {
      L.exp.name = t.value;
      await planExport();
      L.host.render();
    }
    return true;
  }
  if (t.id.startsWith("library-export-folder-")) {
    const folder = (t as HTMLInputElement).dataset.folder ?? "";
    if (L.exp && !L.exp.running && folder) {
      if ((t as HTMLInputElement).checked) L.exp.folders.add(folder);
      else L.exp.folders.delete(folder);
      L.host.render();
      await planExport();
      L.host.render();
    }
    return true;
  }
  if (!t.id.startsWith("library-preset")) return false;
  L.host.setPreset(t.value);
  await refreshLibrary();
  if (L.tracksFor) await loadTracks(L.tracksFor);
  L.host.render();
  return true;
}

// ------------------------------------------------------------------ tree

const TREE_ICON = {
  all: '<svg class="i" viewBox="0 0 24 24"><path d="M9 18V6l11-2v12"/><circle cx="6" cy="18" r="3"/><circle cx="17" cy="16" r="3"/></svg>',
  folder: '<svg class="i" viewBox="0 0 24 24"><path d="M3 7h6l2 2h10v10H3z"/></svg>',
  smart: '<svg class="i" viewBox="0 0 24 24"><path d="M4 6h16M7 12h10M10 18h4"/></svg>',
  cloud: '<svg class="i" viewBox="0 0 24 24"><path d="M7 18h10a4 4 0 0 0 .5-8 6 6 0 0 0-11.4 1.5A3.3 3.3 0 0 0 7 18z"/></svg>',
  drive: '<svg class="i" viewBox="0 0 24 24"><rect x="3" y="6" width="18" height="12" rx="2"/><path d="M7 14h.01M11 14h6"/></svg>',
  computer: '<svg class="i" viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/></svg>',
};

const caret = (open: boolean) => raw(`<svg viewBox="0 0 10 10" width="9" height="9" aria-hidden="true"><path d="${open ? "M1 3h8L5 8z" : "M3 1v8l5-4z"}" fill="currentColor"/></svg>`);

function sourceIcon(s: LibrarySource): string {
  if (s.location.kind === "cloud") return TREE_ICON.cloud;
  if (s.location.kind === "external") return TREE_ICON.drive;
  return TREE_ICON.computer;
}

/** The Collection and Playlists sections of the tree. */
export function libraryTree(): Raw {
  const libOn = !L.host.deviceSelected();
  const counts: StateCounts = { ready: 0, cloud_only: 0, blocked: 0, unchecked: 0, unreadable: 0 };
  for (const s of L.sources) if (s.states) for (const k of STATE_ORDER) counts[k] += s.states[k];
  const all = L.sources.reduce((n, s) => n + (s.totals?.tracks ?? 0), 0);
  const onAll = libOn && L.tracksFor === ALL;
  const coll = (act: string, id: string, label: string, n: number, ic: string, on: boolean) =>
    html`<div class="node ${on ? "on" : ""}"><span class="caret"></span><button class="label" data-act="${act}" data-id="${id}">${raw(ic)}<span class="nm">${label}</span><span class="ct">${num(n)}</span></button></div>`;
  const node = (key: string, label: string, count: number, depth: number, hasChildren: boolean, on: boolean, ic: string, dim = false) => html`<div class="node ${on ? "on" : ""} ${dim ? "dim" : ""}" style="--depth:${depth}">
      ${hasChildren ? html`<button class="caret" data-act="library-toggle" data-id="${key}" aria-label="${L.open.has(key) ? "Collapse" : "Expand"} ${label}" aria-expanded="${L.open.has(key) ? "true" : "false"}">${caret(L.open.has(key))}</button>` : html`<span class="caret"></span>`}
      <button class="label" data-act="library-folder" data-id="${key.includes("|") ? key : `${key}|`}">${raw(ic)}<span class="nm">${label}</span><span class="ct">${num(count)}</span></button>
    </div>`;
  const sourceNodes = L.sources.map((s) => {
    const folders = L.folders.get(s.id) ?? [];
    const visible = folders.filter((f) => {
      // Shown when every ancestor folder is expanded.
      const parts = f.path.split("/");
      for (let end = 1; end < parts.length; end++) if (!L.open.has(`${s.id}|${parts.slice(0, end).join("/")}`)) return false;
      return true;
    });
    const hasKids = (f: FolderNode) => folders.some((g) => g.path.startsWith(`${f.path}/`));
    const here = libOn && L.tracksFor === s.id;
    return html`${node(s.id, s.name, s.totals?.tracks ?? 0, 0, !!s.states && (folders.length > 0 || !L.folders.has(s.id)), here && !L.folder, sourceIcon(s), !s.connected)}
      ${L.open.has(s.id) ? visible.map((f) => node(`${s.id}|${f.path}`, f.name, f.tracks, f.depth + 1, hasKids(f), here && L.folder === f.path, TREE_ICON.folder)) : ""}`;
  });
  return html`<div class="sec"><span class="lbl">Collection</span></div>
    ${coll("library-open", ALL, "All tracks", all, TREE_ICON.all, onAll && L.filter === "all")}
    ${counts.blocked || (onAll && L.filter === "blocked") ? coll("library-collection", "blocked", STATE_LABEL.blocked, counts.blocked, TREE_ICON.smart, onAll && L.filter === "blocked") : ""}
    ${counts.cloud_only || (onAll && L.filter === "cloud_only") ? coll("library-collection", "cloud_only", STATE_LABEL.cloud_only, counts.cloud_only, TREE_ICON.smart, onAll && L.filter === "cloud_only") : ""}
    ${counts.unreadable || (onAll && L.filter === "unreadable") ? coll("library-collection", "unreadable", STATE_LABEL.unreadable, counts.unreadable, TREE_ICON.smart, onAll && L.filter === "unreadable") : ""}
    <div class="sec"><span class="lbl">Playlists</span><button class="btn link" data-act="open-library" title="Add, scan or remove music folders">Manage</button></div>
    ${L.sources.length ? sourceNodes : html`<div class="hint">No music folders yet. <button class="btn link" data-act="library-pick">Add a folder</button></div>`}`;
}

// ------------------------------------------------------------------ list

const bpmText = (bpm: number) => String(Math.round(bpm * 100) / 100);

/** A track's name as DJ software shows it: the title tag, or the file name. */
const titleOf = (t: LibraryTrack) => t.title ?? t.name;

/** One glyph per selected player: plays, no data, won't play, or can't tell yet. */
function playsOn(t: LibraryTrack): { model: string; g: "ok" | "w" | "b" | "u"; why: string }[] {
  return L.host.players().map((p) => {
    if (t.state === "cloud_only") return { model: p.model, g: "u" as const, why: "not checked while the file is online" };
    if (t.state === "unreadable") return { model: p.model, g: "b" as const, why: t.note ?? "unreadable" };
    const issue = t.issues.find((i) => i.device === p.id || i.model === p.model);
    if (!issue) return { model: p.model, g: "ok" as const, why: "plays" };
    return { model: p.model, g: t.state === "blocked" ? ("b" as const) : ("w" as const), why: issue.reason };
  });
}

function trackRow(t: LibraryTrack, i: number): Raw {
  const kind = t.format.toLowerCase().replace(/[^a-z0-9]/g, "") || "other";
  const n = String(i + 1).padStart(3, "0");
  const loaded = !!P.now && trackKey(P.now) === trackKey(t);
  const playing = isPlaying(t);
  const where = [t.folder, t.name].filter(Boolean).join("/");
  const match = !!P.now && !loaded && keysMix(P.now.camelot, t.camelot);
  const plays = playsOn(t);
  const glyph = { ok: "", w: "w", b: "b", u: "u" };
  return html`<tr class="${t.state} ${loaded ? "playing" : ""} ${L.sel === i ? "sel" : ""}" data-act="library-select" data-id="${i}" data-row="${i}">
    <td class="c-n">${t.state === "unreadable"
      ? html`<span class="idx">${n}</span>`
      : html`<button class="rb-play ${loaded ? "on" : ""}" data-act="library-play" data-id="${trackKey(t)}" aria-label="${playing ? "Pause" : "Play"} ${titleOf(t)}" title="${t.state === "cloud_only" ? "Your cloud app downloads this track to play it." : ""}"><span class="rb-n">${n}</span>${icon(playing ? ICON.pause : ICON.play)}</button>`}</td>
    <td class="c-art"><span class="arttile f-${kind}" aria-hidden="true">${t.format.slice(0, 4)}</span></td>
    <td class="c-wave" title="${t.state === "cloud_only" ? "Not analysed: the file is still online" : "The waveform appears once the analysis is read"}"><span class="base"></span></td>
    <td class="c-title" title="${where}">${titleOf(t)}</td>
    <td class="c-artist">${t.artist ?? (t.title ? "" : html`<span class="faint">${t.folder}</span>`)}</td>
    <td class="r num c-bpm bpm">${t.bpm === null ? html`<span class="faint">·</span>` : bpmText(t.bpm)}</td>
    <td class="c-key ${match ? "match" : ""}" title="${t.key ?? ""}">${t.camelot ? keyChip(t.camelot) : html`<span class="faint">·</span>`}</td>
    <td class="r num dim c-time">${t.duration === null ? html`<span class="faint">·</span>` : clock(t.duration)}</td>
    <td class="c-fmt fmt">${t.format}${t.detail ? html`<small>${t.detail}</small>` : t.state === "cloud_only" ? html`<small>not checked</small>` : ""}</td>
    <td class="c-plays"><span class="plays" title="${plays.map((p) => `${p.model}: ${p.why}`).join("\n")}">${plays.map((p) => html`<i class="${glyph[p.g]}"></i>`)}</span></td>
    <td class="c-state"><span class="trackstate ${t.state}" title="${t.note ?? STATE_HELP[t.state]}"><i class="dot ${t.state}"></i>${STATE_LABEL[t.state]}</span></td>
  </tr>`;
}

/** A column heading that orders the list by that column. */
function sortHead(column: string, label: string, cls: string): Raw {
  const on = L.sort === column;
  const dir = on ? (L.desc ? "descending" : "ascending") : "none";
  return html`<th class="${cls}" aria-sort="${dir}"><button class="sort ${on ? "on" : ""}" data-act="library-sort" data-id="${column}" title="Sort by ${label.toLowerCase()}">${label}${on ? raw(`<svg viewBox="0 0 10 10" width="8" height="8" aria-hidden="true"><path d="${L.desc ? "M1 3h8L5 8z" : "M1 7h8L5 2z"}" fill="currentColor"/></svg>`) : ""}</button></th>`;
}

function trackTable(): Raw {
  if (L.loadingTracks && !L.tracks.length) return html`<div class="empty">Loading tracks.</div>`;
  if (!L.tracks.length) return html`<div class="empty">${L.query || L.filter !== "all" ? "No tracks match." : "No audio files were found in this folder."}</div>`;
  const playersW = Math.max(70, L.host.players().length * 8 + 20);
  return html`<table class="tracks">
      <thead><tr>
        <th class="c-n" style="width:52px"><button class="sort ${L.sort ? "" : "on"}" data-act="library-sort" data-id="" title="Folder order">#</button></th>
        <th class="c-art" style="width:40px"></th>
        <th class="c-wave" style="width:110px">Preview</th>
        ${sortHead("title", "Title", "c-title")}
        ${sortHead("artist", "Artist", "c-artist")}
        ${sortHead("bpm", "BPM", "r c-bpm")}
        ${sortHead("key", "Key", "c-key")}
        ${sortHead("time", "Time", "r c-time")}
        ${sortHead("format", "Format", "c-fmt")}
        <th class="c-plays" style="width:${playersW}px">Plays on</th>
        ${sortHead("state", "State", "c-state")}
      </tr></thead>
      <tbody>${L.tracks.map(trackRow)}</tbody>
    </table>
    <div class="tracks-foot"><span>Showing ${num(L.tracks.length)} of ${num(L.total)}</span>${L.tracks.length < L.total ? html`<button class="btn small" data-act="library-more" ${L.loadingTracks ? "disabled" : ""}>Show more</button>` : ""}</div>`;
}

function playersControl(id: string): Raw {
  const host = L.host;
  const current = host.presetId();
  const count = host.targets().length;
  return html`<select id="${id}" class="select" aria-label="Players the tracks are judged against">
      ${current ? "" : html`<option value="" selected>Your selected players (${count})</option>`}
      ${host.presets().map((p) => html`<option value="${p.id}" ${p.id === current ? "selected" : ""}>${p.title} (${plural(p.devices.length, "player")})</option>`)}
    </select>`;
}

function listTitle(s: LibrarySource | undefined): string {
  if (L.folder) return L.folder.split("/").pop() ?? L.folder;
  if (s) return s.name;
  return L.filter === "all" || L.tracksFor !== ALL ? "All tracks" : STATE_LABEL[L.filter];
}

function totalSecs(): number {
  return L.tracks.reduce((n, t) => n + (t.duration ?? 0), 0);
}

function hoursText(secs: number): string {
  if (!secs) return "";
  const h = Math.floor(secs / 3600);
  const m = Math.round((secs % 3600) / 60);
  return h ? `${h} h ${m} min` : `${m} min`;
}

/** The notices above the list: errors, a running scan, an offline drive, an old scan, a finished collection. */
function notices(s: LibrarySource | undefined, all: boolean, scanning: boolean): Raw {
  const out: Raw[] = [];
  if (L.error) out.push(html`<div class="notice bad" role="alert"><div>${L.error}</div><button class="btn" data-act="library-dismiss-error">Dismiss</button></div>`);
  if (scanning) out.push(html`<div class="notice"><span class="spinner" aria-hidden="true"></span><div>Scanning. ${L.progress?.files ? html`${plural(L.progress.files, "track")} found.` : ""}</div><button class="btn" data-act="library-stop">Stop</button></div>`);
  if (s && !s.connected) out.push(html`<div class="notice warn"><div>This drive isn't connected. You're looking at the scan from ${when(s.scanned_unix)}.</div></div>`);
  const stale = (s ? [s] : all ? L.sources : []).filter((x) => x.states && !x.tags_read);
  if (stale.length && !scanning)
    out.push(html`<div class="notice"><div>${s ? "This folder was scanned" : `${stale.map((x) => x.name).join(", ")} ${stale.length === 1 ? "was" : "were"} scanned`} before BoothReady read tags, so ${s ? "its" : "their"} tracks show file names only. Scan again to see artist, title, BPM and key.</div>${s && s.connected ? html`<button class="btn" data-act="library-scan" data-id="${s.id}">Scan again</button>` : ""}</div>`);
  if (s?.collected_from && !L.exp)
    out.push(html`<div class="notice ok nextstep"><div><b>Export this collection to your USB.</b> BoothReady can build the rekordbox Device Library itself from the folders in this collection, or you can import the folder into rekordbox and export from there with both Device Library and OneLibrary turned on.</div><button class="btn primary" data-act="library-export-open" data-id="${s.id}">Export to USB</button><button class="btn" data-act="open-rekordbox">Open rekordbox</button><button class="btn" data-act="library-copy-path" data-id="${s.id}">Copy folder path</button></div>`);
  return html`<div class="notices">${out}</div>`;
}

/** The centre of the window: the list header, filter chips, notices and the track list. */
export function libraryList(inspectorOpen: boolean): Raw {
  const id = L.tracksFor;
  if (!id) {
    const any = L.sources.length > 0;
    return html`<section class="list" id="list">
      <div class="lhead"><div class="ttl"><h1 id="list-title">${any ? "Library" : "Your music"}</h1></div><div class="spacer"></div>${inspectorButton(inspectorOpen)}</div>
      <div class="chips"></div>
      ${notices(undefined, false, false)}
      <div class="scroller"><div class="empty">${any
        ? html`<h2>Scan a music folder</h2><p>Your folders are listed under Playlists. Scan one to see its tracks and how they stand against your players.</p><div class="actions"><button class="btn primary" data-act="open-library">Manage sources</button></div>`
        : html`<h2>Add the folder your music lives in</h2><p>On this computer, on an external drive or in a synced cloud folder. BoothReady reads it where it is and never changes or moves it.</p><div class="actions"><button class="btn primary" data-act="library-pick" ${L.adding ? "disabled" : ""}>Choose folder…</button><button class="btn" data-act="open-library">Manage sources</button></div>`}</div></div>
    </section>`;
  }
  const all = id === ALL;
  const s = all ? undefined : source(id);
  const c = L.states ?? s?.states ?? null;
  const total = c ? STATE_ORDER.reduce((n, k) => n + c[k], 0) : 0;
  const scanning = !!s && (s.scanning || L.progress?.source_id === s.id);
  const browsable = all || !!s?.states;
  const where = all ? `${plural(L.sources.length, "source")}` : s ? locationLabel(s.location) : "";
  const time = hoursText(totalSecs());
  const meta = [plural(L.total, "track"), L.tracks.length === L.total ? time : "", where].filter(Boolean).join(" · ");
  const canExport = !!s?.states && s.connected && !scanning;
  return html`<section class="list" id="list">
    <div class="lhead">
      <div class="ttl"><h1 id="list-title" title="${s ? `${s.path}${L.folder ? `/${L.folder}` : ""}` : ""}">${listTitle(s)}</h1></div>
      <span class="meta num">${meta}</span>
      <div class="spacer"></div>
      ${browsable ? html`<label class="searchbox"><svg class="i" viewBox="0 0 24 24" style="width:14px;height:14px"><circle cx="11" cy="11" r="6"/><path d="M20 20l-4.5-4.5"/></svg><input id="library-search" type="search" placeholder="Search title, artist, album or folder" value="${L.query}" aria-label="Search tracks" autocomplete="off" spellcheck="false"></label>` : ""}
      ${s && !scanning ? html`<button class="btn" data-act="library-scan" data-id="${s.id}" ${s.connected ? "" : "disabled"} title="Read the folder again">Scan</button>` : ""}
      ${s && s.states && !s.collected_from ? html`<button class="btn" data-act="library-collect-open" data-id="${s.id}" ${s.connected && !scanning && !L.collect ? "" : "disabled"} title="Copy these tracks into a verified folder on this computer">Collect</button>` : ""}
      ${inspectorButton(inspectorOpen)}
      <button class="btn primary" data-act="library-export-open" data-id="${s?.id ?? ""}" ${canExport && !L.exp ? "" : "disabled"} title="${s ? "Build a rekordbox Device Library from these folders (Cmd+E)" : "Pick a source or playlist in the tree to export"}">Export to USB</button>
    </div>
    <div class="chips" role="group" aria-label="Filter tracks">
      ${browsable && c
        ? html`<button class="chip ${L.filter === "all" ? "on" : ""}" data-act="library-filter" data-id="all">All <b>${num(total)}</b></button>
          ${STATE_ORDER.filter((k) => c[k] || L.filter === k).map((k) => html`<button class="chip ${k} ${L.filter === k ? "on" : ""}" data-act="library-filter" data-id="${k}" title="${STATE_HELP[k]}"><i class="dot ${k}"></i>${STATE_LABEL[k]} <b>${num(c[k])}</b></button>`)}`
        : ""}
      <span class="spacer"></span>
      <label class="judge">Judged against ${playersControl("library-preset-list")}</label>
    </div>
    ${notices(s, all, scanning)}
    <div class="scroller" id="list-scroller" data-keep-scroll="list">
      ${browsable
        ? html`<div id="library-tracks">${trackTable()}</div>`
        : html`<div class="empty">${s?.totals ? html`<p>The track list from the last scan isn't saved on this computer.</p>` : html`<p>This folder hasn't been scanned yet.</p>`}${s ? html`<div class="actions"><button class="btn primary" data-act="library-scan" data-id="${s.id}" ${s.connected && !scanning ? "" : "disabled"}>Scan ${s.name}</button></div>` : ""}</div>`}
    </div>
  </section>`;
}

function inspectorButton(open: boolean): Raw {
  return html`<button class="btn icon" data-act="toggle-inspector" aria-pressed="${open}" title="${open ? "Hide" : "Show"} the inspector (Cmd+I)" aria-label="Inspector"><svg class="i" viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M15 4v16"/></svg></button>`;
}

// ------------------------------------------------------------------ inspector

function stateBar(c: StateCounts): Raw {
  const total = STATE_ORDER.reduce((n, k) => n + c[k], 0);
  if (!total) return html``;
  return html`<div class="statebar" role="img" aria-label="${STATE_ORDER.filter((k) => c[k]).map((k) => `${STATE_LABEL[k]}: ${num(c[k])}`).join(", ")}">
      ${STATE_ORDER.filter((k) => c[k]).map((k) => html`<span class="seg ${k}" style="flex-grow:${c[k]}"></span>`)}
    </div>
    <div class="statelegend">${STATE_ORDER.filter((k) => c[k]).map((k) => html`<span title="${STATE_HELP[k]}"><i class="dot ${k}"></i>${STATE_LABEL[k]} <b>${num(c[k])}</b></span>`)}</div>`;
}

function trackInspector(t: LibraryTrack): Raw {
  const s = source(t.source);
  const plays = playsOn(t);
  const bad = plays.filter((p) => p.g === "b" || p.g === "w");
  const loaded = !!P.now && trackKey(P.now) === trackKey(t);
  return html`<div class="hd trk">
      <div class="art" aria-hidden="true">${t.format.slice(0, 4)}</div>
      <div style="min-width:0"><div class="lbl">Track</div><h2>${titleOf(t)}</h2><div class="by">${t.artist ?? ""}</div></div>
    </div>
    <section><span class="lbl">Analysis</span><dl class="kv">
      <dt>BPM</dt><dd class="num">${t.bpm === null ? "not read" : bpmText(t.bpm)}</dd>
      <dt>Key</dt><dd>${t.camelot ? html`${keyChip(t.camelot)} ${t.key ?? ""}` : "not read"}</dd>
      <dt>Length</dt><dd class="num">${t.duration === null ? "-" : clock(t.duration)}</dd>
      ${t.album ? html`<dt>Album</dt><dd>${t.album}</dd>` : ""}
      ${t.genre ? html`<dt>Genre</dt><dd>${t.genre}</dd>` : ""}
      ${t.year ? html`<dt>Year</dt><dd>${t.year}</dd>` : ""}
    </dl></section>
    <section><span class="lbl">File</span><dl class="kv">
      <dt>Format</dt><dd>${t.format}${t.detail ? `, ${t.detail}` : t.state === "cloud_only" ? ", not checked" : ""}</dd>
      <dt>Size</dt><dd class="num">${gb(t.size)}</dd>
      <dt>Where</dt><dd>${s ? locationLabel(s.location) : ""}${t.state === "cloud_only" ? ", not downloaded" : ""}</dd>
      <dt>Folder</dt><dd>${(s && !t.folder.startsWith(s.name) ? [s.name, ...t.folder.split("/")] : t.folder.split("/")).filter(Boolean).join(" / ")}</dd>
      <dt>File</dt><dd class="source-path">${t.name}</dd>
    </dl></section>
    <section class="plays-on"><span class="lbl">Plays on · ${STATE_LABEL[t.state]}</span>
      <div class="hint">${t.note ?? STATE_HELP[t.state]}</div>
      ${bad.length ? html`${bad.map((p) => html`<div class="why ${p.g === "w" ? "w" : ""}"><b>${p.g === "b" ? "Won't play on" : "No data for"} ${p.model}</b><span>${p.why}</span></div>`)}` : ""}
      ${t.state === "ready" || t.state === "blocked" || t.state === "unchecked" ? html`<div class="hint" style="margin-top:8px">${plays.filter((p) => p.g === "ok").length} of ${plural(plays.length, "selected player")} play it.</div>` : ""}
    </section>
    ${t.state === "unreadable" ? "" : html`<section><div class="actions" style="margin-top:0"><button class="btn" data-act="library-play" data-id="${trackKey(t)}">${loaded && isPlaying(t) ? "Pause" : loaded ? "Play" : "Load and play"}</button></div></section>`}`;
}

function sourceInspector(s: LibrarySource): Raw {
  const scanning = s.scanning || L.progress?.source_id === s.id;
  return html`<div class="hd"><div class="lbl">${s.collected_from ? "Local collection" : "Music folder"}</div><h2>${s.name}</h2><div class="by">${locationLabel(s.location)}${s.connected ? "" : ", not connected"}</div></div>
    <section><dl class="kv">
      <dt>Tracks</dt><dd class="num">${s.totals ? `${num(s.totals.tracks)}, ${gb(s.totals.bytes)}` : "not scanned"}</dd>
      <dt>Scanned</dt><dd>${when(s.scanned_unix)}</dd>
      ${s.totals?.cloud_only ? html`<dt>In the cloud</dt><dd class="num">${num(s.totals.cloud_only)}, ${gb(s.totals.cloud_only_bytes)}</dd>` : ""}
      ${s.collected_from ? html`<dt>Copied from</dt><dd>${s.collected_from}</dd>` : ""}
      <dt>Path</dt><dd class="source-path">${s.path}</dd>
    </dl></section>
    ${s.states ? html`<section><span class="lbl">Against the selected players</span>${stateBar(L.tracksFor === s.id && L.states ? L.states : s.states)}</section>` : ""}
    <section><div class="actions" style="margin-top:0">
      ${scanning ? html`<button class="btn" data-act="library-stop">Stop scan</button>` : html`<button class="btn" data-act="library-scan" data-id="${s.id}" ${s.connected ? "" : "disabled"}>Scan again</button>`}
      <button class="btn" data-act="library-copy-path" data-id="${s.id}">Copy path</button>
      <button class="btn link small" data-act="open-library">Manage sources</button>
    </div></section>`;
}

function collectionInspector(): Raw {
  const c = L.states;
  return html`<div class="hd"><div class="lbl">Collection</div><h2>${listTitle(undefined)}</h2><div class="by">${plural(L.sources.length, "music folder")}</div></div>
    ${c ? html`<section><span class="lbl">Against the selected players</span>${stateBar(c)}</section>` : ""}
    <section><div class="hint">Select a track to see its tags, its file and which of your players play it.</div></section>`;
}

/** The inspector for the library's selection: a track, else the source or collection shown. */
export function libraryInspector(): Raw {
  const t = L.sel === null ? undefined : L.tracks[L.sel];
  if (t) return trackInspector(t);
  if (L.tracksFor === ALL) return collectionInspector();
  const s = L.tracksFor ? source(L.tracksFor) : undefined;
  if (s) return sourceInspector(s);
  return html`<div class="empty">Select a playlist or a track.</div>`;
}

/** The library's part of the status bar. */
export function libraryStatus(): Raw {
  if (L.progress) {
    const s = source(L.progress.source_id);
    return html`<span><span class="spinner" aria-hidden="true"></span>Scanning ${s?.name ?? "a folder"}${L.progress.files ? `: ${plural(L.progress.files, "track")}` : ""}</span>`;
  }
  if (L.exp?.running) {
    const g = L.exp.progress;
    const total = g?.total ?? L.exp.plan?.tracks ?? 0;
    const pct = g && total ? Math.min(100, Math.round((100 * g.done) / total)) : 0;
    return html`<span>Building ${L.exp.name} <span class="prog"><i style="width:${pct}%"></i></span> ${g ? `${num(g.done)} of ${num(total)}` : ""}</span>`;
  }
  if (L.collect?.running) return html`<span><span class="spinner" aria-hidden="true"></span>Collecting into ${L.collect.name}</span>`;
  const { sources, tracks } = librarySummary();
  return html`<span><i class="dot ${sources ? "ok" : ""}"></i>${sources ? `Library: ${plural(tracks, "track")} in ${plural(sources, "folder")}` : "No music folders yet"}</span>`;
}

/** The selected row's index in the list, if any. */
export function librarySelectionIndex(): number | null {
  return L.sel;
}

/** The selected track's name, for the status bar. */
export function librarySelection(): string {
  const t = L.sel === null ? undefined : L.tracks[L.sel];
  return t ? `Selected: ${titleOf(t)}` : "";
}

// ------------------------------------------------------------------ sheets

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
        <div class="source-path">${s.path}</div>
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
    ${asking ? html`<div class="banner" style="margin-top:12px"><div>Remove <b>${s.name}</b> from your library? The folder and its files stay exactly where they are.</div><button class="btn" data-act="library-remove" data-id="${s.id}">Remove</button><button class="btn link" data-act="library-remove-keep">Keep</button></div>` : ""}
  </section>`;
}

function manageSheet(): Raw {
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
    <h1>Your music</h1>
    <p class="muted" style="max-width:680px">Add the folders your music lives in: on this computer, on an external drive or in a synced cloud folder. BoothReady reads them where they are and never changes or moves them. Scanning downloads nothing.</p>
    ${L.error ? html`<div class="banner bad" role="alert" style="margin-bottom:12px"><div>${L.error}</div><button class="btn link" data-act="library-dismiss-error">Dismiss</button></div>` : ""}
    ${L.sources.length
      ? html`<div class="library-top">
            <div class="stats library-stats">
              <div class="stat"><b>${num(tracks)}</b><span class="small muted">tracks in ${plural(L.sources.length, "source")}</span></div>
              <div class="stat"><b>${gb(bytes)}</b><span class="small muted">in total</span></div>
              <div class="stat"><b>${num(all.ready)}</b><span class="small muted">ready for the selected players</span></div>
              <div class="stat"><b>${num(all.cloud_only)}</b><span class="small muted">still in the cloud</span></div>
              <div class="stat"><b>${num(attention)}</b><span class="small muted">need attention</span></div>
            </div>
            <div class="library-players"><label class="field-label" for="library-preset">Checked against</label>${playersControl("library-preset")}</div>
          </div>
          ${all.cloud_only ? html`<div class="banner cloud" style="margin-bottom:12px"><div><b>${plural(all.cloud_only, "track")} ${all.cloud_only === 1 ? "is" : "are"} still in the cloud.</b> BoothReady lists them without downloading them, so their format is unknown. To check them, make the folder available offline in your cloud app, then scan again.</div></div>` : ""}
          <div class="stack">${L.sources.map(sourceCard)}</div>
          ${exportsList()}`
      : L.loaded
        ? html`<div class="card"><h2>No music sources yet</h2><p class="muted">Start with the folder you keep your DJ music in. You can add more than one.</p></div>`
        : ""}
    <section class="card library-add" style="margin-top:14px">
      <h2>Add a music folder</h2>
      <p class="small muted">A scan lists audio files and reads the first few kilobytes of each one that is on this computer. Files that are still in the cloud are listed from their name and size only.</p>
      <div class="library-add-row">
        <button class="btn primary" data-act="library-pick" ${L.adding ? "disabled" : ""}>Choose folder…</button>
        <span class="small faint">or</span>
        <input id="library-path" class="search" type="text" placeholder="Paste a folder path" value="${L.path}" aria-label="Folder path" autocomplete="off" spellcheck="false">
        <button class="btn" data-act="library-add" ${L.adding ? "disabled" : ""}>Add</button>
      </div>
    </section>
    ${L.sources.some((s) => s.states) ? html`<div class="actions"><button class="btn primary" data-act="library-open" data-id="*">Browse the collection</button></div>` : ""}
  </div>`;
}

function collectSheet(s: LibrarySource): Raw {
  const c = L.collect!;
  const p = c.plan;
  if (c.done) {
    return html`<div class="collect"><div class="banner ok"><div><b>Collected and verified ${plural(c.done.tracks, "track")}.</b> They are on this computer as ${c.done.name}, checked against the selected players. Open the collection for the next step: exporting it to your USB.</div></div><div class="actions"><button class="btn primary" data-act="library-open" data-id="${c.done.id}">Open the collection</button><button class="btn" data-act="library-collect-close">Close</button></div></div>`;
  }
  if (!p) return html`<div class="collect">${c.error ? html`<div class="banner bad" role="alert"><div>${c.error}</div></div><div class="actions"><button class="btn" data-act="library-collect-close">Close</button></div>` : html`<p class="muted">Working out what to copy.</p>`}</div>`;
  const short = p.free_bytes !== null && p.free_bytes < p.needed_bytes;
  const blocked = short || p.exists || !p.tracks;
  if (c.running) {
    const g = c.progress;
    const within = g && g.bytes_total ? Math.min(1, g.bytes_done / g.bytes_total) : 0;
    const pct = g ? Math.round((PHASE_START[g.phase] + within / 3) * 100) : 0;
    const label = g ? (g.phase === "reading" && p.cloud_only ? "Reading and downloading" : PHASE_LABEL[g.phase]) : "Starting";
    return html`<div class="collect" aria-live="polite">
      <div class="eyebrow">Collecting into ${p.name}</div>
      <h2>${label}</h2>
      <div class="bar" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${pct}"><span style="width:${pct}%"></span></div>
      <div class="collect-progress"><span>${g ? html`${num(g.files_done)} of ${plural(g.files_total, "track")} · ${gb(g.bytes_done)} of ${gb(g.bytes_total)}` : "Reading the folder."}</span><button class="btn" data-act="library-collect-stop">Stop</button></div>
      ${p.cloud_only ? html`<p class="small muted">Your cloud app downloads ${plural(p.cloud_only, "track")} as they are read, so this step depends on your connection.</p>` : ""}
    </div>`;
  }
  return html`<div class="collect">
    <h2>Bring ${plural(p.tracks, "track")} onto this computer</h2>
    <p class="small muted">BoothReady copies the tracks shown for ${s.name} into a folder of their own and checks every copy against the original. The source is left as it is.</p>
    <dl class="facts collect-facts">
      <dt>Tracks</dt><dd>${num(p.tracks)}, ${gb(p.bytes)}</dd>
      ${p.cloud_only ? html`<dt>From the cloud</dt><dd>${num(p.cloud_only)}, ${gb(p.cloud_only_bytes)}. Your cloud app downloads them as they are read.</dd>` : ""}
      ${p.unreadable ? html`<dt>Left out</dt><dd>${plural(p.unreadable, "unreadable file")}</dd>` : ""}
      <dt>Folder</dt><dd class="source-path">${p.destination}</dd>
      <dt>Free space</dt><dd>${p.free_bytes === null ? "unknown" : html`${gb(p.free_bytes)} free, ${gb(p.needed_bytes)} needed`}</dd>
    </dl>
    <label class="field-label" for="library-collect-name">Collection name</label>
    <input id="library-collect-name" class="search" type="text" value="${c.name}" autocomplete="off" spellcheck="false">
    ${c.error ? html`<div class="banner bad" role="alert" style="margin-top:10px"><div>${c.error}</div></div>` : ""}
    ${short ? html`<div class="banner bad" style="margin-top:10px"><div>There isn't enough free space on this computer for these tracks.</div></div>` : ""}
    ${p.exists ? html`<div class="banner warn" style="margin-top:10px"><div>A collection with this name already exists. Choose another name.</div></div>` : ""}
    <div class="actions"><button class="btn primary" data-act="library-collect-start" ${blocked ? "disabled" : ""}>Collect ${plural(p.tracks, "track")}</button><button class="btn" data-act="library-collect-close">Cancel</button></div>
  </div>`;
}

/**
 * Export to USB: tick folders of the source, see what the export involves,
 * build it into a folder of its own on this computer, then copy it onto the
 * one stick the user confirms through the stick's own export step.
 */
function exportSheet(s: LibrarySource): Raw {
  const x = L.exp!;
  if (x.done) {
    const d = x.done;
    const analysed = Math.floor(d.analysis_files / 3);
    return html`<div class="export" aria-live="polite">
      <div class="banner ok"><div><b>Export built: ${plural(d.tracks, "track")} in ${plural(d.playlists, "playlist")}${analysed ? html`, with beat grids and waveforms for ${analysed === d.tracks ? "all of them" : `${num(analysed)} of them`}` : ""}.</b> It is on this computer as <span class="source-path">${d.path}</span>.</div></div>
      ${analysed < d.tracks
        ? html`<div class="banner warn"><div>${analysed ? `${plural(d.tracks - analysed, "track")} couldn't be analysed` : "No track could be analysed"}, so players show no waveform or beat grid for ${analysed ? "them" : "any of them"}. The notes below say why.</div></div>`
        : ""}
      <h2 style="margin-top:16px">Next: put it on a USB</h2>
      <p class="small muted">Plug in a prepared USB and select it under Devices, then open its export step. BoothReady offers to copy this export onto it, asks you to confirm that exact drive, and then reads every file back to check the copy. Nothing has been written to a USB yet.</p>
      ${d.warnings.length ? html`<details class="export-warnings"><summary>${plural(d.warnings.length, "note")}</summary><ul>${d.warnings.map((w) => html`<li>${w}</li>`)}</ul></details>` : ""}
      <div class="actions"><button class="btn primary" data-act="library-export-close">Done</button></div>
    </div>`;
  }
  const p = x.plan;
  if (x.running) {
    const g = x.progress;
    const total = g?.total ?? p?.tracks ?? 0;
    const perTrack = total ? 1 / total : 0;
    // Analysing and copying split each track's share; the database and install come last.
    const within = g ? (g.phase === "copy" ? 0.9 : g.phase === "database" || g.phase === "install" ? 1 : 0.1) : 0;
    const pct = g ? Math.min(100, Math.round(100 * (g.done * perTrack + within * perTrack))) : 0;
    const leftToAnalyse = g ? Math.max(0, g.to_analyse - g.analysed) : (p?.to_analyse ?? 0);
    const eta = g && (g.phase === "analyze" || g.phase === "probe" || g.phase === "copy") ? roughTime(leftToAnalyse * 6 + Math.max(0, total - g.done) * 0.5) : null;
    return html`<div class="export" aria-live="polite">
      <div class="eyebrow">Building ${x.name}</div>
      <h2>${g ? EXPORT_PHASE[g.phase] : "Starting"}${g?.current ? html`<span class="muted export-current">: ${g.current}</span>` : ""}</h2>
      <div class="bar" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${pct}"><span style="width:${pct}%"></span></div>
      <div class="collect-progress"><span>${g ? html`${num(Math.min(g.done + (g.phase === "database" || g.phase === "install" ? 0 : 1), total))} of ${plural(total, "track")}${eta ? html` · ${eta} left` : ""}` : "Reading the folders."}</span>${x.stopping ? html`<span class="muted">Stopping after this track.</span>` : html`<button class="btn" data-act="library-export-stop">Stop</button>`}</div>
      ${p?.cloud_only ? html`<p class="small muted">Your cloud app downloads ${plural(p.cloud_only, "track")} as they are read, so this step depends on your connection.</p>` : ""}
      <p class="small muted">Each track that hasn't been analysed before takes about ${roughTime(6).replace("under a minute", "six seconds")}. Stopping leaves nothing behind.</p>
    </div>`;
  }
  const folders = L.folders.get(s.id) ?? [];
  const short = !!p && p.free_bytes !== null && p.free_bytes < p.needed_bytes;
  const blocked = !p || short || p.exists || !p.tracks || x.planning;
  return html`<div class="export">
    <h2>Build a rekordbox Device Library from ${s.name}</h2>
    <p class="small muted">Tick the folders to export. Each becomes a playlist with every track below it. BoothReady analyses the tracks itself (beat grid, key and waveforms), writes the export into a folder of its own on this computer, and only then offers to copy it onto a USB. It holds a Device Library only, so players that need OneLibrary, such as the CDJ-3000X, won't list it; the check after the copy shows which players will.</p>
    <div class="export-folders" role="group" aria-label="Folders to export" data-keep-scroll="export-folders">
      ${folders.length
        ? folders.map((f, i) => html`<label class="check-row export-folder" style="--depth:${f.depth}"><input id="library-export-folder-${i}" type="checkbox" data-folder="${f.path}" ${x.folders.has(f.path) ? "checked" : ""} ${x.running ? "disabled" : ""}><span class="grow">${f.name}</span><b>${num(f.tracks)}</b></label>`)
        : html`<p class="small muted" style="padding:6px 12px">This source has no folders to export as playlists.</p>`}
    </div>
    ${x.error ? html`<div class="banner bad" role="alert"><div>${x.error}</div></div>` : ""}
    ${p
      ? html`<dl class="facts collect-facts">
          <dt>Playlists</dt><dd>${p.playlists.map((l) => `${l.name} (${num(l.tracks)})`).join(", ")}</dd>
          <dt>Tracks</dt><dd>${num(p.tracks)}, ${gb(p.bytes)}${p.left_out ? html` · <span class="muted">${plural(p.left_out, "file")} left out: damaged, not audio or a format a Device Library can't hold</span>` : ""}</dd>
          ${p.cloud_only ? html`<dt>From the cloud</dt><dd>${num(p.cloud_only)}, ${gb(p.cloud_only_bytes)}. Your cloud app downloads them as they are read.</dd>` : ""}
          <dt>Analysis</dt><dd>${p.to_analyse ? html`${plural(p.to_analyse, "track")} to analyse, ${roughTime(p.estimate_secs)}` : "Nothing to analyse"}${p.analysed ? html` <span class="muted">· ${num(p.analysed)} already analysed on this computer</span>` : ""}</dd>
          <dt>Folder</dt><dd class="source-path">${p.destination}</dd>
          <dt>Free space</dt><dd>${p.free_bytes === null ? "unknown" : html`${gb(p.free_bytes)} free, ${gb(p.needed_bytes)} needed`}</dd>
        </dl>
        <label class="field-label" for="library-export-name">Export name</label>
        <input id="library-export-name" class="search" type="text" value="${x.name}" autocomplete="off" spellcheck="false">
        ${short ? html`<div class="banner bad" style="margin-top:10px"><div>There isn't enough free space on this computer for this export.</div></div>` : ""}
        ${p.exists ? html`<div class="banner warn" style="margin-top:10px"><div>An export with this name is already there. Choose another name; an export is never written into an existing one.</div></div>` : ""}`
      : x.planning
        ? html`<p class="muted">Working out what to export.</p>`
        : x.folders.size
          ? ""
          : html`<p class="muted">Tick at least one folder.</p>`}
    <div class="actions"><button class="btn primary" data-act="library-export-start" ${blocked ? "disabled" : ""}>Build the export${p?.tracks ? ` (${plural(p.tracks, "track")})` : ""}</button><button class="btn" data-act="library-export-close">Cancel</button></div>
  </div>`;
}

/** Finished exports on this computer, for the Manage sources sheet. */
function exportsList(): Raw {
  if (!L.exports.length) return html``;
  return html`<section class="card exports" style="margin-top:14px">
    <h2>Ready to copy onto a USB</h2>
    <p class="small muted">Exports built on this computer. Select a prepared USB under Devices and open its export step, and BoothReady offers to copy one of these onto it.</p>
    <div class="export-list">${L.exports.map((e) => html`<div class="export-row" data-export="${e.name}">
      <div class="grow"><b>${e.name}</b><div class="small muted">${plural(e.tracks, "track")} in ${plural(e.playlists, "playlist")} · ${gb(e.bytes)} · from ${e.source} · built ${when(e.created_unix)}</div><div class="source-path">${e.path}</div></div>
      ${L.exportRemoveAsk === e.name
        ? html`<span class="small">Remove this export from this computer? USBs are not touched.</span><button class="btn danger" data-act="library-export-remove" data-id="${e.name}">Remove</button><button class="btn link" data-act="library-export-remove-keep">Keep</button>`
        : html`<button class="btn link small" data-act="library-export-remove-ask" data-id="${e.name}">Remove</button>`}
    </div>`)}</div>
  </section>`;
}

/** The library sheet that is open, if any: export, collect, or managing sources. */
export function librarySheet(): { crumbs: string[]; body: Raw; narrow: boolean; closable: boolean } | null {
  if (L.exp) {
    const s = source(L.exp.sourceId);
    if (s) return { crumbs: ["Library", s.name, "Export to USB"], body: exportSheet(s), narrow: false, closable: !L.exp.running };
  }
  if (L.collect) {
    const s = source(L.collect.sourceId);
    if (s) return { crumbs: ["Library", s.name, "Collect"], body: collectSheet(s), narrow: true, closable: !L.collect.running };
  }
  if (L.manage) return { crumbs: ["Library", "Music sources"], body: manageSheet(), narrow: false, closable: true };
  return null;
}
