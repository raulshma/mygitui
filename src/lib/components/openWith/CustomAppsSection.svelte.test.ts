import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CustomAppsStore, CUSTOM_APPS_KEY } from "$lib/openwith/customApps.svelte";

vi.mock("$lib/toast", () => ({
  toast: vi.fn(),
  dismissToast: vi.fn(),
  pauseToast: vi.fn(),
  resumeToast: vi.fn(),
  getToasts: vi.fn(() => []),
}));

import { toast } from "$lib/toast";
import CustomAppsSection from "./CustomAppsSection.svelte";

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

function envelope(data: unknown): string {
  return JSON.stringify({ v: 1, data });
}

describe("CustomAppsSection", () => {
  let storage: Storage;
  let store: CustomAppsStore;

  beforeEach(() => {
    storage = memoryStorage();
    store = new CustomAppsStore(storage);
    vi.clearAllMocks();
  });

  afterEach(() => {
    cleanup();
  });

  it("renders the empty state when no custom apps exist", async () => {
    render(CustomAppsSection, { store });
    expect(screen.getByText("No custom apps.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Add custom app" })).toBeTruthy();
  });

  it("validates the form inline when saving without a name or command", async () => {
    render(CustomAppsSection, { store });
    await fireEvent.click(screen.getByRole("button", { name: "Add custom app" }));
    await fireEvent.click(screen.getByRole("button", { name: "Add app" }));

    expect(screen.getByText("Name is required.")).toBeTruthy();
    expect(screen.getByText("Command is required.")).toBeTruthy();
    expect(store.list()).toEqual([]);
    // The form stays open for corrections.
    expect(screen.getByLabelText("Custom app name")).toBeTruthy();
  });

  it("adds an app through the form and persists it via the store envelope", async () => {
    render(CustomAppsSection, { store });
    await fireEvent.click(screen.getByRole("button", { name: "Add custom app" }));

    await fireEvent.input(screen.getByLabelText("Custom app name"), {
      target: { value: "Neovim (kitty)" },
    });
    await fireEvent.input(screen.getByLabelText("Custom app command"), {
      target: { value: "kitty {path}" },
    });
    await fireEvent.click(screen.getByRole("button", { name: "Add app" }));

    // The list row shows name + command in mono.
    expect(screen.getByText("Neovim (kitty)")).toBeTruthy();
    expect(screen.getByText("kitty {path}")).toBeTruthy();
    expect(store.list()).toHaveLength(1);
    expect(store.list()[0].command).toBe("kitty {path}");

    // Persisted through the versioned envelope.
    const raw = storage.getItem(CUSTOM_APPS_KEY);
    expect(raw).not.toBeNull();
    expect(JSON.parse(raw as string)).toMatchObject({
      v: 1,
      data: [{ name: "Neovim (kitty)", command: "kitty {path}" }],
    });
    await waitFor(() => expect(toast).toHaveBeenCalledWith("Added custom app Neovim (kitty)", { kind: "success" }));
  });

  it("edits an existing app from its row and persists the change", async () => {
    store.add({ name: "Neovim (kitty)", command: "kitty {path}" });
    render(CustomAppsSection, { store });

    await fireEvent.click(
      screen.getByRole("button", { name: "Edit custom app Neovim (kitty)" }),
    );
    const nameInput = screen.getByLabelText("Custom app name") as HTMLInputElement;
    expect(nameInput.value).toBe("Neovim (kitty)");
    await fireEvent.input(nameInput, { target: { value: "Neovim" } });
    await fireEvent.click(screen.getByRole("button", { name: "Save changes" }));

    expect(screen.getByText("Neovim")).toBeTruthy();
    expect(store.list()[0].name).toBe("Neovim");
    const reloaded = new CustomAppsStore(storage);
    expect(reloaded.list()[0].name).toBe("Neovim");
  });

  it("deletes an app after the confirm dialog and persists the removal", async () => {
    store.add({ name: "Neovim", command: "kitty {path}" });
    render(CustomAppsSection, { store });

    await fireEvent.click(
      screen.getByRole("button", { name: "Delete custom app Neovim" }),
    );
    // The row is still there until the confirm lands.
    expect(screen.getByText("Neovim")).toBeTruthy();
    await fireEvent.click(screen.getByRole("button", { name: "Delete app" }));

    await waitFor(() => expect(screen.queryByText("Neovim")).toBeNull());
    expect(store.list()).toEqual([]);
    expect(new CustomAppsStore(storage).list()).toEqual([]);
  });
});
