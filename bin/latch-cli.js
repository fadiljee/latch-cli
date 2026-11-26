#!/usr/bin/env node
"use strict";

const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

const rootDir = path.resolve(__dirname, "..");
const candidates = [
  path.join(rootDir, "bin", "native", "latch-cli"),
  path.join(rootDir, "target", "release", "latch-cli"),
];

let binPath = candidates.find((p) => fs.existsSync(p));

if (!binPath) {
  const installer = path.join(rootDir, "scripts", "npm-install.js");
  const installRes = spawnSync(process.execPath, [installer], { stdio: "inherit" });
  if (installRes.status === 0) {
    binPath = candidates.find((p) => fs.existsSync(p));
  }
}

if (!binPath) {
  console.error(
    "latch-cli binary not found. Ensure a GitHub Release exists for your platform or build locally with `cargo build --release -p latch-cli`."
  );
  process.exit(1);
}

const result = spawnSync(binPath, process.argv.slice(2), { stdio: "inherit" });
if (result.error) {
  console.error(`Failed to execute latch-cli: ${result.error.message}`);
  process.exit(1);
}
if (result.signal) {
  process.kill(process.pid, result.signal);
}
process.exit(result.status ?? 0);
