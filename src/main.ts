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
 * Post-boot runtime errors: a fixed-position dismissible banner — headline,
 * collapsible detail (the rest of a multi-line message, e.g. Svelte's
 * component stack), a "Copy details" button and a close button — appended
 * to <body> as plain DOM (it must not depend on the app being alive, same
 * contract as {@link showErrorFallback}), never replacing the mounted app.
 * A later error updates the message and the copy payload; dismissing the
 * bar lets a future error show a fresh one.
 *
 * Styled from the theme tokens like the app's other destructive surfaces
 * (a neutral card with a destructive *accent*), not a destructive fill:
 * this scheme's dark `--destructive` is a light salmon meant for accents,
 * so white-on-destructive text is unreadable there.
 */

/** Details (message + stack) the banner's Copy button hands to the clipboard. */
let bannerDetails = "";

/** Warn-triangle glyph for the banner's accent chip (24×24 currentColor). */
const ALERT_SVG =
  '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3"/><path d="M12 9v4"/><path d="M12 17h.01"/></svg>';

function showErrorBanner(message: string, details: string): void {
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
      "flex-direction: column",
      "gap: 8px",
      "padding: 10px 12px",
      "border: 1px solid color-mix(in srgb, var(--destructive, #dc2626) 35%, transparent)",
      "border-radius: var(--radius, 8px)",
      "background: var(--popover, var(--card, #fff))",
      "color: var(--foreground, #0a0a0a)",
      "font: 13px/1.45 Inter, system-ui, sans-serif",
      "box-shadow: var(--shadow-overlay, 0 8px 24px rgba(0, 0, 0, 0.35))",
    ].join(";");

    // Header row: accent icon chip, headline, then the actions.
    const head = document.createElement("div");
    head.style.cssText =
      "display: flex; align-items: center; gap: 10px; min-width: 0";

    const chip = document.createElement("span");
    chip.style.cssText = [
      "flex: 0 0 auto",
      "display: grid",
      "place-items: center",
      "width: 26px",
      "height: 26px",
      "border-radius: 999px",
      "background: color-mix(in srgb, var(--destructive, #dc2626) 15%, transparent)",
      "color: var(--destructive, #dc2626)",
    ].join(";");
    chip.setAttribute("aria-hidden", "true");
    chip.innerHTML = ALERT_SVG;
    const glyph = chip.firstElementChild as SVGSVGElement | null;
    if (glyph) {
      glyph.style.width = "14px";
      glyph.style.height = "14px";
    }

    const text = document.createElement("span");
    text.className = "runtime-error-message";
    text.style.cssText =
      "min-width: 0; flex: 1 1 auto; font-weight: 500; overflow-wrap: anywhere";
    head.append(chip, text);

    const copy = document.createElement("button");
    copy.type = "button";
    copy.textContent = "Copy details";
    copy.className = "runtime-error-copy";
    copy.style.cssText = [
      "flex: 0 0 auto",
      "padding: 4px 10px",
      "border: 1px solid var(--border, #e4e4e7)",
      "border-radius: 999px",
      "background: transparent",
      "color: inherit",
      "cursor: pointer",
      "font: inherit",
      "white-space: nowrap",
    ].join(";");
    copy.onmouseenter = () =>
      (copy.style.background =
        "color-mix(in srgb, var(--foreground, #0a0a0a) 6%, transparent)");
    copy.onmouseleave = () => (copy.style.background = "transparent");
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
      "display: grid",
      "place-items: center",
      "width: 24px",
      "height: 24px",
      "border: 0",
      "border-radius: 999px",
      "background: transparent",
      "color: inherit",
      "cursor: pointer",
      "font: inherit",
      "font-size: 15px",
      "line-height: 1",
    ].join(";");
    dismiss.onmouseenter = () =>
      (dismiss.style.background =
        "color-mix(in srgb, var(--foreground, #0a0a0a) 6%, transparent)");
    dismiss.onmouseleave = () => (dismiss.style.background = "transparent");
    dismiss.onclick = () => bar!.remove();

    head.append(copy, dismiss);

    // Optional detail block: everything past the first line (component
    // stack, cause chains) — monospace, capped, scrollable, selectable.
    const detail = document.createElement("pre");
    detail.className = "runtime-error-detail";
    detail.style.cssText = [
      "display: none",
      "margin: 0",
      "max-height: 6.5rem",
      "overflow: auto",
      "padding: 6px 8px",
      "border-radius: calc(var(--radius, 8px) - 2px)",
      "background: color-mix(in srgb, var(--foreground, #0a0a0a) 4%, transparent)",
      "color: var(--muted-foreground, #737373)",
      "font: 12px/1.5 var(--app-font-mono, ui-monospace, monospace)",
      "white-space: pre-wrap",
      "overflow-wrap: anywhere",
      "user-select: text",
    ].join(";");

    bar.append(head, detail);
  }

  // The latest error wins in the (possibly reused) bar: first line is the
  // headline, the rest (when present) lands in the detail block.
  const newlineAt = message.indexOf("\n");
  const headText =
    newlineAt === -1 ? message : message.slice(0, newlineAt);
  const rest = newlineAt === -1 ? "" : message.slice(newlineAt + 1).trim();

  const text = bar.querySelector<HTMLElement>(".runtime-error-message");
  if (text) text.textContent = headText;

  const detail = bar.querySelector<HTMLElement>(".runtime-error-detail");
  if (detail) {
    detail.textContent = rest;
    detail.style.display = rest === "" ? "none" : "block";
  }

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
