// Cross-check the reset against a different PDF implementation.
//
// The Rust tests prove the object graph says what it should. This asks the
// library the app actually renders with — pdf.js — what it sees in the file
// before and after, which is the thing a user ends up looking at.
//
//     npm run check
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const fixture = join(root, "crates", "pdf-form-core", "tests", "assets", "filled_form.pdf");
const pdfjs = await import("pdfjs-dist/legacy/build/pdf.mjs");

async function widgetsOf(path) {
  const task = pdfjs.getDocument({
    data: new Uint8Array(readFileSync(path)),
    standardFontDataUrl: join(root, "node_modules", "pdfjs-dist", "standard_fonts") + "/",
  });
  const document = await task.promise;

  const widgets = [];
  for (let number = 1; number <= document.numPages; number += 1) {
    const page = await document.getPage(number);
    for (const annotation of await page.getAnnotations()) {
      if (annotation.subtype !== "Widget") continue;
      widgets.push({
        page: number,
        name: annotation.fieldName,
        type: annotation.fieldType,
        value: annotation.fieldValue ?? null,
        exportValue: annotation.buttonValue ?? null,
        // A button is on when the group's value names this widget's own
        // export value. pdf.js reports a field with no /V as its /DV, which is
        // exactly the fallback a cleared field has to defeat.
        on:
          annotation.checkBox || annotation.radioButton
            ? annotation.fieldValue === annotation.buttonValue
            : null,
      });
    }
  }
  await task.destroy();
  return widgets;
}

function cli(...args) {
  return execFileSync(
    "cargo",
    ["run", "-q", "-p", "pdf-form-core", "--example", "reset", "--", ...args],
    { cwd: root, encoding: "utf8" },
  ).trim();
}

const failures = [];
function check(what, actual, expected) {
  const ok = JSON.stringify(actual) === JSON.stringify(expected);
  console.log(`${ok ? "  ok  " : " FAIL "} ${what}`);
  if (!ok) {
    failures.push(what);
    console.log(`        expected ${JSON.stringify(expected)}`);
    console.log(`        actual   ${JSON.stringify(actual)}`);
  }
}

const work = mkdtempSync(join(tmpdir(), "pdf-form-resetter-"));
const file = join(work, "form.pdf");
copyFileSync(fixture, file);

const before = await widgetsOf(file);
const radio = (widgets) => widgets.filter((widget) => widget.name === "choice");

console.log("as produced:");
check(
  "pdf.js sees the radio group set to beta",
  radio(before).map((widget) => [widget.exportValue, widget.on]),
  [
    ["alpha", false],
    ["beta", true],
    ["gamma", false],
  ],
);

console.log(cli("reset", file, "choice"));
const after = await widgetsOf(file);

console.log("after resetting the radio group:");
check(
  "pdf.js sees no button on",
  radio(after).map((widget) => [widget.exportValue, widget.on]),
  [
    ["alpha", false],
    ["beta", false],
    ["gamma", false],
  ],
);
check(
  "the group holds no value",
  [...new Set(radio(after).map((widget) => widget.value))],
  [null],
);
check(
  "every other field is exactly as it was",
  after.filter((widget) => widget.name !== "choice"),
  before.filter((widget) => widget.name !== "choice"),
);

console.log(cli("clear", file, "fullname"));
const cleared = await widgetsOf(file);
check(
  "the text field reads as empty",
  cleared.find((widget) => widget.name === "fullname").value,
  "",
);
check(
  "and the fields around it did not move",
  cleared.filter((widget) => !["choice", "fullname"].includes(widget.name)),
  before.filter((widget) => !["choice", "fullname"].includes(widget.name)),
);

console.log(
  failures.length ? `\n${failures.length} check(s) failed` : "\nall checks passed",
);
process.exit(failures.length ? 1 : 0);
