import { describe, expect, it } from "vitest";
import { getLocale, setLocale, t } from "./index";

describe("i18n groundwork", () => {
  it("translates known keys", () => {
    expect(t("action.refresh")).toBe("Refresh");
    expect(t("panel.health")).toBe("Repo");
  });

  it("interpolates {placeholders} and leaves unknown ones intact", () => {
    expect(t("dialog.deleteTag.title", { name: "v1.0" })).toBe("Delete tag v1.0?");
    expect(t("toast.failed", { label: "Push", error: "no net" })).toBe(
      "Push failed: no net",
    );
    expect(t("dialog.deleteTag.title")).toBe("Delete tag {name}?");
  });

  it("falls back to English, then the key", () => {
    expect(t("not.in.any.catalog")).toBe("not.in.any.catalog");
  });

  it("setLocale ignores unknown locales; getLocale reflects the active one", () => {
    setLocale("en");
    expect(getLocale()).toBe("en");
    // @ts-expect-error — runtime guard for unknown locales
    setLocale("xx");
    expect(getLocale()).toBe("en");
    expect(t("action.refresh")).toBe("Refresh");
  });

  // Pilot surfaces (item 69): the keys the wired call sites read.
  it("resolves the pilot keys (ConfirmDialog defaults, Toaster aria, shortcuts, palette)", () => {
    expect(t("action.cancel")).toBe("Cancel");
    expect(t("action.confirm")).toBe("Confirm");
    expect(t("toaster.dismiss")).toBe("Dismiss notification");
    expect(t("shortcuts.title")).toBe("Keyboard shortcuts");
    expect(t("common.noRepository")).toBe("no repository");
  });

  it("interpolates the pilot's {vars} (palette placeholder, repeat count)", () => {
    expect(t("palette.searchPlaceholder", { repo: "r1" })).toBe("Type a command… (r1)");
    expect(t("palette.searchPlaceholder", { repo: "no repository" })).toBe(
      "Type a command… (no repository)",
    );
    expect(t("toaster.repeated", { count: 3 })).toBe("Repeated 3 times");
  });

  it("falls back to the key for a missing pilot key", () => {
    expect(t("pilot.not.yet.cataloged")).toBe("pilot.not.yet.cataloged");
  });
});
