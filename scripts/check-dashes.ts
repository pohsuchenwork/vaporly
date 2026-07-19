// Dash policy guard: no em dashes and no en dashes anywhere we write.
// Use commas, colons, periods, or plain hyphens instead.
import fs from "fs";
import path from "path";

const ROOT = path.join(path.dirname(new URL(import.meta.url).pathname), "..");
const EXTS = new Set([
  ".md",
  ".ts",
  ".tsx",
  ".rs",
  ".json",
  ".sh",
  ".yml",
  ".yaml",
  ".html",
  ".css",
  ".nsi",
  ".nix",
  ".py",
  ".rb",
  ".ps1",
]);
const SKIP_DIRS = new Set([
  "node_modules",
  "target",
  "dist",
  ".git",
  "resources",
]);
const SKIP_FILES = new Set([
  "bun.lock",
  "Cargo.lock",
  "LICENSE.llama.cpp",
  "check-dashes.ts",
]);

const offenders: string[] = [];

function walk(dir: string) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      if (!SKIP_DIRS.has(entry.name)) walk(path.join(dir, entry.name));
      continue;
    }
    if (SKIP_FILES.has(entry.name)) continue;
    if (!EXTS.has(path.extname(entry.name))) continue;
    const p = path.join(dir, entry.name);
    const content = fs.readFileSync(p, "utf8");
    const lines = content.split("\n");
    lines.forEach((line, i) => {
      if (line.includes("—") || line.includes("–")) {
        offenders.push(
          `${path.relative(ROOT, p)}:${i + 1}: ${line.trim().slice(0, 90)}`,
        );
      }
    });
  }
}

walk(ROOT);

if (offenders.length > 0) {
  console.error(
    `Dash policy violation: ${offenders.length} line(s) contain an em or en dash:\n`,
  );
  offenders.slice(0, 40).forEach((o) => console.error("  " + o));
  if (offenders.length > 40)
    console.error(`  ... and ${offenders.length - 40} more`);
  process.exit(1);
}
console.log("No em or en dashes found. Policy holds.");
