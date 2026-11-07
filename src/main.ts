import { mount } from "svelte";
import "./app.css";

/**
 * Global error boundary — an unhandled exception during boot or render used
 * to leave a black window (the index.html splash auto-fades after 8s and
 * nothing replaces it). First unhandled error swaps the app for a plain-DOM
 * fallback (no framework code — the crash may have broken Svelte itself);
 * first-error-wins so a repeating timer error can't spam re-renders.
 * Registered BEFORE the app module is even imported (see the dynamic import
 * below), so module-evaluation failures are caught too.
 */
let errorShown = false;

/** Theme token or dark fallback (app.css vars may not exist pre-boot). */
function themeVar(name: string, fallback: string): string {
  const value = getComputedStyle(document.documentElement)
    .getPropertyValue(name)
    .trim();
  return value !== "" ? value : fallback;
}

function showErrorFallback(message: string): void {
  if (errorShown) return;
  errorShown = true;
  console.error("[mygitui] unhandled error:", message);

  const background = themeVar("--background", "#0a0a0a");
  const foreground = themeVar("--foreground", "#fafafa");

  const overlay = document.createElement("div");
  overlay.style.cssText = [
    "position: fixed",
    "inset: 0",
    "z-index: 2147483647",
    "display: flex",
    "flex-direction: column",
    "align-items: center",
    "justify-content: center",
    "gap: 12px",
    "padding: 24px",
    `background: ${background}`,
    `color: ${foreground}`,
    "font: 14px/1.5 Inter, system-ui, sans-serif",
    "text-align: center",
  ].join(";");

  const title = document.createElement("p");
  title.style.cssText = "margin:0; font-size: 18px; font-weight: 600";
  title.setAttribute("role", "alert"); // repo convention for error surfaces
  title.textContent = "Something went wrong";

  const detail = document.createElement("p");
  detail.style.cssText =
    "margin:0; max-width: 36rem; opacity: 0.8; overflow-wrap: anywhere";
  detail.textContent = message;

  const reload = document.createElement("button");
  reload.type = "button";
  reload.style.cssText = [
    "margin-top: 8px",
    "padding: 6px 18px",
    "border: 0",
    "border-radius: 999px",
    "cursor: pointer",
    "font-size: 13px",
    "font-weight: 500",
    `background: ${foreground}`,
    `color: ${background}`,
  ].join(";");
  reload.textContent = "Reload";
  reload.onclick = () => location.reload();

  overlay.append(title, detail, reload);

  // Replace whatever mounted (or didn't) with the fallback, and take the
  // splash down — it would otherwise fade to reveal only this overlay over
  // a dead app, or linger if the fade itself never ran.
  const app = document.getElementById("app");
  if (app) app.replaceChildren(overlay);
  else document.body.replaceChildren(overlay);
  document.getElementById("app-splash")?.remove();
}

function describe(err: unknown): string {
  if (err instanceof Error) return err.message;
  return typeof err === "string" ? err : String(err);
}

window.onerror = (message, _source, _line, _col, error) => {
  showErrorFallback(describe(error ?? message));
};

window.addEventListener("unhandledrejection", (event) => {
  showErrorFallback(describe(event.reason));
});

// Dynamic import ON PURPOSE: a static `import App` would evaluate the whole
// app module graph before the handlers above exist, leaving boot-time
// evaluation crashes uncaught (black window again). A failed import
// (top-level await rejection) lands in `unhandledrejection` instead.
const app = mount((await import("./App.svelte")).default, {
  target: document.getElementById("app")!,
});

export default app;
