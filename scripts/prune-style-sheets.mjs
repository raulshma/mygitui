/**
 * Prunes the vendored shadcn-svelte style sheets (src/lib/theme/styles) down
 * to the components this app actually applies.
 *
 * The sheets are vendored verbatim from upstream, so they define utilities
 * for every registry component (~441 selectors) while the app only applies
 * ~190. The rest is dead weight: parsed at startup (nova ships with app.css)
 * and on every style switch (the other seven are lazy chunks).
 *
 * A top-level rule block survives when any cn-* class named anywhere inside
 * it (its selector or nested rules) appears verbatim in an app source file
 * (.svelte/.ts) — the markers are always literal strings in component
 * sources and the menu-color engine, never built dynamically. Section
 * comments (`MARK: …`) die with the section they describe; rules without any
 * cn-* class are always kept.
 *
 * Idempotent; run after re-vendoring an upstream sheet:
 *   npm run prune:styles
 *
 * src/lib/theme/styles/sheets-coverage.test.ts guards the invariant in both
 * directions: every applied marker keeps its rule, and the pruner stays a
 * no-op on an already-pruned tree.
 */
import { readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const rootDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const srcDir = path.join(rootDir, "src");
const stylesDir = path.join(srcDir, "lib", "theme", "styles");

/** Every cn-* marker applied anywhere in app sources (.svelte/.ts). */
function collectUsedTokens() {
	const used = new Set();
	const walk = (dir) => {
		for (const entry of readdirSync(dir)) {
			const full = path.join(dir, entry);
			if (statSync(full).isDirectory()) walk(full);
			else if (/\.(svelte|ts)$/.test(entry)) {
				for (const match of readFileSync(full, "utf8").matchAll(/cn-[a-z0-9-]+/g)) {
					used.add(match[0]);
				}
			}
		}
	};
	walk(srcDir);
	return used;
}

/** Splits a rule-block body into its top-level blocks plus trailing text. */
function topLevelBlocks(body) {
	const segments = [];
	let depth = 0;
	let segmentStart = 0;
	for (let i = 0; i < body.length; i++) {
		const ch = body[i];
		if (ch === "{") depth += 1;
		else if (ch === "}") {
			depth -= 1;
			if (depth === 0) {
				segments.push(body.slice(segmentStart, i + 1));
				segmentStart = i + 1;
			}
		}
	}
	return { segments, trailing: body.slice(segmentStart) };
}

function pruneSheet(file, used) {
	const css = readFileSync(file, "utf8");
	// Baseline of markers this sheet carried before pruning — sheets differ
	// upstream (some legitimately lack markers other styles define), so the
	// safety net only guards against losing what was there.
	const appliedTokens = new Set(css.match(/cn-[a-z0-9-]+/g).filter((token) => used.has(token)));
	const open = css.indexOf("{");
	const close = css.lastIndexOf("}");
	const head = css.slice(0, open + 1);
	const tail = css.slice(close);
	const { segments, trailing } = topLevelBlocks(css.slice(open + 1, close));

	const kept = [];
	let pruned = 0;
	for (const segment of segments) {
		// Match the whole segment, not just the top selector: some styles nest
		// rules for other markers inside a component block, and a nested
		// applied marker must keep its enclosing block alive.
		const tokens = segment.match(/cn-[a-z0-9-]+/g);
		if (!tokens || tokens.some((token) => used.has(token))) {
			kept.push(segment);
		} else {
			pruned += 1;
		}
	}

	const body = kept.join("").replace(/^\s+/, "");
	if (trailing.trim()) {
		throw new Error(`${path.basename(file)}: unexpected content after the last rule block`);
	}

	// Safety net, checked on the in-memory result BEFORE writing: the pruned
	// sheet must still name every applied marker it carried before pruning.
	// Writing first would leave a damaged sheet on disk whose pruned state
	// then becomes the next run's baseline, so the lost rules cannot return.
	const text = `${head}\n${body}${tail}`;
	const missing = [...appliedTokens].filter((token) => !text.includes(token));
	if (missing.length > 0) {
		throw new Error(`${path.basename(file)}: pruned away applied markers: ${missing.join(" ")}`);
	}
	writeFileSync(file, text);
	return { total: segments.length, kept: kept.length, pruned };
}

const used = collectUsedTokens();
const sheets = readdirSync(stylesDir)
	.filter((name) => /^style-.+\.css$/.test(name))
	.sort();

for (const name of sheets) {
	const stats = pruneSheet(path.join(stylesDir, name), used);
	const kb = ((statSync(path.join(stylesDir, name)).size / 1024) | 0) + "KB";
	console.log(`${name}: kept ${stats.kept}/${stats.total} rules (pruned ${stats.pruned}), now ${kb}`);
}
