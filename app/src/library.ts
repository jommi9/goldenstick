// The Library screens: the folders the user's music lives in, what the last
// scan of each found, and how the tracks stand against the selected players.
// State lives here; app.ts routes views and actions to this module.

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
  /** The column the list is ordered by; empty keeps folder order. */
  sort: "",
  desc: false,
  tracks: [] as LibraryTrack[],
  total: 0,
  tracksFor: null as string | null,
  loadingTracks: false,
  /** The folder selected in the browser's tree, relative to the source. */
  folder: "",
  /** State counts for the current folder and search, from the last page. */
  states: null as StateCounts | null,
  /** Folder trees, fetched when a source is opened in the tree. */
  folders: new Map<string, FolderNode[]>(),
  /** Expanded tree nodes: a source id, or "source id|folder path". */
  open: new Set<string>(),
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
  /** The Export to USB panel for one source: ticked folders, the plan, the run and the result. */
  exp: null as null | {
    sourceId: string;
    folders: Set<string>;
    plan: ExportPlan | null;
    planning: boolean;
    name: string;
    running: boolean;
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

// ------------------------------------------------------------------ player

/**
 * One audio element for the whole app. It lives outside the rendered markup,
 * so re-rendering a screen never interrupts what is playing.
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

/** Move the clock and the seek bar without re-rendering the screen. */
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
const icon = (d: string) => raw(`<svg viewBox="0 0 14 14" width="14" height="14" aria-hidden="true"><path d="${d}" fill="currentColor"/></svg>`);

/** The bar along the bottom of the window while a track is loaded. */
export function libraryPlayerBar(): Raw {
  const t = P.now;
  if (!t) return html``;
  const a = P.audio;
  const playing = !!a && !a.paused;
  const s = source(t.source);
  const cloud = t.state === "cloud_only" ? (s?.location.kind === "cloud" ? s.location.provider : "your cloud app") : null;
  const at = P.queue.findIndex((x) => trackKey(x) === trackKey(t));
  const duration = a && Number.isFinite(a.duration) ? a.duration : 0;
  // Rows in "All tracks" already start their folder with the source's name.
  const where = s && !t.folder.startsWith(s.name) ? [s.name, t.folder].filter(Boolean).join(" · ") : t.folder;
  const facts = [t.bpm !== null ? `${bpmText(t.bpm)} BPM` : "", t.camelot ? `${t.camelot} (${t.key})` : "", t.detail ? `${t.format}, ${t.detail}` : t.format].filter(Boolean).join(" · ");
  return html`<section class="player" aria-label="Player">
    <div class="player-keys">
      <button class="player-key" data-act="library-prev" aria-label="Previous track, or the start of this one">${icon(ICON.prev)}</button>
      <button class="player-key main ${playing ? "on" : ""}" data-act="library-toggle-play" aria-label="${playing ? "Pause" : "Play"}" ${P.error ? "disabled" : ""}>${icon(playing ? ICON.pause : ICON.play)}</button>
      <button class="player-key" data-act="library-next" aria-label="Next track" ${at >= 0 && at < P.queue.length - 1 ? "" : "disabled"}>${icon(ICON.next)}</button>
    </div>
    <div class="player-track">
      <b class="player-name">${titleOf(t)}${t.artist ? html`<span class="player-artist"> · ${t.artist}</span>` : ""}</b>
      <div class="small faint">${[where, facts].filter(Boolean).join(" · ")}</div>
      ${P.error
        ? html`<div class="small player-error" role="alert">${P.error}</div>`
        : P.loading && cloud
          ? html`<div class="small muted">${cloud} is downloading this track so it can play.</div>`
          : P.loading
            ? html`<div class="small muted">Loading.</div>`
            : ""}
    </div>
    <div class="player-time">
      <span id="player-elapsed" class="player-clock">${clock(a?.currentTime ?? 0)}</span>
      <input id="library-seek" class="player-seek" type="range" min="0" step="0.1" max="${duration}" value="${a?.currentTime ?? 0}" aria-label="Position" ${duration ? "" : "disabled"}>
      <span id="player-remain" class="player-clock">${duration ? `-${clock(duration - (a?.currentTime ?? 0))}` : "--:--"}</span>
    </div>
    <button class="player-key" data-act="library-stop-play" aria-label="Stop and close the player">${icon(ICON.stop)}</button>
  </section>`;
}

/** The folder name of a source, for the path bar. */
export function librarySourceName(id: string): string {
  if (id === ALL) return "Collection";
  return source(id)?.name ?? "Source";
}

/** The browser's id for every source at once. */
const ALL = "*";

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
}

