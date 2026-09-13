// pdf.js loads character maps, standard font data, ICC profiles and its wasm
// decoders at runtime rather than through the bundler, so they have to sit in
// the app's own assets. Copied from node_modules on every build so the version
// can never drift from the one in package-lock.json.
import { cpSync, mkdirSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const from = join(root, "node_modules", "pdfjs-dist");
const to = join(root, "public", "pdfjs");

rmSync(to, { recursive: true, force: true });
mkdirSync(to, { recursive: true });
for (const asset of ["cmaps", "standard_fonts", "wasm", "iccs"]) {
  cpSync(join(from, asset), join(to, asset), { recursive: true });
}
console.log(`pdf.js assets copied to ${to}`);
