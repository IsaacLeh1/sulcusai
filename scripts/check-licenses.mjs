// SPDX-License-Identifier: AGPL-3.0-only
// Fails if a production npm dependency uses a license outside the allow list.
// Usage: pnpm check:licenses
import { execSync } from "node:child_process";

const ALLOWED = new Set([
  "MIT", "MIT-0", "ISC", "0BSD", "BSD-2-Clause", "BSD-3-Clause", "Apache-2.0",
  "Zlib", "CC0-1.0", "BlueOak-1.0.0", "Unlicense", "Python-2.0", "MPL-2.0",
  "(MIT OR Apache-2.0)", "MIT OR Apache-2.0", "Apache-2.0 OR MIT", "(MIT OR CC0-1.0)",
]);

const raw = execSync("pnpm licenses list --prod --json", { encoding: "utf8" });
const byLicense = JSON.parse(raw);
const bad = [];
for (const [license, pkgs] of Object.entries(byLicense)) {
  if (ALLOWED.has(license)) continue;
  for (const p of pkgs) bad.push(`${p.name}@${(p.versions ?? [p.version]).join(",")}: ${license}`);
}

const count = Object.values(byLicense).reduce((n, pkgs) => n + pkgs.length, 0);
if (bad.length) {
  console.error(`Disallowed licenses found:\n  ${bad.join("\n  ")}`);
  process.exit(1);
}
console.log(`All ${count} production npm packages use allowed licenses.`);