async function loadTracks(id: string, more = false) {
  L.loadingTracks = true;
  try {
    const offset = more && L.tracksFor === id ? L.tracks.length : 0;
    const page = await L.host.api.libraryTracks(id, L.host.targets(), L.filter, L.query, offset, PAGE, L.folder, L.sort, L.desc);
    L.tracks = offset ? [...L.tracks, ...page.tracks] : page.tracks;
    L.total = page.total;
    L.states = page.states;
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
  host.api.onLibraryExportProgress((p) => {
    if (!L.exp?.running || L.exp.sourceId !== p.source_id) return;
    L.exp.progress = p;
    if (host.showing()) host.render();
  });
  document.addEventListener("keydown", (ev) => {
    if (ev.key !== " " || !P.now || ev.metaKey || ev.ctrlKey || ev.altKey) return;
    // Space keeps its usual job in fields and on buttons.
    const el = ev.target as HTMLElement | null;
    if (el?.closest("input, textarea, select, button, summary, [contenteditable]")) return;
    ev.preventDefault();
    togglePlayer();
  });
  await refreshLibrary();
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
  x.error = null;
  x.progress = null;
  L.host.render();
  try {
    x.done = await L.host.api.libraryExport(x.sourceId, [...x.folders], x.name);
  } catch (e) {
    x.error = String(e);
  }
  x.running = false;
  x.progress = null;
  await refreshLibrary();
  L.host.render();
}

export async function openLibrarySource(id: string, folder = "") {
  L.collect = null;
  if (L.exp && !L.exp.running) L.exp = null;
  L.folder = folder;
  L.open.add(id);
  await loadFolders(id);
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
    case "library-folder": {
      // data-id is "source id|folder path"; an empty path is the whole source.
      const [sourceId, path] = [id.slice(0, id.indexOf("|")), id.slice(id.indexOf("|") + 1)];
      host.openSource(sourceId);
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
      // The folder shown in the browser starts ticked; the whole source has no folder to tick.
      const folders = new Set<string>(L.folder ? [L.folder] : []);
      L.exp = { sourceId: id, folders, plan: null, planning: false, name: "", running: false, progress: null, error: null, done: null };
      L.collect = null;
      await loadFolders(id);
      await planExport();
      host.render();
      return true;
    }
    case "library-export-start":
      await runExport();
      return true;
    case "library-export-stop":
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
      renderTracks();
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
      renderTracks();
    }, 180);
    return true;
  }
  return false;
}

