import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { listen, type Event } from "@tauri-apps/api/event";
import * as pdfjsLib from "pdfjs-dist";
import PdfWorker from "pdfjs-dist/build/pdf.worker.min.mjs?worker";

pdfjsLib.GlobalWorkerOptions.workerPort = new PdfWorker();

type Meta = { width: number; height: number; page_count: number };
type Align = "left" | "center" | "right";

type Entry = {
  text: string;
  size: number;
  align: Align;
  xFrac: number;
  yFrac: number;
};

type TextPayload = {
  text: string;
  font_size: number;
  x_frac: number;
  y_frac: number;
  align: Align;
};

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const loadBtn = $<HTMLButtonElement>("load-btn");
const emptyState = $("empty-state");
const preview = $("preview");
const pageBox = $("page-box");
const pageCanvas = $<HTMLCanvasElement>("page-canvas");
const sliders = {
  r: $<HTMLInputElement>("ch-r"),
  g: $<HTMLInputElement>("ch-g"),
  b: $<HTMLInputElement>("ch-b"),
  a: $<HTMLInputElement>("ch-a"),
};
const swatch = $("swatch");
const textList = $("text-list");
const addTextBtn = $<HTMLButtonElement>("add-text-btn");
const finishBtn = $<HTMLButtonElement>("finish-btn");
const backdrop = $("modal-backdrop");
const modalMsg = $("modal-msg");
const modalClose = $<HTMLButtonElement>("modal-close");

const DEFAULT_ENTRY = (): Entry => ({
  text: "",
  size: 24,
  align: "center",
  xFrac: 0.5,
  yFrac: 0.5,
});

const state = {
  path: null as string | null,
  meta: null as Meta | null,
  entries: [] as Entry[],
  selected: 0,
};

const isTauri = "__TAURI_INTERNALS__" in window;
if (!isTauri) {
  loadBtn.disabled = true;
  finishBtn.disabled = true;
  popup(
    "This UI must run inside the Tauri app.\nUse: make dev  (i.e. `cargo tauri dev` from the repo root)"
  );
}

function popup(msg: string) {
  modalMsg.textContent = msg || String(msg);
  backdrop.hidden = false;
  modalClose.focus();
}
function errText(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === "string" && e) return e;
  try {
    return JSON.stringify(e);
  } catch {
    return "unknown error";
  }
}
window.addEventListener("unhandledrejection", (ev) => {
  popup(`Unexpected error:\n${errText(ev.reason)}`);
});
modalClose.addEventListener("click", () => (backdrop.hidden = true));
backdrop.addEventListener("click", (e) => {
  if (e.target === backdrop) backdrop.hidden = true;
});

// ---------- localStorage cache ----------
const CACHE_KEY = "watermark-texts";
const COLOR_KEY = "watermark-color";

function loadColorCache() {
  try {
    const raw = localStorage.getItem(COLOR_KEY);
    if (!raw) return;
    const c = JSON.parse(raw);
    if (typeof c !== "object" || c === null) return;
    const clamp = (v: unknown, fallback: number) => {
      const n = Number(v);
      return Number.isFinite(n) ? Math.min(255, Math.max(0, Math.round(n))) : fallback;
    };
    sliders.r.value = String(clamp(c.r, 0));
    sliders.g.value = String(clamp(c.g, 0));
    sliders.b.value = String(clamp(c.b, 0));
    sliders.a.value = String(clamp(c.a, 153));
  } catch {
    // ignore malformed cache
  }
}

function saveColorCache() {
  localStorage.setItem(
    COLOR_KEY,
    JSON.stringify({
      r: Number(sliders.r.value),
      g: Number(sliders.g.value),
      b: Number(sliders.b.value),
      a: Number(sliders.a.value),
    })
  );
}

