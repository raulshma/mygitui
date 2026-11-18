/**
 * Tests for the favorite-repos store (`$lib/stores/favoriteRepos.svelte`):
 * parse validation, add/remove/toggle semantics, path-normalized dedupe,
 * the launch list cap, and the versioned-envelope localStorage round-trip
 * (a fresh store restores the persisted favorite order).
 */
import { beforeEach, describe, expect, it } from "vitest";
import {
  FAVORITES_LAUNCH_CAP,
  FAVORITE_REPOS_KEY,
  FavoriteReposStore,
  parseFavoriteRepos,
  favoriteRepos,
} from "./favoriteRepos.svelte";

beforeEach(() => {
  localStorage.clear();
  favoriteRepos.clear();
});

describe("parseFavoriteRepos", () => {
  it("returns null for non-array payloads", () => {
    expect(parseFavoriteRepos(null)).toBeNull();
    expect(parseFavoriteRepos(undefined)).toBeNull();
    expect(parseFavoriteRepos({})).toBeNull();
    expect(parseFavoriteRepos("x")).toBeNull();
    expect(parseFavoriteRepos(42)).toBeNull();
  });

  it("keeps non-empty strings in order and drops blanks/duplicates", () => {
    expect(
      parseFavoriteRepos([
        "/a/repo",
        "",
        42,
        null,
        "/b/repo",
        "/a/repo",
        "/A/repo", // case variant of /a/repo: the normalized key dedupes
      ]),
    ).toEqual(["/a/repo", "/b/repo"]);
  });
});

describe("FavoriteReposStore", () => {
  it("adds favorites newest-first and persists the envelope", () => {
    const store = new FavoriteReposStore();
    expect(store.add("/a/repo")).toBe(true);
    expect(store.add("/b/repo")).toBe(true);
    expect(store.favorites).toEqual(["/b/repo", "/a/repo"]);
    const raw = JSON.parse(localStorage.getItem(FAVORITE_REPOS_KEY)!);
    expect(raw).toEqual({ v: 1, data: ["/b/repo", "/a/repo"] });
  });

  it("refuses duplicates regardless of separator/case spelling", () => {
    const store = new FavoriteReposStore();
    store.add("/a/repo");
    expect(store.add("/a/repo")).toBe(false);
    expect(store.add("/a//repo/")).toBe(false);
    expect(store.favorites).toEqual(["/a/repo"]);
  });

  it("has() matches path-normalized", () => {
    const store = new FavoriteReposStore();
    store.add("/a/repo");
    expect(store.has("/a/repo")).toBe(true);
    expect(store.has("/a/repo/")).toBe(true);
    expect(store.has("/a/other")).toBe(false);
  });

  it("removes and toggles, reporting the new state", () => {
    const store = new FavoriteReposStore();
    expect(store.remove("/a/repo")).toBe(false);
    expect(store.toggle("/a/repo")).toBe(true);
    expect(store.toggle("/a/repo")).toBe(false);
    expect(store.favorites).toEqual([]);
    // The toggle-off persists the emptied envelope (not a removed key) —
    // an empty payload still loads back as no favorites.
    expect(JSON.parse(localStorage.getItem(FAVORITE_REPOS_KEY)!)).toEqual({
      v: 1,
      data: [],
    });
  });

  it("launchList caps at FAVORITES_LAUNCH_CAP in star order", () => {
    const store = new FavoriteReposStore();
    for (let i = 0; i < FAVORITES_LAUNCH_CAP + 2; i++) {
      store.add(`/r${i}`);
    }
    // Newest star first: r6 is the last add, so it leads.
    expect(store.launchList()).toHaveLength(FAVORITES_LAUNCH_CAP);
    expect(store.launchList()[0]).toBe(`/r${FAVORITES_LAUNCH_CAP + 1}`);
    expect(store.launchList(2)).toEqual([`/r${FAVORITES_LAUNCH_CAP + 1}`, `/r${FAVORITES_LAUNCH_CAP}`]);
  });

  it("a fresh store restores persisted order from storage", () => {
    localStorage.setItem(
      FAVORITE_REPOS_KEY,
      JSON.stringify({ v: 1, data: ["/first", "/second"] }),
    );
    const store = new FavoriteReposStore();
    expect(store.favorites).toEqual(["/first", "/second"]);
    expect(store.has("/second")).toBe(true);
  });

  it("clear() empties memory and storage", () => {
    const store = new FavoriteReposStore();
    store.add("/a");
    store.clear();
    expect(store.favorites).toEqual([]);
    expect(localStorage.getItem(FAVORITE_REPOS_KEY)).toBeNull();
  });
});
