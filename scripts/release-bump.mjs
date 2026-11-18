#!/usr/bin/env node
/**
 * Release version bump (item 65) — patches every file that carries the app
 * version in one pass, so a release never ships with a stale About dialog.
 *
 * Usage: node scripts/release-bump.mjs --version x.y.z
 *        (without --version the script prompts for one)
 *
 * Files patched:
 *   - package.json                     ("version")
 *   - package-lock.json                (root "version" + packages."".version)
 *   - src-tauri/Cargo.toml             ([package] version = "…")
 *   - src-tauri/Cargo.lock             (the `name = "mygitui"` package section)
 *   - src-tauri/tauri.conf.json        ("version")
 *   - src/lib/components/settings/SettingsDialog.svelte  (FALLBACK_VERSION)
 *   - src/lib/components/AboutDialog.svelte              (FALLBACK_VERSION)
 *
 * The Cargo.lock replace is scoped to the mygitui package block (other
 * crates' versions are left alone; the lockfile fully settles on the next
 * `cargo build`). JSON files are pretty-printed with 2 spaces (matching the
 * existing formatting); TOML/Svelte files get a targeted textual replace.
 *
 * Node-only (no dependencies), like generate-icon-sets.mjs. Prints a
 * per-file summary and hints at the diff review; never commits.
 */
import { readFileSync, writeFileSync } from "node:fs";
import { createInterface } from "node:readline/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const SEMVER = /^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/;

/** Reads `--version x.y.z` from argv (also accepts `--version=x.y.z`). */
function versionFromArgv(argv) {
	for (let i = 0; i < argv.length; i++) {
		const arg = argv[i];
		if (arg === "--version" || arg === "-v") return argv[i + 1];
		if (arg.startsWith("--version=")) return arg.slice("--version=".length);
	}
	return undefined;
}

async function promptVersion() {
	const rl = createInterface({ input: process.stdin, output: process.stdout });
	try {
		return await rl.question("New version (x.y.z): ");
	} finally {
		rl.close();
	}
}

/**
 * Replaces the About fallback constants (`const FALLBACK_VERSION = "…"`,
 * one per dialog). Returns null when nothing matched.
 */
function replaceFallbackVersion(content, nextVersion) {
	const out = content.replace(
		/(const\s+FALLBACK_VERSION\s*=\s*")([^"]+)(")/g,
		(_match, prefix, _oldValue, suffix) => `${prefix}${nextVersion}${suffix}`,
	);
	return out === content ? null : out;
}

/**
 * Replaces the version inside the `name = "mygitui"` package section of
 * Cargo.lock (scoped: every other crate entry stays untouched).
 */
function replaceCargoLock(content, nextVersion) {
	const lines = content.split("\n");
	let inBlock = false;
	let replaced = false;
	for (let i = 0; i < lines.length; i++) {
		const line = lines[i];
		if (line.trim() === "[[package]]") {
			inBlock = true;
			continue;
		}
		if (line.startsWith("[")) {
			inBlock = false;
			continue;
		}
		if (inBlock && line.trim() === 'name = "mygitui"') {
			// The version line follows the name line inside this block.
			for (let j = i + 1; j < lines.length; j++) {
				if (lines[j].startsWith("[")) break;
				const match = lines[j].match(/^(\s*version\s*=\s*")([^"]*)(")\s*$/);
				if (match) {
					lines[j] = `${match[1]}${nextVersion}${match[3]}`;
					replaced = true;
					break;
				}
			}
			inBlock = false;
		}
	}
	return replaced ? lines.join("\n") : null;
}

/** Rewrites `file` when `transform` changed it; returns the old version. */
function patchFile(relative, transform, changed) {
	const path = resolve(repoRoot, relative);
	const before = readFileSync(path, "utf8");
	const after = transform(before);
	if (after === null || after === before) {
		changed.push({ relative, status: "unchanged" });
		return null;
	}
	writeFileSync(path, after, "utf8");
	changed.push({ relative, status: "updated" });
	return before;
}

function oldVersionOf(source) {
	const match = source.match(/"version"\s*:\s*"([^"]+)"/);
	return match ? match[1] : null;
}