function loadCache(): Entry[] {
  try {
    const raw = localStorage.getItem(CACHE_KEY);
    if (!raw) return [];
    const arr = JSON.parse(raw);
    if (!Array.isArray(arr)) return [];
    return arr
      .filter((e) => typeof e === "object" && e !== null)
      .map((e) => ({
        text: typeof e.text === "string" ? e.text : "",
        size: Number.isFinite(Number(e.size)) ? Number(e.size) : 24,
        align: e.align === "left" || e.align === "right" ? e.align : "center",
        xFrac: 0.5,
        yFrac: 0.5,
      }));
  } catch {
    return [];
  }
}

function saveCache() {
  localStorage.setItem(
    CACHE_KEY,
    JSON.stringify(state.entries.map(({ text, size, align }) => ({ text, size, align })))
  );
}

const previewLabels: HTMLDivElement[] = [];

function updateSwatch() {
  const { r, g, b, a } = sliders;
  const css = `rgba(${r.value}, ${g.value}, ${b.value}, ${(Number(a.value) / 255).toFixed(3)})`;
  swatch.style.background = css;
  for (const l of previewLabels) l.style.color = css;
  syncLabelSizes();
}

// keep preview labels visually proportional to the real PDF pt size
function syncLabelSizes() {
  const scale = state.meta
    ? pageBox.getBoundingClientRect().width / state.meta.width
    : null;
  for (const l of previewLabels) {
    const pt = Number(l.dataset.size || 24);
    l.style.fontSize = `${scale ? pt * scale : pt}px`;
  }
}
window.addEventListener("resize", syncLabelSizes);
for (const s of Object.values(sliders))
  s.addEventListener("input", () => {
    saveColorCache();
    updateSwatch();
  });
loadColorCache();
updateSwatch();

// ---------- entries / form ----------
function refreshFinish() {
  finishBtn.disabled = !(state.path && state.entries.some((e) => e.text.trim()));
}

function labelFromFractions(entry: Entry, label: HTMLDivElement) {
  label.style.left = `${entry.xFrac * 100}%`;
  label.style.top = `${entry.yFrac * 100}%`;
}

function renderPreviewLabels() {
  for (const l of previewLabels) l.remove();
  previewLabels.length = 0;
  state.entries.forEach((entry, i) => {
    const label = document.createElement("div");
    label.className = "watermark-label";
    label.dataset.size = String(entry.size);
    label.textContent = entry.text || " ";
    label.style.textAlign = entry.align;
    labelFromFractions(entry, label);
    if (i === state.selected) label.classList.add("selected");
    attachDrag(label, entry);
    pageBox.appendChild(label);
    previewLabels.push(label);
  });
  updateSwatch();
  syncLabelSizes();
}

function renderForm() {
  textList.replaceChildren();
  state.entries.forEach((entry, i) => {
    const row = document.createElement("div");
    row.className = "text-row" + (i === state.selected ? " selected" : "");

    const capRow = document.createElement("div");
    capRow.className = "caption-row";

    const cap = document.createElement("span");
    cap.className = "caption";
    cap.textContent = "Text";

    const controls = document.createElement("div");
    controls.className = "row-controls";

    const sizeInput = document.createElement("input");
    sizeInput.type = "number";
    sizeInput.min = "6";
    sizeInput.max = "144";
    sizeInput.value = String(entry.size);
    sizeInput.title = "Font size (pt)";
    sizeInput.addEventListener("input", () => {
      const n = Number(sizeInput.value);
      entry.size = Number.isFinite(n) ? Math.min(144, Math.max(1, n)) : 24;
      saveCache();
      renderPreviewLabels();
    });

    const alignSel = document.createElement("select");
    alignSel.title = "Text alignment";
    for (const [value, label] of [
      ["left", "Left"],
      ["center", "Center"],
      ["right", "Right"],
    ] as const) {
      const opt = document.createElement("option");
      opt.value = value;
      opt.textContent = label;
      alignSel.appendChild(opt);
    }
    alignSel.value = entry.align;
    alignSel.addEventListener("change", () => {
      entry.align = alignSel.value as Align;
      saveCache();
      const label = previewLabels[i];
      if (label) label.style.textAlign = entry.align;
    });

    const delBtn = document.createElement("button");
    delBtn.type = "button";
    delBtn.className = "row-btn";
    delBtn.textContent = "x";
    delBtn.title = "Remove text";
    delBtn.addEventListener("click", () => {
      if (state.entries.length <= 1) return;
      state.entries.splice(i, 1);
      state.selected = Math.min(state.selected, state.entries.length - 1);
      saveCache();
      renderForm();
      renderPreviewLabels();
      refreshFinish();
    });

    controls.append(sizeInput, alignSel, delBtn);
    capRow.append(cap, controls);

    const ta = document.createElement("textarea");
    ta.rows = 3;
    ta.placeholder = "Watermark text";
    ta.value = entry.text;
    ta.addEventListener("input", () => {
      entry.text = ta.value;
      saveCache();
      const label = previewLabels[i];
      if (label) {
        label.textContent = entry.text || " ";
        label.dataset.size = String(entry.size);
      }
      refreshFinish();
    });
    ta.addEventListener("focus", () => {
      state.selected = i;
      renderPreviewLabels();
      for (const [j, r] of Array.from(textList.children).entries()) {
        r.classList.toggle("selected", j === i);
      }
    });

    row.append(capRow, ta);
    textList.appendChild(row);
  });
}

