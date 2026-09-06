import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { listen, type Event } from "@tauri-apps/api/event";
import * as pdfjsLib from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";

pdfjsLib.GlobalWorkerOptions.workerSrc = workerUrl;

type Meta = { width: number; height: number; page_count: number };

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const loadBtn = $<HTMLButtonElement>("load-btn");
const emptyState = $("empty-state");
const preview = $("preview");
const pageBox = $("page-box");
const pageCanvas = $<HTMLCanvasElement>("page-canvas");
const wmLabel = $("watermark-label");
const textInput = $<HTMLInputElement>("text-input");
const sliders = {
  r: $<HTMLInputElement>("ch-r"),
  g: $<HTMLInputElement>("ch-g"),
  b: $<HTMLInputElement>("ch-b"),
  a: $<HTMLInputElement>("ch-a"),
  size: $<HTMLInputElement>("ch-size"),
};
const sizeVal = $("size-val");
const swatch = $("swatch");
const finishBtn = $<HTMLButtonElement>("finish-btn");
const backdrop = $("modal-backdrop");
const modalMsg = $("modal-msg");
const modalClose = $<HTMLButtonElement>("modal-close");

const state = {
  path: null as string | null,
  meta: null as Meta | null,
  xFrac: 0.5,
  yFrac: 0.5,
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

function updateSwatch() {
  const { r, g, b, a } = sliders;
  const css = `rgba(${r.value}, ${g.value}, ${b.value}, ${(Number(a.value) / 255).toFixed(3)})`;
  swatch.style.background = css;
  wmLabel.style.color = css;
  syncLabelSize();
}

// keep preview label visually proportional to the real PDF pt size
function syncLabelSize() {
  const px = state.meta
    ? Number(sliders.size.value) * (pageBox.getBoundingClientRect().width / state.meta.width)
    : `${sliders.size.value}px`;
  wmLabel.style.fontSize = `${px}px`;
}
window.addEventListener("resize", syncLabelSize);
for (const s of Object.values(sliders)) s.addEventListener("input", updateSwatch);
sliders.size.addEventListener("input", () => (sizeVal.textContent = `${sliders.size.value}pt`));
updateSwatch();

textInput.value = localStorage.getItem("watermark-text") ?? "";
textInput.addEventListener("input", () => {
  localStorage.setItem("watermark-text", textInput.value);
  wmLabel.textContent = textInput.value || " ";
  refreshFinish();
});

function refreshFinish() {
  finishBtn.disabled = !(state.path && textInput.value.trim());
}

function labelFromFractions() {
  wmLabel.style.left = `${state.xFrac * 100}%`;
  wmLabel.style.top = `${state.yFrac * 100}%`;
}

// drag watermark label inside page box
let dragging = false;
wmLabel.addEventListener("pointerdown", (e) => {
  dragging = true;
  wmLabel.setPointerCapture(e.pointerId);
});
wmLabel.addEventListener("pointermove", (e) => {
  if (!dragging) return;
  const rect = pageBox.getBoundingClientRect();
  state.xFrac = Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width));
  state.yFrac = Math.min(1, Math.max(0, (e.clientY - rect.top) / rect.height));
  labelFromFractions();
});
wmLabel.addEventListener("pointerup", () => {
  dragging = false;
  const rect = pageBox.getBoundingClientRect();
  console.log(
    `[wm] drop: xFrac=${state.xFrac.toFixed(4)} yFrac=${state.yFrac.toFixed(4)}`,
    `pageBox=(${rect.left.toFixed(1)},${rect.top.toFixed(1)}) ${rect.width.toFixed(1)}x${rect.height.toFixed(1)}`,
    `meta=${state.meta ? `${state.meta.width}x${state.meta.height} pages=${state.meta.page_count}` : "none"}`,
  );
});
wmLabel.addEventListener("pointerdown", (e) => e.preventDefault());

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
    wmLabel.hidden = false;
    wmLabel.textContent = textInput.value || " ";
    state.path = path;
    state.meta = meta;
    labelFromFractions();
    syncLabelSize();
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
    `[wm] apply: xFrac=${state.xFrac.toFixed(4)} yFrac=${state.yFrac.toFixed(4)}`,
    `text="${textInput.value}" size=${sliders.size.value}`,
    `meta=${state.meta ? `${state.meta.width}x${state.meta.height}` : "none"}`,
  );
  try {
    await invoke("apply_watermark", {
      path: state.path,
      outPath: out,
      text: textInput.value,
      xFrac: state.xFrac,
      yFrac: state.yFrac,
      fontSize: Number(sliders.size.value),
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
