import { expect, test } from "@playwright/test";
import { installTauriMock, seedStorage, trackErrors } from "./tauri-mock";

/**
 * Theme application: a pinned dark preference must apply the `.dark` class
 * (and with it the dark oklch token set + dark `--app-*` hues) even when the
 * OS reports a light color scheme.
 */
test.use({ colorScheme: "light" });

test("pinned dark theme applies dark status tokens on a light OS", async ({
  page,
}) => {
  const tracked = trackErrors(page);

  await seedStorage(page, { "mygitui.theme": "dark" });
  await installTauriMock(page);
  await page.goto("/");
  await expect(page.locator(".home-top")).toBeVisible();

  await expect
    .poll(() =>
      page.evaluate(() => document.documentElement.dataset.theme ?? null),
    )
    .toBe("dark");

  const tokens = await page.evaluate(() => {
    const root = document.documentElement;
    const style = getComputedStyle(root);
    return {
      darkClass: root.classList.contains("dark"),
      background: style.getPropertyValue("--background").trim(),
      added: style.getPropertyValue("--app-added").trim(),
      deleted: style.getPropertyValue("--app-deleted").trim(),
      modified: style.getPropertyValue("--app-modified").trim(),
      colorScheme: style.getPropertyValue("color-scheme").trim(),
    };
  });
  expect(tokens.darkClass).toBe(true);
  expect(tokens.background).toBe("oklch(0.145 0 0)");
  expect(tokens.added).toBe("#3fb950");
  expect(tokens.deleted).toBe("#f85149");
  expect(tokens.modified).toBe("#d29922");
  expect(tokens.colorScheme).toBe("dark");

  expect(tracked.errors).toEqual([]);
  await tracked.attach(test.info());
});

test("pinned light theme keeps light status tokens on a dark OS", async ({
  page,
}) => {
  await page.emulateMedia({ colorScheme: "dark" });
  const tracked = trackErrors(page);

  await seedStorage(page, { "mygitui.theme": "light" });
  await installTauriMock(page);
  await page.goto("/");
  await expect(page.locator(".home-top")).toBeVisible();

  await expect
    .poll(() =>
      page.evaluate(() => document.documentElement.dataset.theme ?? null),
    )
    .toBe("light");

  const tokens = await page.evaluate(() => {
    const root = document.documentElement;
    const style = getComputedStyle(root);
    return {
      darkClass: root.classList.contains("dark"),
      background: style.getPropertyValue("--background").trim(),
      added: style.getPropertyValue("--app-added").trim(),
      colorScheme: style.getPropertyValue("color-scheme").trim(),
    };
  });
  expect(tokens.darkClass).toBe(false);
  expect(tokens.background).toBe("oklch(1 0 0)");
  expect(tokens.added).toBe("#1a7f37");
  expect(tokens.colorScheme).toBe("light");

  expect(tracked.errors).toEqual([]);
  await tracked.attach(test.info());
});
