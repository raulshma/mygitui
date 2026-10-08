/**
 * Unit tests for the gitignore quick-add pattern helpers (gitignore.ts):
 * exact / extension / directory candidates from one changed path.
 */

import { describe, expect, it } from "vitest";
import { patternFor } from "./gitignore";

describe("patternFor", () => {
  it("offers exact, extension and directory patterns for nested files", () => {
    expect(patternFor("src/tmp/scratch.log")).toEqual({
      exact: "src/tmp/scratch.log",
      ext: "*.log",
      dir: "src/tmp/",
    });
  });

  it("treats root-level files as having no directory option", () => {
    expect(patternFor("notes.txt")).toEqual({
      exact: "notes.txt",
      ext: "*.txt",
      dir: "",
    });
  });

  it("offers no extension pattern for extension-less files", () => {
    expect(patternFor("bin/app")).toEqual({
      exact: "bin/app",
      ext: "",
      dir: "bin/",
    });
  });

  it("treats dotfiles as having no extension", () => {
    // `.gitignore` is a name, not extension `.gitignore`.
    expect(patternFor("config/.env")).toEqual({
      exact: "config/.env",
      ext: "",
      dir: "config/",
    });
    expect(patternFor("Dockerfile")).toEqual({
      exact: "Dockerfile",
      ext: "",
      dir: "",
    });
  });

  it("keeps dotfile extensions like .d.ts as a real extension", () => {
    expect(patternFor("dist/index.d.ts").ext).toBe("*.ts");
  });

  it("normalizes Windows separators and ./ prefixes", () => {
    expect(patternFor("src\\gen\\cache.tmp")).toEqual({
      exact: "src/gen/cache.tmp",
      ext: "*.tmp",
      dir: "src/gen/",
    });
    expect(patternFor("./output.txt").exact).toBe("output.txt");
  });

  it("degrades gracefully on empty input", () => {
    expect(patternFor("")).toEqual({ exact: "", ext: "", dir: "" });
    expect(patternFor("   ")).toEqual({ exact: "", ext: "", dir: "" });
  });
});