// ---------------------------------------------------------------------------
// Run
// ---------------------------------------------------------------------------

const requested = versionFromArgv(process.argv.slice(2)) ?? (await promptVersion());
const version = (requested ?? "").trim();
if (!SEMVER.test(version)) {
	console.error(`error: "${version}" is not a semver version (expected x.y.z).`);
	process.exit(1);
}

const changed = [];

// package.json — the source of truth the other files mirror.
const pkgOld = patchFile(
	"package.json",
	(content) => {
		const parsed = JSON.parse(content);
		const old = parsed.version;
		parsed.version = version;
		return `${JSON.stringify(parsed, null, 2)}\n` !== content
			? JSON.stringify(parsed, null, 2) + (content.endsWith("\n") ? "\n" : "")
			: null;
	},
	changed,
);

// package-lock.json — root "version" AND packages."".version.
patchFile(
	"package-lock.json",
	(content) => {
		const parsed = JSON.parse(content);
		let touched = false;
		if (parsed.version !== undefined && parsed.version !== version) {
			parsed.version = version;
			touched = true;
		}
		if (
			parsed.packages &&
			parsed.packages[""] &&
			parsed.packages[""].version !== version
		) {
			parsed.packages[""].version = version;
			touched = true;
		}
		if (!touched) return null;
		return JSON.stringify(parsed, null, 2) + (content.endsWith("\n") ? "\n" : "");
	},
	changed,
);

// src-tauri/Cargo.toml — the [package] section's version line.
patchFile(
	"src-tauri/Cargo.toml",
	(content) => {
		const lines = content.split("\n");
		let inPackage = false;
		let hit = false;
		for (let i = 0; i < lines.length; i++) {
			if (lines[i].trim() === "[package]") {
				inPackage = true;
				continue;
			}
			if (lines[i].startsWith("[")) {
				inPackage = false;
				continue;
			}
			if (inPackage && /^\s*version\s*=\s*"/.test(lines[i])) {
				lines[i] = lines[i].replace(
					/^(\s*version\s*=\s*")([^"]*)(")/,
					`$1${version}$3`,
				);
				hit = true;
				break;
			}
		}
		return hit ? lines.join("\n") : null;
	},
	changed,
);

// src-tauri/Cargo.lock — scoped to the mygitui package block.
patchFile("src-tauri/Cargo.lock", (c) => replaceCargoLock(c, version), changed);

// src-tauri/tauri.conf.json — the app "version" field.
patchFile(
	"src-tauri/tauri.conf.json",
	(content) => {
		const parsed = JSON.parse(content);
		if (parsed.version === version) return null;
		parsed.version = version;
		return JSON.stringify(parsed, null, 2) + (content.endsWith("\n") ? "\n" : "");
	},
	changed,
);

// The About fallback constants (tolerant: `const FALLBACK_VERSION = "…"` in
// either dialog — SettingsDialog and AboutDialog each carry one).
for (const relative of [
	"src/lib/components/settings/SettingsDialog.svelte",
	"src/lib/components/AboutDialog.svelte",
]) {
	patchFile(relative, (content) => replaceFallbackVersion(content, version), changed);
}

// -- summary ------------------------------------------------------------------

const pkgBefore =
	pkgOld === null
		? JSON.parse(readFileSync(resolve(repoRoot, "package.json"), "utf8")).version
		: oldVersionOf(pkgOld);

console.log(`release-bump: ${pkgBefore ?? "?"} -> ${version}`);
let updated = 0;
for (const entry of changed) {
	console.log(`  ${entry.status.padEnd(10)} ${entry.relative}`);
	if (entry.status === "updated") updated++;
}
if (updated === 0) {
	console.log("no files changed — every version already matched.");
} else {
	console.log(
		`\n${updated} file(s) updated. Review with:\n  git diff --stat\n  git diff`,
	);
	console.log(
		"next: run the test suite, then build once (cargo settles Cargo.lock) and tag.",
	);
}
