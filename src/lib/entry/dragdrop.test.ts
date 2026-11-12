import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { pathsFromDataTransfer } from "$lib/entry/dragdrop";

/**
 * `pathsFromDataTransfer` turns a file drop into repository paths.
 * Only `getData` + `files` are read, so a plain object stands in for the
 * real DataTransfer (vitest runs in node — no DOM DataTransfer).
 */
function fakeTransfer(options: {
  uriList?: string;
  files?: Array<{ path?: unknown; name?: string }>;
}): DataTransfer {
  return {
    getData: (type: string) => (type === "text/uri-list" ? (options.uriList ?? "") : ""),
    files: (options.files ?? []) as unknown as FileList,
  } as unknown as DataTransfer;
}

describe("pathsFromDataTransfer", () => {
  it("extracts file:// URIs (Linux/WebKitGTK file drops)", () => {
    expect(
      pathsFromDataTransfer(
        fakeTransfer({ uriList: "file:///home/me/projects/app\n" }),
      ),
    ).toEqual(["/home/me/projects/app"]);
  });

  it("decodes URL-encoded path segments", () => {
    expect(
      pathsFromDataTransfer(fakeTransfer({ uriList: "file:///tmp/my%20repo" })),
    ).toEqual(["/tmp/my repo"]);
  });

  it("skips comments, blank lines and remote URIs", () => {
    expect(
      pathsFromDataTransfer(
        fakeTransfer({
          uriList:
            "# some comment\n\nhttps://example.com/x\nfile:///tmp/repo\nsmb://host/share",
        }),
      ),
    ).toEqual(["/tmp/repo"]);
  });

  it("falls back to File.path (Windows/WebView2 file drops)", () => {
    expect(
      pathsFromDataTransfer(fakeTransfer({ files: [{ path: "C:\\repos\\app" }] })),
    ).toEqual(["C:\\repos\\app"]);
  });

  it("ignores Files without a path (platforms that expose none)", () => {
    expect(pathsFromDataTransfer(fakeTransfer({ files: [{ name: "app" }] }))).toEqual(
      [],
    );
  });

  it("merges both sources and deduplicates", () => {
    expect(
      pathsFromDataTransfer(
        fakeTransfer({
          uriList: "file:///tmp/repo\nfile:///tmp/other",
          files: [{ path: "/tmp/repo" }, { path: "/tmp/third" }],
        }),
      ),
    ).toEqual(["/tmp/repo", "/tmp/other", "/tmp/third"]);
  });

  it("returns [] for null and for drags without files", () => {
    expect(pathsFromDataTransfer(null)).toEqual([]);
    expect(pathsFromDataTransfer(fakeTransfer({}))).toEqual([]);
  });

  it("the main window runs with dragDropEnabled disabled", () => {
    // Tauri's native drag-drop handler (the default) swallows every HTML5
    // drag event in the webview — repo tab reordering and file-drop repo
    // opening both depend on `false` here. Guards against regression.
    const conf = JSON.parse(
      readFileSync(resolve(process.cwd(), "src-tauri/tauri.conf.json"), "utf8"),
    ) as { app: { windows: Array<Record<string, unknown>> } };
    for (const win of conf.app.windows) {
      expect(win["dragDropEnabled"]).toBe(false);
    }
  });
});