/** Handle a changed select. Returns false when it isn't a Library one. */
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
      <div><div class="eyebrow">Music library</div><h1>Your music</h1><p class="muted" style="max-width:640px">Add the folders your music lives in: on this computer, on an external drive or in a synced cloud folder. BoothReady reads them where they are and never changes or moves them. Scanning downloads nothing.</p></div>
      <div class="source-actions">${L.sources.some((s) => s.states) ? html`<button class="btn primary" data-act="library-open" data-id="*">Browse the collection</button>` : ""}<button class="btn" data-act="home">Back</button></div>
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
          <div class="stack">${L.sources.map(sourceCard)}</div>
          ${exportsList()}`
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

const bpmText = (bpm: number) => String(Math.round(bpm * 100) / 100);

/** A track's name as DJ software shows it: the title tag, or the file name. */
const titleOf = (t: LibraryTrack) => t.title ?? t.name;

function trackRow(t: LibraryTrack, i: number): Raw {
  const shown = t.issues.slice(0, 3);
  const rest = t.issues.length - shown.length;
  const kind = t.format.toLowerCase().replace(/[^a-z0-9]/g, "") || "other";
  const n = String(i + 1).padStart(3, "0");
  const loaded = !!P.now && trackKey(P.now) === trackKey(t);
  const playing = isPlaying(t);
  // The artist goes under the title. An untagged file shows its folder
  // there instead, and the full location is in the title's tooltip.
  const where = [t.folder, t.name].filter(Boolean).join("/");
  return html`<tr class="${t.state} ${loaded ? "playing" : ""}">
    <td class="idx">${t.state === "unreadable"
      ? n
      : html`<button class="rb-play ${loaded ? "on" : ""}" data-act="library-play" data-id="${trackKey(t)}" aria-label="${playing ? "Pause" : "Play"} ${titleOf(t)}" title="${t.state === "cloud_only" ? "Your cloud app downloads this track to play it." : ""}"><span class="rb-n">${n}</span>${icon(playing ? ICON.pause : ICON.play)}</button>`}</td>
    <td class="art"><span class="arttile f-${kind}" aria-hidden="true">${t.format.slice(0, 4)}</span></td>
    <td class="track-cell"><b title="${where}">${titleOf(t)}</b>${t.artist ? html`<div class="small track-sub track-artist">${t.artist}</div>` : t.folder ? html`<div class="small faint track-sub">${t.folder}</div>` : ""}
      ${shown.length ? html`<details class="track-issues"><summary>${t.state === "blocked" ? "Won't play on" : "No data for"} ${t.issues.map((i) => i.model).join(", ")}</summary><ul>${shown.map((i) => html`<li><b>${i.model}:</b> ${i.reason}</li>`)}${rest > 0 ? html`<li>and ${plural(rest, "more player")}</li>` : ""}</ul></details>` : ""}
      ${t.note ? html`<div class="small track-note">${t.note}</div>` : ""}
    </td>
    <td class="num bpm">${t.bpm === null ? html`<span class="faint">·</span>` : bpmText(t.bpm)}</td>
    <td class="key">${t.camelot ? html`<b>${t.camelot}</b><div class="small faint">${t.key}</div>` : html`<span class="faint">·</span>`}</td>
    <td class="num time">${t.duration === null ? html`<span class="faint">·</span>` : clock(t.duration)}</td>
    <td class="format">${t.format}${t.detail ? html`<div class="small faint">${t.detail}</div>` : t.state === "cloud_only" ? html`<div class="small faint">not checked</div>` : ""}</td>
    <td><span class="trackstate ${t.state}" title="${STATE_HELP[t.state]}"><i class="dot ${t.state}"></i>${STATE_LABEL[t.state]}</span></td>
  </tr>`;
}

/** A column heading that orders the list by that column. */
function sortHead(column: string, label: string, cls = ""): Raw {
  const on = L.sort === column;
  const dir = on ? (L.desc ? "descending" : "ascending") : "none";
  return html`<th class="${cls}" aria-sort="${dir}"><button class="rb-sort ${on ? "on" : ""}" data-act="library-sort" data-id="${column}" title="Sort by ${label.toLowerCase()}">${label}${on ? raw(`<svg viewBox="0 0 10 10" width="8" height="8" aria-hidden="true"><path d="${L.desc ? "M1 3h8L5 8z" : "M1 7h8L5 2z"}" fill="currentColor"/></svg>`) : ""}</button></th>`;
}

function trackTable(): Raw {
  if (L.loadingTracks && !L.tracks.length) return html`<p class="muted">Loading tracks.</p>`;
  if (!L.tracks.length) return html`<p class="muted">${L.query || L.filter !== "all" ? "No tracks match." : "No audio files were found in this folder."}</p>`;
  return html`<table class="tracks">
      <thead><tr>
        <th class="idx"><button class="rb-sort ${L.sort ? "" : "on"}" data-act="library-sort" data-id="" title="Folder order">#</button></th>
        <th class="art"></th>
        <th aria-sort="${L.sort === "title" || L.sort === "artist" ? (L.desc ? "descending" : "ascending") : "none"}"><span class="track-head">${["title", "artist"].map((c) => html`<button class="rb-sort ${L.sort === c ? "on" : ""}" data-act="library-sort" data-id="${c}" title="Sort by ${c}">${c === "title" ? "Track" : "Artist"}${L.sort === c ? raw(`<svg viewBox="0 0 10 10" width="8" height="8" aria-hidden="true"><path d="${L.desc ? "M1 3h8L5 8z" : "M1 7h8L5 2z"}" fill="currentColor"/></svg>`) : ""}</button>`)}</span></th>
        ${sortHead("bpm", "BPM", "num bpm")}${sortHead("key", "Key", "key")}${sortHead("time", "Time", "num time")}${sortHead("format", "Format", "format")}${sortHead("state", "Status")}
      </tr></thead>
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
 * Export to USB: tick folders of the source, see what the export involves,
 * build it into a folder of its own on this computer, then hand it to the
 * USB tab, where the copy flow puts it on the one stick the user confirms.
 */
