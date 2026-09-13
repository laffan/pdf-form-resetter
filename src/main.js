// The window: render the PDF as it is on disk, let a field be picked, and ask
// the Rust side to reset it. It holds no copy of the document — every action
// re-reads the file, so what is on screen is what another program would see.

import * as pdfjs from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;

// Copied out of node_modules by scripts/sync-pdfjs.mjs.
const pdfAssets = {
  cMapUrl: "pdfjs/cmaps/",
  cMapPacked: true,
  standardFontDataUrl: "pdfjs/standard_fonts/",
  wasmUrl: "pdfjs/wasm/",
  iccUrl: "pdfjs/iccs/",
};

const PAGE_GAP = 20;
const MAX_PAGE_WIDTH = 1000;

const ui = {
  open: document.getElementById("open"),
  viewer: document.getElementById("viewer"),
  placeholder: document.getElementById("placeholder"),
  docName: document.getElementById("doc-name"),
  docMeta: document.getElementById("doc-meta"),
  status: document.getElementById("status"),
  selection: document.getElementById("selection"),
  selectionName: document.getElementById("selection-name"),
  selectionDetail: document.getElementById("selection-detail"),
  reset: document.getElementById("reset"),
  clear: document.getElementById("clear"),
};

const state = {
  path: null,
  model: null,
  pdf: null,
  task: null,
  /** Field ids, in the order they were picked. */
  selected: [],
  fields: new Map(),
  /** Cancels renders still queued when a new document replaces this one. */
  generation: 0,
};

/* ------------------------------------------------------------------ opening */

async function openFile(path, { keepScroll = false } = {}) {
  const scrollTop = keepScroll ? ui.viewer.scrollTop : 0;
  const generation = ++state.generation;

  try {
    const [model, bytes] = await Promise.all([
      invoke("open_pdf", { path }),
      invoke("pdf_bytes", { path }),
    ]);
    // pdf.js takes ownership of the buffer it is handed, so give it a copy of
    // its own rather than one we might read again later. Teardown lives on the
    // loading task, not the document, so keep hold of it.
    const task = pdfjs.getDocument({ data: new Uint8Array(bytes), ...pdfAssets });
    const pdf = await task.promise;

    if (generation !== state.generation) {
      await task.destroy();
      return;
    }

    await state.task?.destroy();
    state.task = task;
    state.path = path;
    state.model = model;
    state.pdf = pdf;
    state.fields = new Map(model.fields.map((field) => [field.id, field]));
    state.selected = state.selected.filter((id) => state.fields.has(id));

    describe(model);
    await drawPages(generation);
    ui.viewer.scrollTop = scrollTop;
    paintSelection();
  } catch (error) {
    say(message(error), true);
  }
}

function describe(model) {
  ui.docName.textContent = model.fileName;
  const resettable = model.fields.filter((field) => field.resettable);
  const filled = resettable.filter((field) => field.hasValue);
  const pages = `${model.pageCount} page${model.pageCount === 1 ? "" : "s"}`;
  ui.docMeta.textContent = resettable.length
    ? `${pages} · ${resettable.length} field${resettable.length === 1 ? "" : "s"}, ${filled.length} filled`
    : `${pages} · no form fields`;
  if (!resettable.length) {
    say("This PDF has no form fields to reset.");
  } else if (!model.writable) {
    say("This file is read-only; a reset will not be able to save.", true);
  } else {
    say("");
  }
}

/* ----------------------------------------------------------------- rendering */

async function drawPages(generation) {
  ui.placeholder.remove();
  ui.viewer.replaceChildren();

  const width = Math.min(ui.viewer.clientWidth - 2 * PAGE_GAP, MAX_PAGE_WIDTH);
  const byPage = groupWidgetsByPage();

  // Page canvases are drawn only once they come near the viewport: a long form
  // should not cost a hundred rasterisations up front. The boxes do not wait,
  // so a field can be picked the moment it scrolls into view.
  const observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (entry.isIntersecting) {
          observer.unobserve(entry.target);
          paintPage(entry.target, generation);
        }
      }
    },
    { root: ui.viewer, rootMargin: "200px 0px" },
  );

  for (let number = 1; number <= state.pdf.numPages; number += 1) {
    const page = await state.pdf.getPage(number);
    if (generation !== state.generation) return;

    const viewport = page.getViewport({ scale: width / page.getViewport({ scale: 1 }).width });
    const element = document.createElement("div");
    element.className = "page";
    element.style.width = `${viewport.width}px`;
    element.style.height = `${viewport.height}px`;
    element.page = page;
    element.viewport = viewport;

    const overlay = document.createElement("div");
    overlay.className = "overlay";
    for (const { field, widget } of byPage.get(number - 1) ?? []) {
      overlay.append(boxFor(field, widget, viewport));
    }
    element.append(overlay);

    ui.viewer.append(element);
    observer.observe(element);
  }
}

async function paintPage(element, generation) {
  const canvas = document.createElement("canvas");
  const ratio = window.devicePixelRatio || 1;
  const viewport = element.page.getViewport({ scale: element.viewport.scale * ratio });
  canvas.width = Math.floor(viewport.width);
  canvas.height = Math.floor(viewport.height);

  // Default annotation handling paints each widget's own appearance stream,
  // so the canvas shows exactly the state stored in the file — including the
  // radio button that cannot be unchecked in a viewer.
  await element.page.render({ canvasContext: canvas.getContext("2d"), viewport }).promise;
  if (generation === state.generation) element.prepend(canvas);
}

