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
});
