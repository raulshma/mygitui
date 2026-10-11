/**
 * Unit tests for fileIcons.ts: resolution precedence (exact filename →
 * `.env*` prefix → extension → fallback), case-insensitivity, separator
 * handling, and the per-basename memoization.
 */

import { describe, expect, it } from "vitest";
import {
  defaultFileIcon,
  fileIconForPath,
} from "./fileIcons";

describe("fileIconForPath", () => {
  it("falls back to the generic document icon for unknown paths", () => {
    expect(fileIconForPath("src/thing.weird")).toBe(defaultFileIcon);
    expect(fileIconForPath("Makefile2")).toBe(defaultFileIcon);
  });

  it("maps extensions, case-insensitively", () => {
    expect(fileIconForPath("src/app.ts")).not.toBe(defaultFileIcon);
    expect(fileIconForPath("src/app.ts")).toBe(fileIconForPath("APP.TS"));
    expect(fileIconForPath("a/readme.MD")).toBe(fileIconForPath("b/readme.md"));
  });

  it("splits on both path separators", () => {
    expect(fileIconForPath("C:\\repo\\src\\app.rs")).toBe(
      fileIconForPath("repo/src/app.rs"),
    );
  });

  it("prefers exact filenames over their extension", () => {
    // package.json would otherwise resolve via the json extension.
    expect(fileIconForPath("package.json")).toBe(
      fileIconForPath("packages/pkg/package.json"),
    );
    expect(fileIconForPath(".gitignore")).toBe(
      fileIconForPath("sub/.gitignore"),
    );
    expect(fileIconForPath("Dockerfile")).toBe(fileIconForPath("a/Dockerfile"));
    // Cargo.toml resolves as a name, not the generic config extension.
    expect(fileIconForPath("Cargo.toml")).toBe(
      fileIconForPath("src/lib.rs"),
    );
  });

  it("matches .env files and prefixed variants", () => {
    expect(fileIconForPath(".env")).toBe(fileIconForPath(".env.local"));
    expect(fileIconForPath(".env.production")).not.toBe(defaultFileIcon);
  });

  it("returns the basename's icon regardless of directory (memoized)", () => {
    const first = fileIconForPath("a/one/index.ts");
    const second = fileIconForPath("b/two/index.ts");
    expect(second).toBe(first);
  });

  it("treats dotfiles as names, not extensions", () => {
    // ".hidden" has no meaningful extension; unknown → document fallback.
    expect(fileIconForPath(".hidden")).toBe(defaultFileIcon);
  });
});