function groupWidgetsByPage() {
  const pages = new Map();
  for (const field of state.model.fields) {
    for (const widget of field.widgets) {
      if (widget.pageIndex === null || !widget.rect) continue;
      const list = pages.get(widget.pageIndex) ?? [];
      list.push({ field, widget });
      pages.set(widget.pageIndex, list);
    }
  }
  return pages;
}

function boxFor(field, widget, viewport) {
  const [x1, y1, x2, y2] = viewport.convertToViewportRectangle(widget.rect);
  const box = document.createElement("button");
  box.type = "button";
  box.dataset.fieldId = field.id;
  box.style.left = `${Math.min(x1, x2)}px`;
  box.style.top = `${Math.min(y1, y2)}px`;
  box.style.width = `${Math.abs(x2 - x1)}px`;
  box.style.height = `${Math.abs(y2 - y1)}px`;

  if (!field.resettable) {
    box.className = "widget inert";
    box.disabled = true;
    box.title = field.note ?? "";
    return box;
  }

  box.className = field.hasValue ? "widget filled" : "widget";
  box.title = summarise(field);
  box.addEventListener("click", (event) => {
    pick(field.id, event.shiftKey || event.metaKey || event.ctrlKey);
  });
  return box;
}

/* ----------------------------------------------------------------- selecting */

function pick(id, additive) {
  if (additive) {
    state.selected = state.selected.includes(id)
      ? state.selected.filter((each) => each !== id)
      : [...state.selected, id];
  } else {
    state.selected = state.selected.length === 1 && state.selected[0] === id ? [] : [id];
  }
  paintSelection();
}

function paintSelection() {
  const chosen = new Set(state.selected);
  for (const box of ui.viewer.querySelectorAll(".widget")) {
    box.classList.toggle("selected", chosen.has(box.dataset.fieldId));
  }

  const fields = state.selected.map((id) => state.fields.get(id)).filter(Boolean);
  ui.selection.hidden = fields.length === 0;
  if (!fields.length) return;

  if (fields.length === 1) {
    const [field] = fields;
    ui.selectionName.textContent = field.name || "(unnamed field)";
    ui.selectionDetail.textContent = summarise(field);
  } else {
    ui.selectionName.textContent = `${fields.length} fields selected`;
    ui.selectionDetail.textContent = fields
      .map((field) => field.name || "(unnamed)")
      .join(", ");
  }

  // Restoring the default and clearing outright are the same action unless one
  // of these fields actually has a default, so the second button only shows up
  // where it would do something different.
  ui.clear.hidden = !fields.some((field) => field.defaultValue);
  ui.reset.textContent = ui.clear.hidden ? "RESET" : "RESET TO DEFAULT";
}

function summarise(field) {
  const parts = [field.kindLabel];
  parts.push(field.value ? `currently "${field.value}"` : "currently empty");
  if (field.defaultValue) parts.push(`default "${field.defaultValue}"`);
  if (field.widgets.length > 1) parts.push(`${field.widgets.length} widgets`);
  if (field.readOnly) parts.push("read-only");
  if (field.sharedValue) parts.push("value shared with sibling fields");
  return parts.join(" · ");
}

/* ------------------------------------------------------------------ resetting */

async function applyReset(mode) {
  if (!state.selected.length) return;
  const buttons = [ui.reset, ui.clear];
  buttons.forEach((button) => (button.disabled = true));
  say("Writing…");

  try {
    const report = await invoke("reset_fields", {
      path: state.path,
      fieldIds: state.selected,
      mode,
    });
    const names = report.fields.map((name) => name || "(unnamed)").join(", ");
    say(
      report.objectsChanged === 0
        ? `${names} was already in that state — the file was not touched.`
        : `${mode === "clear" ? "Cleared" : "Reset"} ${names} · ` +
            `${report.objectsChanged} object${report.objectsChanged === 1 ? "" : "s"} ` +
            `appended (${report.bytesAppended} bytes)` +
            (report.needAppearancesSet ? " · asked the viewer to redraw" : ""),
    );
    await openFile(state.path, { keepScroll: true });
  } catch (error) {
    say(message(error), true);
  } finally {
    buttons.forEach((button) => (button.disabled = false));
  }
}

/* --------------------------------------------------------------------- glue */

function say(text, isError = false) {
  ui.status.textContent = text;
  ui.status.classList.toggle("error", isError && Boolean(text));
}

function message(error) {
  return typeof error === "string" ? error : (error?.message ?? String(error));
}

async function chooseFile() {
  const path = await openDialog({
    multiple: false,
    filters: [{ name: "PDF", extensions: ["pdf"] }],
  });
  if (typeof path === "string") await openFile(path);
}

ui.open.addEventListener("click", chooseFile);
ui.reset.addEventListener("click", () => applyReset("default"));
ui.clear.addEventListener("click", () => applyReset("clear"));

ui.viewer.addEventListener("click", (event) => {
  // A click on the page but not on a field clears the selection.
  if (!event.target.closest(".widget")) {
    state.selected = [];
    paintSelection();
  }
});

window.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && state.selected.length) {
    state.selected = [];
    paintSelection();
  }
  if (event.key.toLowerCase() === "o" && (event.metaKey || event.ctrlKey)) {
    event.preventDefault();
    chooseFile();
  }
});

// Re-layout on resize, keeping roughly the same place in the document.
let resizeTimer;
window.addEventListener("resize", () => {
  if (!state.path) return;
  clearTimeout(resizeTimer);
  resizeTimer = setTimeout(() => openFile(state.path, { keepScroll: true }), 150);
});

getCurrentWebview().onDragDropEvent((event) => {
  if (event.payload.type !== "drop") return;
  const [path] = event.payload.paths ?? [];
  if (path?.toLowerCase().endsWith(".pdf")) openFile(path);
});