addTextBtn.addEventListener("click", () => {
  const n = state.entries.length;
  const e = DEFAULT_ENTRY();
  // stagger new entries slightly so labels don't stack invisibly
  e.yFrac = Math.min(1, 0.5 + n * 0.07);
  state.entries.push(e);
  state.selected = n;
  saveCache();
  renderForm();
  renderPreviewLabels();
  refreshFinish();
});

// drag watermark label inside page box (dragging selects + moves that entry)
let dragging: { index: number; label: HTMLDivElement } | null = null;

function attachDrag(label: HTMLDivElement, _entry: Entry) {
  const index = previewLabels.length;
  label.addEventListener("pointerdown", (e) => {
    dragging = { index, label };
    label.setPointerCapture(e.pointerId);
    if (state.selected !== index) {
      state.selected = index;
      for (const [i, l] of previewLabels.entries()) l.classList.toggle("selected", i === index);
    }
  });
  label.addEventListener("pointermove", (e) => {
    if (!dragging || dragging.label !== label) return;
    const rect = pageBox.getBoundingClientRect();
    state.entries[index].xFrac = Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width));
    state.entries[index].yFrac = Math.min(1, Math.max(0, (e.clientY - rect.top) / rect.height));
    labelFromFractions(state.entries[index], label);
  });
  label.addEventListener("pointerup", () => {
    if (!dragging || dragging.label !== label) return;
    dragging = null;
    const entry = state.entries[index];
    const rect = pageBox.getBoundingClientRect();
    console.log(
      `[wm] drop: text="${entry.text}" xFrac=${entry.xFrac.toFixed(4)} yFrac=${entry.yFrac.toFixed(4)}`,
      `pageBox=(${rect.left.toFixed(1)},${rect.top.toFixed(1)}) ${rect.width.toFixed(1)}x${rect.height.toFixed(1)}`,
      `meta=${state.meta ? `${state.meta.width}x${state.meta.height} pages=${state.meta.page_count}` : "none"}`,
    );
  });
  label.addEventListener("pointerdown", (e) => e.preventDefault());
}

// ---------- init entries from cache ----------
{
  const cached = loadCache();
  state.entries = cached.length ? cached : [DEFAULT_ENTRY()];
  // spread default positions vertically when multiple entries share 0.5
  state.entries.forEach((e, i) => {
    if (e.xFrac === 0.5 && e.yFrac === 0.5 && i > 0) e.yFrac = Math.min(1, 0.5 + i * 0.07);
  });
  renderForm();
  renderPreviewLabels();
  refreshFinish();
}

