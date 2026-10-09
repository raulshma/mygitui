/**
 * Unit tests for the popout query contract (`popout.ts`): string building
 * + parsing round trips, including the `commitdetail` panel's `sha`
 * requirement. Window creation itself (`openPanelPopout`) needs a Tauri
 * runtime and is covered by manual/e2e paths instead.
 */

import { describe, expect, it } from "vitest";
import { parsePopoutQuery, popoutQueryString } from "$lib/layout/popout";

describe("popoutQueryString", () => {
  it("appends the sha for commitdetail panels", () => {
    expect(popoutQueryString("commitdetail", "repo1", { sha: "abc123" })).toBe(
      "?panel=commitdetail&repo=repo1&sha=abc123",
    );
  });

  it("encodes ids and shas", () => {
    expect(popoutQueryString("commitdetail", "a b", { sha: "c/d" })).toBe(
      "?panel=commitdetail&repo=a%20b&sha=c%2Fd",
    );
  });

  it("omits empty optional params", () => {
    expect(popoutQueryString("history", "repo1")).toBe("?panel=history&repo=repo1");
    expect(popoutQueryString("commitdetail", "repo1", { sha: "" })).toBe(
      "?panel=commitdetail&repo=repo1",
    );
  });

  it("keeps path for filehistory panels", () => {
    expect(popoutQueryString("filehistory", "repo1", { path: "src/a.ts" })).toBe(
      "?panel=filehistory&repo=repo1&path=src%2Fa.ts",
    );
  });
});

describe("parsePopoutQuery", () => {
  it("round trips every panel kind", () => {
    for (const panel of ["diff", "history", "filehistory", "commitdetail"] as const) {
      const query = popoutQueryString(panel, "repo 1", {
        path: panel === "filehistory" ? "src/a b.ts" : undefined,
        sha: panel === "commitdetail" ? "deadbeef" : undefined,
      });
      expect(parsePopoutQuery(query)).toEqual({
        panel,
        repoId: "repo 1",
        path: panel === "filehistory" ? "src/a b.ts" : null,
        sha: panel === "commitdetail" ? "deadbeef" : null,
      });
    }
  });

  it("rejects non-popout searches", () => {
    expect(parsePopoutQuery("")).toBeNull();
    expect(parsePopoutQuery("?other=1")).toBeNull();
    expect(parsePopoutQuery("?panel=unknown&repo=r")).toBeNull();
    expect(parsePopoutQuery("?panel=history")).toBeNull(); // no repo
  });

  it("requires path for filehistory", () => {
    expect(parsePopoutQuery("?panel=filehistory&repo=r")).toBeNull();
  });

  it("requires sha for commitdetail", () => {
    expect(parsePopoutQuery("?panel=commitdetail&repo=r")).toBeNull();
    expect(parsePopoutQuery("?panel=commitdetail&repo=r&sha=")).toBeNull();
    expect(parsePopoutQuery("?panel=commitdetail&repo=r&sha=abc")).toEqual({
      panel: "commitdetail",
      repoId: "r",
      path: null,
      sha: "abc",
    });
  });

  it("accepts a search without the leading question mark", () => {
    expect(parsePopoutQuery("panel=commitdetail&repo=r&sha=abc")?.sha).toBe("abc");
  });
});