function exportPanel(s: LibrarySource): Raw {
  const x = L.exp;
  if (!x || x.sourceId !== s.id) return html``;
  if (x.done) {
    const d = x.done;
    return html`<section class="card export" aria-live="polite">
      <div class="banner ok"><div style="flex:1"><b>Export built: ${plural(d.tracks, "track")} in ${plural(d.playlists, "playlist")}${d.analysis_files ? html`, with beat grids and waveforms for ${num(d.analysis_files / 3)} of them` : ""}.</b> It is on this computer as <span class="source-path">${d.path}</span>.</div></div>
      <h2>Next: put it on a USB</h2>
      <p class="small muted">Open the USB tab and plug in a prepared USB. BoothReady offers to copy this export onto it, asks you to confirm that exact drive, and then reads every file back to check the copy. Nothing has been written to a USB yet.</p>
      ${d.warnings.length ? html`<details class="export-warnings"><summary>${plural(d.warnings.length, "note")}</summary><ul>${d.warnings.map((w) => html`<li>${w}</li>`)}</ul></details>` : ""}
      <div class="actions"><button class="btn primary" data-act="home">Go to the USB tab</button><button class="btn" data-act="library-export-close">Close</button></div>
    </section>`;
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
    return html`<section class="card export" aria-live="polite">
      <div class="eyebrow">Building ${x.name}</div>
      <h2>${g ? EXPORT_PHASE[g.phase] : "Starting"}${g?.current ? html`<span class="muted export-current">: ${g.current}</span>` : ""}</h2>
      <div class="bar" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${pct}"><span style="width:${pct}%"></span></div>
      <div class="collect-progress"><span>${g ? html`${num(Math.min(g.done + (g.phase === "database" || g.phase === "install" ? 0 : 1), total))} of ${plural(total, "track")}${eta ? html` · ${eta} left` : ""}` : "Reading the folders."}</span><button class="btn" data-act="library-export-stop">Stop</button></div>
      ${p?.cloud_only ? html`<p class="small muted">Your cloud app downloads ${plural(p.cloud_only, "track")} as they are read, so this step depends on your connection.</p>` : ""}
      <p class="small muted">Each track that hasn't been analysed before takes about ${roughTime(6).replace("under a minute", "six seconds")}. Stopping leaves nothing behind.</p>
    </section>`;
  }
  const folders = L.folders.get(s.id) ?? [];
  const short = !!p && p.free_bytes !== null && p.free_bytes < p.needed_bytes;
  const blocked = !p || short || p.exists || !p.tracks || x.planning;
  return html`<section class="card export">
    <div class="eyebrow">Export to USB</div>
    <h2>Build a rekordbox Device Library from ${s.name}</h2>
    <p class="small muted">Tick the folders to export. Each becomes a playlist with every track below it. BoothReady analyses the tracks itself (beat grid, key and waveforms), writes the export into a folder of its own on this computer, and only then offers to copy it onto a USB.</p>
    <div class="export-folders" role="group" aria-label="Folders to export">
      ${folders.length
        ? folders.map((f, i) => html`<label class="check-row export-folder" style="--depth:${f.depth}"><input id="library-export-folder-${i}" type="checkbox" data-folder="${f.path}" ${x.folders.has(f.path) ? "checked" : ""} ${x.running ? "disabled" : ""}><span class="grow">${f.name}</span><b>${num(f.tracks)}</b></label>`)
        : html`<p class="small muted">This source has no folders to export as playlists.</p>`}
    </div>
    ${x.error ? html`<div class="banner bad" role="alert"><div>${x.error}</div></div>` : ""}
    ${p
      ? html`<dl class="facts collect-facts">
          <dt>Playlists</dt><dd>${p.playlists.map((l) => `${l.name} (${num(l.tracks)})`).join(", ")}</dd>
          <dt>Tracks</dt><dd>${num(p.tracks)}, ${gb(p.bytes)}${p.left_out ? html` · <span class="muted">${plural(p.left_out, "file")} left out: damaged, not audio or a format a Device Library can't hold</span>` : ""}</dd>
          ${p.cloud_only ? html`<dt>From the cloud</dt><dd>${num(p.cloud_only)}, ${gb(p.cloud_only_bytes)}. Your cloud app downloads them as they are read.</dd>` : ""}
          <dt>Analysis</dt><dd>${p.to_analyse ? html`${plural(p.to_analyse, "track")} to analyse, ${roughTime(p.estimate_secs)}` : "Nothing to analyse"}${p.analysed ? html` <span class="muted">· ${num(p.analysed)} already analysed for an earlier export</span>` : ""}</dd>
          <dt>Folder</dt><dd class="source-path">${p.destination}</dd>
          <dt>Free space</dt><dd>${p.free_bytes === null ? "unknown" : html`${gb(p.free_bytes)} free, ${gb(p.needed_bytes)} needed`}</dd>
        </dl>
        <label class="field-label" for="library-export-name">Export name</label>
        <input id="library-export-name" class="search" type="text" value="${x.name}" autocomplete="off" spellcheck="false">
        ${short ? html`<div class="banner bad"><div>There isn't enough free space on this computer for this export.</div></div>` : ""}
        ${p.exists ? html`<div class="banner warn"><div>An export with this name is already there. Choose another name; an export is never written into an existing one.</div></div>` : ""}`
      : x.planning
        ? html`<p class="muted">Working out what to export.</p>`
        : x.folders.size
          ? ""
          : html`<p class="muted">Tick at least one folder.</p>`}
    <div class="actions"><button class="btn primary" data-act="library-export-start" ${blocked ? "disabled" : ""}>Build the export${p?.tracks ? ` (${plural(p.tracks, "track")})` : ""}</button><button class="btn" data-act="library-export-close">Cancel</button></div>
  </section>`;
}

