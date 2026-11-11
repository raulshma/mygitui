import { mount } from "svelte";
import "./app.css";
import { describeError } from "$lib/errors";

/**
 * Global error boundary — an unhandled exception during boot or render used
 * to leave a black window (the index.html splash auto-fades after 8s and
 * nothing replaces it). Errors thrown BEFORE the app finishes mounting swap
 * the app for a plain-DOM fallback (no framework code — the crash may have
 * broken Svelte itself; first-error-wins so a repeating timer error can't
 * spam re-renders). AFTER boot, runtime errors surface as a non-destructive
 * dismissible banner instead: one panel crashing must not replace the whole
 * app. The benign, spec-defined, self-healing "ResizeObserver loop
 * completed with undelivered notifications" error (a window error with no
 * Error object — expanding a panel resizes ~9 observers at once) is only
 * ever logged. Handlers are registered BEFORE the app module is even
 * imported (see the dynamic import below), so module-evaluation failures
 * are caught too. Popout windows boot through this same entry and get the
 * identical treatment.
 */
let errorShown = false;
/** Flips once `mount` resolved: before it, errors get the full fallback. */
let booted = false;

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

/**
 * Post-boot runtime errors: a fixed-position dismissible banner — message,
 * a "Copy details" button and a close button — appended to <body> as plain
 * DOM (it must not depend on the app being alive, same contract as
 * {@link showErrorFallback}), never replacing the mounted app. A later
 * error updates the message and the copy payload; dismissing the bar lets
 * a future error show a fresh one.
 */

/** Details (message + stack) the banner's Copy button hands to the clipboard. */
let bannerDetails = "";

function showErrorBanner(message: string, details: string): void {
  const background = themeVar("--destructive", "#dc2626");
  const foreground = themeVar("--destructive-foreground", "#ffffff");
  bannerDetails = details;

  let bar = document.getElementById("runtime-error-banner");
  const fresh = bar === null;
  if (bar === null) {
    bar = document.createElement("div");
    bar.id = "runtime-error-banner";
    bar.setAttribute("data-testid", "runtime-error-banner");
    bar.setAttribute("role", "alert");
    bar.style.cssText = [
      "position: fixed",
      "left: 12px",
      "right: 12px",
      "bottom: 12px",
      "z-index: 2147483647",
      "display: flex",
      "align-items: center",
      "gap: 10px",
      "padding: 10px 14px",
      "border-radius: 8px",
      `background: ${background}`,
      `color: ${foreground}`,
      "font: 13px/1.45 Inter, system-ui, sans-serif",
      "box-shadow: 0 8px 24px rgba(0, 0, 0, 0.35)",
    ].join(";");

    const text = document.createElement("span");
    text.className = "runtime-error-message";
    text.style.cssText =
      "min-width: 0; flex: 1 1 auto; overflow-wrap: anywhere; white-space: pre-wrap";
    bar.append(text);

    const copy = document.createElement("button");
    copy.type = "button";
    copy.textContent = "Copy details";
    copy.style.cssText = [
      "flex: 0 0 auto",
      "padding: 4px 10px",
      "border: 0",
      "border-radius: 999px",
      "background: transparent",
      "color: inherit",
      "cursor: pointer",
      "font: inherit",
    ].join(";");
    copy.onclick = () => {
      navigator.clipboard
        .writeText(bannerDetails)
        .then(() => (copy.textContent = "Copied"))
        .catch(() => (copy.textContent = "Copy failed"));
    };

    const dismiss = document.createElement("button");
    dismiss.type = "button";
    dismiss.textContent = "×";
    dismiss.setAttribute("aria-label", "Dismiss error");
    dismiss.style.cssText = [
      "flex: 0 0 auto",
      "width: 22px",
      "height: 22px",
      "border: 0",
      "border-radius: 999px",
      "background: transparent",
      "color: inherit",
      "cursor: pointer",
      "font: inherit",
      "line-height: 1",
    ].join(";");
    dismiss.onclick = () => bar!.remove();

    bar.append(copy, dismiss);
  }

  // The latest error wins in the (possibly reused) bar.
  const text = bar.querySelector<HTMLElement>(".runtime-error-message");
  if (text) text.textContent = message;

  if (fresh) document.body.append(bar);
}

function describe(err: unknown): string {
  return describeError(err);
}

/** Message + stack (when present) for the banner's Copy details button. */
function describeWithDetails(err: unknown, message: string): string {
  const stack = err instanceof Error && err.stack ? `\n${err.stack}` : "";
  return `${message}${stack}`;
}

/**
 * The browser's benign, spec-defined, self-healing resize-storm message
 * ("ResizeObserver loop completed with undelivered notifications" / "…
 * loop limit exceeded") arrives as a window error WITHOUT an Error object
 * and resolves itself on the next frame. Only ever logged.
 */
const RESIZE_LOOP_ERROR = /ResizeObserver loop/i;

function handleRuntimeError(err: unknown, rawMessage: unknown): void {
  const text = describe(err ?? rawMessage);
  if (RESIZE_LOOP_ERROR.test(text)) {
    console.warn("[mygitui] ResizeObserver loop (benign, self-healing):", text);
    return;
  }
  if (booted) {
    showErrorBanner(text, describeWithDetails(err, text));
  } else {
    showErrorFallback(text);
  }
}

window.onerror = (message, _source, _line, _col, error) => {
  handleRuntimeError(error, message);
};

window.addEventListener("unhandledrejection", (event) => {
  handleRuntimeError(event.reason, event.reason);
});

// Dynamic import ON PURPOSE: a static `import App` would evaluate the whole
// app module graph before the handlers above exist, leaving boot-time
// evaluation crashes uncaught (black window again). A failed import
// (top-level await rejection) lands in `unhandledrejection` — still pre-boot
// (`booted` is false), so it gets the full-replacement fallback.
const app = mount((await import("./App.svelte")).default, {
  target: document.getElementById("app")!,
});
booted = true;

export default app;
