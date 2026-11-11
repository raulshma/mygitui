import { beforeEach, describe, expect, it } from "vitest";
import {
  CUSTOM_APPS_KEY,
  CustomAppsStore,
  fromMenuId,
  isCustomId,
  parseStored,
} from "./customApps.svelte";

/** A storage stub that never throws (write failures swallowed). */
function memoryStorage(): Storage {
  const map = new Map<string, string>();
  return {
    get length() {
      return map.size;
    },
    clear: () => map.clear(),
    getItem: (k) => map.get(k) ?? null,
    key: (i) => [...map.keys()][i] ?? null,
    removeItem: (k) => map.delete(k),
    setItem: (k, v) => void map.set(k, v),
  };
}

describe("parseStored", () => {
  it("re-defaults malformed payloads", () => {
    expect(parseStored(null)).toEqual([]);
    expect(parseStored("nope")).toEqual([]);
    expect(parseStored({ id: "a", name: "A", command: "cmd" })).toEqual([]);
  });

  it("drops entries missing id, name or command", () => {
    expect(
      parseStored([
        { id: "a", name: "A", command: "code {path}" },
        { name: "no id", command: "cmd" },
        { id: "b", command: "cmd" },
        { id: "c", name: "no command" },
        { id: "d", name: "   ", command: "cmd" },
        { id: "e", name: "E", command: "   " },
        "junk",
        null,
      ]),
    ).toEqual([{ id: "a", name: "A", command: "code {path}" }]);
  });

  it("trims name/command and keeps a non-empty icon", () => {
    expect(
      parseStored([
        { id: "a", name: "  A  ", command: " code {path} ", icon: "terminal" },
        { id: "b", name: "B", command: "cmd", icon: "" },
      ]),
    ).toEqual([
      { id: "a", name: "A", command: "code {path}", icon: "terminal" },
      { id: "b", name: "B", command: "cmd" },
    ]);
  });
});

describe("custom id namespace", () => {
  it("detects and strips the custom: prefix", () => {
    expect(isCustomId("custom:abc")).toBe(true);
    expect(isCustomId("vscode")).toBe(false);
    expect(fromMenuId("custom:abc")).toBe("abc");
    expect(fromMenuId("vscode")).toBe("vscode");
  });
});

describe("CustomAppsStore", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("starts empty with empty storage", () => {
    const store = new CustomAppsStore(memoryStorage());
    expect(store.list()).toEqual([]);
  });

  it("adds apps with minted ids and persists through the versioned envelope", () => {
    const storage = memoryStorage();
    const store = new CustomAppsStore(storage);
    const app = store.add({ name: "Neovim", command: " kitty {path} " });

    expect(store.list()).toHaveLength(1);
    expect(app.name).toBe("Neovim");
    expect(app.command).toBe("kitty {path}");
    expect(app.id).not.toBe("");

    const raw = storage.getItem(CUSTOM_APPS_KEY);
    expect(raw).not.toBeNull();
    expect(JSON.parse(raw as string)).toMatchObject({ v: 1 });

    const reloaded = new CustomAppsStore(storage);
    expect(reloaded.byId(app.id)).toEqual(app);
  });

  it("updates an existing app and persists the change", () => {
    const storage = memoryStorage();
    const store = new CustomAppsStore(storage);
    const app = store.add({ name: "Neovim", command: "kitty {path}" });

    const updated = store.update(app.id, { name: "Neovim (kitty)", command: "kitty --dir {path}" });
    expect(updated).toEqual({ id: app.id, name: "Neovim (kitty)", command: "kitty --dir {path}" });
    expect(new CustomAppsStore(storage).byId(app.id)?.command).toBe("kitty --dir {path}");
  });

  it("update returns undefined for an unknown id", () => {
    const store = new CustomAppsStore(memoryStorage());
    expect(store.update("nope", { name: "X" })).toBeUndefined();
  });

  it("removes an app; false for an unknown id", () => {
    const storage = memoryStorage();
    const store = new CustomAppsStore(storage);
    const app = store.add({ name: "Neovim", command: "kitty {path}" });
    expect(store.remove("nope")).toBe(false);
    expect(store.remove(app.id)).toBe(true);
    expect(store.list()).toEqual([]);
    expect(new CustomAppsStore(storage).list()).toEqual([]);
  });

  it("rejects blank name or command on add/update", () => {
    const store = new CustomAppsStore(memoryStorage());
    expect(() => store.add({ name: "  ", command: "cmd" })).toThrow(/name and a command/);
    expect(() => store.add({ name: "A", command: "" })).toThrow(/name and a command/);
    const app = store.add({ name: "A", command: "cmd" });
    expect(() => store.update(app.id, { command: "   " })).toThrow(/name and a command/);
    expect(store.list()).toHaveLength(1);
  });

  it("re-defaults to an empty list on a malformed envelope", () => {
    const storage = memoryStorage();
    storage.setItem(CUSTOM_APPS_KEY, "{not json");
    const store = new CustomAppsStore(storage);
    expect(store.list()).toEqual([]);
  });
});