/** Finished exports on this computer, for the Library screen. */
function exportsList(): Raw {
  if (!L.exports.length) return html``;
  return html`<section class="card exports">
    <div class="eyebrow">Exports on this computer</div>
    <h2>Ready to copy onto a USB</h2>
    <p class="small muted">Open the USB tab with a prepared USB plugged in and BoothReady offers to copy one of these onto it.</p>
    <div class="export-list">${L.exports.map((e) => html`<div class="export-row" data-export="${e.name}">
      <div class="grow"><b>${e.name}</b><div class="small muted">${plural(e.tracks, "track")} in ${plural(e.playlists, "playlist")} · ${gb(e.bytes)} · from ${e.source} · built ${when(e.created_unix)}</div><div class="small faint source-path">${e.path}</div></div>
      ${L.exportRemoveAsk === e.name
        ? html`<span class="small">Remove this export from this computer? USBs are not touched.</span><button class="btn danger" data-act="library-export-remove" data-id="${e.name}">Remove</button><button class="btn link" data-act="library-export-remove-keep">Keep</button>`
        : html`<button class="btn link small" data-act="library-export-remove-ask" data-id="${e.name}">Remove</button>`}
    </div>`)}</div>
  </section>`;
}

/**
 * What to do with a finished collection. BoothReady does not write DJ
 * libraries, so the export is done in rekordbox and checked back here.
 */
