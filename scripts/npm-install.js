#!/usr/bin/env node
"use strict";

const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const https = require("node:https");
const path = require("node:path");

const rootDir = path.resolve(__dirname, "..");
const pkg = JSON.parse(fs.readFileSync(path.join(rootDir, "package.json"), "utf8"));
const nativeDir = path.join(rootDir, "bin", "native");
const destBin = path.join(nativeDir, "latch-cli");
const localReleaseBin = path.join(rootDir, "target", "release", "latch-cli");

function targetTriple() {
  const key = `${process.platform}-${process.arch}`;
  const map = {
    "linux-x64": "x86_64-unknown-linux-musl",
    "linux-arm64": "aarch64-unknown-linux-musl",
    "darwin-x64": "x86_64-apple-darwin",
    "darwin-arm64": "aarch64-apple-darwin",
  };
  return map[key] || null;
}

function downloadFile(url, outPath, redirects = 0) {
  return new Promise((resolve, reject) => {
    if (redirects > 5) {
      return reject(new Error("Too many redirects"));
    }
    https
      .get(url, { headers: { "User-Agent": "latch-runtime-installer" } }, (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          res.resume();
          return resolve(downloadFile(res.headers.location, outPath, redirects + 1));
        }
        if (res.statusCode !== 200) {
          res.resume();
          return reject(new Error(`HTTP ${res.statusCode} for ${url}`));
        }
        const file = fs.createWriteStream(outPath);
        res.pipe(file);
        file.on("finish", () => file.close(() => resolve()));
        file.on("error", reject);
      })
      .on("error", reject);
  });
}

function tryLocalCargoBuild() {
  if (fs.existsSync(localReleaseBin)) {
    fs.mkdirSync(nativeDir, { recursive: true });
    fs.copyFileSync(localReleaseBin, destBin);
    fs.chmodSync(destBin, 0o755);
    return true;
  }
  if (!fs.existsSync(path.join(rootDir, "Cargo.toml"))) {
    return false;
  }
  const cargoCheck = spawnSync("cargo", ["--version"], { stdio: "ignore" });
  if (cargoCheck.status !== 0) {
    return false;
  }
  console.log("latch-runtime: building latch-cli from source with cargo...");
  const build = spawnSync("cargo", ["build", "--release", "-p", "latch-cli"], {
    cwd: rootDir,
    stdio: "inherit",
  });
  if (build.status === 0 && fs.existsSync(localReleaseBin)) {
    fs.mkdirSync(nativeDir, { recursive: true });
    fs.copyFileSync(localReleaseBin, destBin);
    fs.chmodSync(destBin, 0o755);
    return true;
  }
  return false;
}

async function main() {
  if (fs.existsSync(destBin)) {
    return;
  }

  // If developing locally with an existing Cargo release binary, copy it directly.
  if (fs.existsSync(localReleaseBin)) {
    fs.mkdirSync(nativeDir, { recursive: true });
    fs.copyFileSync(localReleaseBin, destBin);
    fs.chmodSync(destBin, 0o755);
    return;
  }

  const triple = targetTriple();
  if (!triple) {
    console.warn(
      `latch-runtime: unsupported platform ${process.platform}-${process.arch} (Linux/macOS supported; use WSL2 on Windows).`
    );
    return;
  }

  const asset = `latch-cli-${triple}.tar.gz`;
  const url = `https://github.com/fadiljee/latch-cli/releases/download/v${pkg.version}/${asset}`;
  fs.mkdirSync(nativeDir, { recursive: true });
  const archivePath = path.join(nativeDir, asset);

  try {
    await downloadFile(url, archivePath);
    const untar = spawnSync("tar", ["-xzf", archivePath, "-C", nativeDir], {
      stdio: "inherit",
    });
    fs.rmSync(archivePath, { force: true });
    if (untar.status !== 0 || !fs.existsSync(destBin)) {
      throw new Error("Failed to extract latch-cli binary");
    }
    fs.chmodSync(destBin, 0o755);
  } catch (err) {
    fs.rmSync(archivePath, { force: true });
    if (tryLocalCargoBuild()) {
      return;
    }
    console.warn(`latch-runtime: prebuilt binary unavailable (${err.message}).`);
    console.warn("Install Rust and run `cargo build --release -p latch-cli` to build locally.");
  }
}

main();
