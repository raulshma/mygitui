/**
 * File-drop repository opening (e2e): dropping a file/folder from the
 * platform file manager opens it as a repo tab, through the webview's own
 * HTML5 drag-and-drop (`dragDropEnabled: false` — see tauri.conf.json and
 * `$lib/entry/dragdrop.ts`).
 *
 * Reproduces the environment the fix replaced: Tauri's native drag-drop
 * handler used to own file drops (`onDragDropEvent`); with it disabled the
 * page receives real `dragover`/`drop` events, with dropped paths as
 * `file://` URIs on Linux/WebKitGTK.
 */

import { expect, test } from "@playwright/test";
import { installTauriMock, openRecentRepo, recentRepos, seedStorage, trackErrors } from "./tauri-mock";

test("dropping a folder opens it as a repository tab", async ({ page }) => {
  const tracked = trackErrors(page);
  await seedStorage(page, recentRepos(["/tmp/repo"]));
  await installTauriMock(page);
  await openRecentRepo(page); // into a workspace so the shell is up
  await expect(page.getByRole("tablist", { name: "Open repositories" })).toBeVisible();

  // A platform file drag: types contain "Files" (a real File is attached)
  // and the paths ride along as `file://` URIs — the WebKitGTK shape.
  const invoked = await page.evaluate(() => {
    const dt = new DataTransfer();
    dt.items.add(new File([""], "myapp"));
    dt.setData("text/uri-list", "file:///tmp/dropped%20repo");
    let overAdmitted = false;
    window.addEventListener(
      "dragover",
      (e) => {
        overAdmitted = e.defaultPrevented;
      },
      { once: true },
    );
    window.dispatchEvent(
      new DragEvent("dragover", { dataTransfer: dt, bubbles: true, cancelable: true }),
    );
    window.dispatchEvent(
      new DragEvent("drop", { dataTransfer: dt, bubbles: true, cancelable: true }),
    );
    return { overAdmitted };
  });
  expect(invoked.overAdmitted, "dragover must admit file drags").toBe(true);

  // The decoded path flowed through to the backend open call…
  await expect
    .poll(async () => {
      const calls = await page.evaluate(() => window.__TAURI_MOCK__.calls);
      return calls.filter((c) => c.cmd === "repo_open").map((c) => c.args.path);
    })
    .toContain("/tmp/dropped repo");
  // …and the tab is in the strip (mock answers repo_open with /tmp/repo).
  await expect(page.getByRole("tab", { name: "repo — /tmp/repo" })).toBeVisible();

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
});

test("non-file drags (tab reorder) are not treated as drops", async ({ page }) => {
  const tracked = trackErrors(page);
  await seedStorage(page, recentRepos(["/tmp/repo"]));
  await installTauriMock(page);
  await openRecentRepo(page); // into a workspace so the shell is up
  await expect(page.getByRole("tablist", { name: "Open repositories" })).toBeVisible();

  // An in-page drag carrying only string data (the tab strip's marker) must
  // not trigger a repo open: no Files type, no path extraction.
  await page.evaluate(() => {
    const dt = new DataTransfer();
    dt.setData("application/x-mygitui-tab", "r1");
    dt.setData("text/plain", "r1");
    window.dispatchEvent(
      new DragEvent("dragover", { dataTransfer: dt, bubbles: true, cancelable: true }),
    );
    window.dispatchEvent(
      new DragEvent("drop", { dataTransfer: dt, bubbles: true, cancelable: true }),
    );
  });
  const calls = await page.evaluate(() => window.__TAURI_MOCK__.calls);
  // Only the boot-time open (the recent repo), nothing from the fake drop.
  expect(calls.filter((c) => c.cmd === "repo_open")).toHaveLength(1);

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
});