function nextStep(s: LibrarySource): Raw {
  return html`<section class="card nextstep">
    <div class="eyebrow">Next step</div>
    <h2>Export this collection to your USB</h2>
    <p class="small muted">BoothReady can build the rekordbox Device Library itself from the folders in this collection, or you can import the folder into rekordbox and export from there with both Device Library and OneLibrary turned on.</p>
    <div class="actions"><button class="btn primary" data-act="library-export-open" data-id="${s.id}" ${L.exp ? "disabled" : ""}>Export to USB</button><button class="btn" data-act="open-rekordbox">Open rekordbox</button><button class="btn" data-act="library-copy-path" data-id="${s.id}">Copy folder path</button><button class="btn" data-act="home">Go to the USB tab</button></div>
  </section>`;
}

/** Re-render only the track list, so typing in the search box keeps focus. */
function renderTracks() {
  const el = document.getElementById("library-tracks");
  if (el) el.innerHTML = trackTable().s;
  else L.host.render();
}

/** The browser's tree: the whole collection, then each source and its folders. */
function tree(selected: string): Raw {
  const all = L.sources.reduce((n, s) => n + (s.totals?.tracks ?? 0), 0);
  const node = (key: string, label: string, count: number, depth: number, hasChildren: boolean, act: string, on: boolean) => html`<div class="rb-node ${on ? "on" : ""}" style="--depth:${depth}">
      ${hasChildren ? html`<button class="rb-caret" data-act="library-toggle" data-id="${key}" aria-label="${L.open.has(key) ? "Collapse" : "Expand"} ${label}" aria-expanded="${L.open.has(key) ? "true" : "false"}">${raw(`<svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true"><path d="${L.open.has(key) ? "M1 3h8L5 8z" : "M3 1v8l5-4z"}" fill="currentColor"/></svg>`)}</button>` : html`<span class="rb-caret"></span>`}
      <button class="rb-label" data-act="${act}" data-id="${act === "library-open" ? key : `${key.includes("|") ? key : `${key}|`}`}"><span class="rb-name">${label}</span><b>${num(count)}</b></button>
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
    return html`${node(s.id, s.name, s.totals?.tracks ?? 0, 0, !!s.states && (folders.length > 0 || !L.folders.has(s.id)), "library-folder", selected === s.id && !L.folder)}
      ${L.open.has(s.id) ? visible.map((f) => node(`${s.id}|${f.path}`, f.name, f.tracks, f.depth + 1, hasKids(f), "library-folder", selected === s.id && L.folder === f.path)) : ""}`;
  });
  return html`<nav class="rb-tree" aria-label="Library">
    <div class="rb-section">Collection</div>
    ${node(ALL, "All tracks", all, 0, false, "library-open", selected === ALL)}
    <div class="rb-section">Sources</div>
    ${sourceNodes}
    <button class="btn link small rb-manage" data-act="open-library">Manage sources</button>
  </nav>`;
}

export function librarySourceView(id: string): Raw {
  const all = id === ALL;
  const s = all ? undefined : source(id);
  if (!all && !s) return html`<div class="library"><p class="muted">That music source was removed.</p><button class="btn" data-act="open-library">Back to your music</button></div>`;
  const c = L.states ?? s?.states ?? null;
  const total = c ? STATE_ORDER.reduce((n, k) => n + c[k], 0) : 0;
  const scanning = !!s && (s.scanning || L.progress?.source_id === s.id);
  const title = L.folder ? L.folder.split("/").pop() ?? L.folder : all ? "All tracks" : s!.name;
  const crumb = all ? "Collection" : [locationLabel(s!.location), s!.name, ...L.folder.split("/").filter(Boolean).slice(0, -1)].join(" · ");
  const browsable = all || !!s?.states;
  const stale = (s ? [s] : L.sources).filter((x) => x.states && !x.tags_read);
  return html`<div class="library browser">
    <div class="devtools-heading">
      <div><div class="eyebrow">${crumb}${s && !s.connected ? " · not connected" : ""}</div><h1>${title}</h1>${s ? html`<div class="small faint source-path">${s.path}${L.folder ? `/${L.folder}` : ""}</div>` : html`<div class="small muted">Every track in ${plural(L.sources.length, "source")}.</div>`}</div>
      <div class="source-actions"><button class="btn" data-act="open-library">Manage sources</button>${s ? (scanning ? html`<button class="btn" data-act="library-stop">Stop</button>` : html`<button class="btn" data-act="library-scan" data-id="${s.id}" ${s.connected ? "" : "disabled"}>Scan again</button>`) : ""}${s && s.states && !s.collected_from && !L.collect ? html`<button class="btn" data-act="library-collect-open" data-id="${s.id}" ${s.connected && !scanning ? "" : "disabled"}>Collect</button>` : ""}${s && s.states && !L.exp ? html`<button class="btn primary" data-act="library-export-open" data-id="${s.id}" ${s.connected && !scanning ? "" : "disabled"}>Export to USB</button>` : ""}</div>
    </div>
    ${errorBanner()}
    ${scanning ? html`<div class="banner"><span class="spinner" aria-hidden="true"></span><div>Scanning. ${L.progress?.files ? html`${plural(L.progress.files, "track")} found.` : ""}</div></div>` : ""}
    ${s && !s.connected ? html`<div class="banner"><div>This drive isn't connected. You're looking at the scan from ${when(s.scanned_unix)}.</div></div>` : ""}
    ${stale.length && !scanning ? html`<div class="banner"><div style="flex:1">${s ? "This folder was scanned" : `${stale.map((x) => x.name).join(", ")} ${stale.length === 1 ? "was" : "were"} scanned`} before BoothReady read tags, so ${s ? "its" : "their"} tracks show file names only. Scan again to see artist, title, BPM and key.</div>${s && s.connected ? html`<button class="btn" data-act="library-scan" data-id="${s.id}">Scan again</button>` : ""}</div>` : ""}
    ${s ? exportPanel(s) : ""}
    ${s ? collectPanel(s) : ""}
    ${s?.collected_from && !L.exp ? nextStep(s) : ""}
    <div class="rb">
      ${tree(id)}
      <div class="rb-main">
        <div class="library-top">
          <div class="small muted">${s?.totals ? html`<b>${plural(s.totals.tracks, "track")}</b> in this source · ${gb(s.totals.bytes)} · scanned ${when(s.scanned_unix)}` : all ? html`<b>${plural(total, "track")}</b> across your sources` : "Not scanned yet."}</div>
          ${playersControl()}
        </div>
        ${c ? stateBar(c) : ""}
        ${browsable && c
          ? html`<div class="library-filters rb-filters" role="group" aria-label="Filter tracks">
                <button class="chip ${L.filter === "all" ? "on" : ""}" data-act="library-filter" data-id="all">All <b>${num(total)}</b></button>
                ${STATE_ORDER.filter((k) => c[k] || L.filter === k).map((k) => html`<button class="chip ${k} ${L.filter === k ? "on" : ""}" data-act="library-filter" data-id="${k}" title="${STATE_HELP[k]}"><i class="dot ${k}"></i>${STATE_LABEL[k]} <b>${num(c[k])}</b></button>`)}
              </div>
              <input id="library-search" class="search" type="search" placeholder="Search by title, artist, album or folder" value="${L.query}" aria-label="Search tracks" autocomplete="off" spellcheck="false">
              <div id="library-tracks" class="card tracks-card">${trackTable()}</div>`
          : html`<p class="muted">${s?.totals ? "The track list from the last scan isn't saved on this computer. Scan again to see it." : "Scan this folder to see its tracks."}</p>`}
      </div>
    </div>
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