async function loadPdf(path: string) {
  try {
    const meta = await invoke<Meta>("load_pdf", { path });
    // preview a middle page (first and last are never watermarked)
    const b64 = await invoke<string>("pdf_bytes_b64", { path });
    const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    const doc = await pdfjsLib.getDocument({ data: bytes }).promise;
    const page = await doc.getPage(Math.min(doc.numPages, Math.max(2, meta.page_count - 1)));

    const aspect = meta.width / meta.height;
    // fit the page box into the preview zone (both dimensions)
    const zone = preview.getBoundingClientRect();
    const availW = zone.width - 16;
    const availH = zone.height - 16;
    const boxW = Math.min(availW, availH * aspect);
    pageBox.style.width = `${Math.floor(boxW)}px`;
    pageBox.style.height = `${Math.floor(boxW / aspect)}px`;
    // render exactly into page-box; canvas maps 1:1 to PDF page coordinates
    const base = page.getViewport({ scale: 1 });
    const scale = (boxW / base.width) * (window.devicePixelRatio || 1);
    const vp = page.getViewport({ scale });
    pageCanvas.width = Math.floor(vp.width);
    pageCanvas.height = Math.floor(vp.height);
    pageCanvas.style.width = "100%";
    pageCanvas.style.height = "100%";
    await page.render({ canvas: pageCanvas, viewport: vp }).promise;

    emptyState.hidden = true;
    preview.hidden = false;
    state.path = path;
    state.meta = meta;
    renderPreviewLabels();
    refreshFinish();
  } catch (e) {
    popup(`Failed to load PDF:\n${errText(e)}`);
  }
}

if (isTauri) {
  loadBtn.addEventListener("click", () => void pickAndLoad());
  async function pickAndLoad() {
    const path = await open({
      multiple: false,
      filters: [{ name: "PDF", extensions: ["pdf"] }],
    });
    if (typeof path === "string") await loadPdf(path);
  }
}

// file drop (tauri intercepts webview drops and emits these events)
const zone = $("preview-zone");
zone.addEventListener("dragover", (e) => {
  e.preventDefault();
  if (!state.path) emptyState.classList.add("dragover");
});
zone.addEventListener("dragleave", () => emptyState.classList.remove("dragover"));

type DragPayload = { paths?: string[]; position?: unknown };
const onDrop = (e: Event<DragPayload>) => {
  emptyState.classList.remove("dragover");
  if (!state.path && e.payload?.paths?.length) void loadPdf(e.payload.paths[0]);
};
try {
  void listen<DragPayload>("tauri://drag-drop", onDrop);
  void listen("tauri://drag-leave", () => emptyState.classList.remove("dragover"));
} catch (e) {
  console.warn("drag-drop listener failed:", e);
}

finishBtn.addEventListener("click", async () => {
  if (!state.path) return;
  const texts: TextPayload[] = state.entries
    .filter((e) => e.text.trim())
    .map((e) => ({
      text: e.text,
      font_size: e.size,
      x_frac: e.xFrac,
      y_frac: e.yFrac,
      align: e.align,
    }));
  if (!texts.length) return;
  let out: string | null = null;
  try {
    out = await save({
      filters: [{ name: "PDF", extensions: ["pdf"] }],
      defaultPath: "watermarked.pdf",
    });
  } catch (e) {
    popup(`Save dialog failed:\n${errText(e)}`);
    return;
  }
  if (typeof out !== "string") return;
  console.log(
    `[wm] apply: texts=${JSON.stringify(texts)}`,
    `meta=${state.meta ? `${state.meta.width}x${state.meta.height}` : "none"}`,
  );
  try {
    await invoke("apply_watermark", {
      path: state.path,
      outPath: out,
      texts,
      r: Number(sliders.r.value),
      g: Number(sliders.g.value),
      b: Number(sliders.b.value),
      a: Number(sliders.a.value),
    });
    popup("Saved.");
  } catch (e) {
    popup(`Failed to apply watermark:\n${errText(e)}`);
  }
});
