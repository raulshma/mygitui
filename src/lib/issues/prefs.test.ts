import { describe, expect, it } from "vitest";
import {
  IssueViewPrefsStore,
  ISSUE_VIEW_PREFS_PREFIX,
  defaultIssueViewPreferences,
  issueViewPrefsKey,
  loadIssueViewPreferences,
  normalizeIssueRoot,
  parseIssueViewPreferences,
  saveIssueViewPreferences,
} from "./prefs";
import { DEFAULT_ISSUE_QUERY } from "./query";
import { MemoryStorage } from "./testFixtures";

describe("Issues view preferences", () => {
  it("normalizes root keys across separators, case, and trailing slashes", () => {
    expect(normalizeIssueRoot("C:\\Repos\\Demo\\")).toBe("c:/repos/demo");
    expect(issueViewPrefsKey("C:/Repos/Demo")).toBe(issueViewPrefsKey("c:\\repos\\demo\\"));
    expect(issueViewPrefsKey("")).toBe(ISSUE_VIEW_PREFS_PREFIX);
  });

  it("re-defaults malformed payloads and clamps valid values", () => {
    expect(parseIssueViewPreferences(null)).toBeNull();
    expect(parseIssueViewPreferences("bad")).toBeNull();
    expect(parseIssueViewPreferences({ query: "bad", splitRatio: 5 })).toEqual({
      query: DEFAULT_ISSUE_QUERY,
      splitRatio: 0.85,
    });
    expect(defaultIssueViewPreferences().query).toEqual(DEFAULT_ISSUE_QUERY);
  });

  it("round-trips through the versioned envelope", () => {
    const storage = new MemoryStorage();
    saveIssueViewPreferences("C:\\Repo", {
      query: { ...DEFAULT_ISSUE_QUERY, state: "closed", per_page: 50, search: "needle" },
      splitRatio: 0.42,
    }, storage);
    expect(storage.getItem(issueViewPrefsKey("c:/repo"))).toContain('"v":1');
    expect(loadIssueViewPreferences("c:/repo/", storage)).toEqual({
      query: { ...DEFAULT_ISSUE_QUERY, state: "closed", per_page: 50, search: "needle" },
      splitRatio: 0.42,
    });
  });

  it("drops unknown fields instead of copying untrusted persisted data", () => {
    expect(parseIssueViewPreferences({
      query: { ...DEFAULT_ISSUE_QUERY, token: "secret", body: "secret" },
      splitRatio: 0.5,
      token: "secret",
    })).toEqual({ query: DEFAULT_ISSUE_QUERY, splitRatio: 0.5 });
  });

  it("updates one preference without clobbering the other and evicts memory only", () => {
    const storage = new MemoryStorage();
    const store = new IssueViewPrefsStore(storage);
    store.setQuery("repo", { state: "all", labels: ["bug"] });
    store.setSplitRatio("repo", 0.4);
    expect(store.read("repo").query.state).toBe("all");
    expect(store.read("repo").splitRatio).toBe(0.4);
    store.evict("repo");
    expect(store.read("repo").query.state).toBe("all");
    store.reset("repo");
    expect(store.read("repo")).toEqual(defaultIssueViewPreferences());
  });
});
