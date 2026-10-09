/**
 * AI health chip mapping (M12, lane C): collapses the two-backend status
 * map from the connection supervisor into one dot + label for the chip —
 * green "connected", amber "degraded/retrying", red "down", gray "not
 * configured". Pure and unit tested; the component only renders.
 */

import type { BackendStatus, BackendStatusMap } from "./connection";
import type { AiBackend } from "./types";

export type AiHealthTone = "ok" | "degraded" | "down" | "off";

/** What the chip renders. */
export interface AiHealthView {
  tone: AiHealthTone;
  label: string;
  /** Hover tooltip: one line per backend (status + latency / error). */
  detail: string;
}

/** One backend's tooltip line, e.g. `opencode: ok (123 ms)`. */
export function backendLine(backend: AiBackend, status: BackendStatus): string {
  switch (status.status) {
    case "ok":
      return `${backend}: ok (${status.latencyMs ?? "?"} ms)`;
    case "checking":
      return `${backend}: checking…`;
    case "unknown":
      return `${backend}: not probed yet`;
    case "unauthenticated":
      return `${backend}: needs credentials`;
    case "down":
      return status.error ? `${backend}: down — ${status.error}` : `${backend}: down`;
  }
}

/**
 * The chip state for the CURRENT supervisor snapshot. `preferred` is the
 * configured backend; when it is down/unknown and the fallback is
 * connected (and allowed), the chip shows amber "degraded (fallback)"
 * instead of red.
 */
export function aiHealth(
  statuses: BackendStatusMap,
  preferred: AiBackend,
  allowFallback: boolean,
): AiHealthView {
  const other: AiBackend = preferred === "opencode" ? "openrouter" : "opencode";
  const detail = `${backendLine(preferred, statuses[preferred])}\n${backendLine(other, statuses[other])}`;

  switch (statuses[preferred].status) {
    case "ok":
      return { tone: "ok", label: "connected", detail };
    case "checking":
      return { tone: "degraded", label: "checking…", detail };
    case "unauthenticated":
      return { tone: "degraded", label: "needs credentials", detail };
    case "down":
      if (allowFallback && statuses[other].status === "ok") {
        return { tone: "degraded", label: "degraded (fallback)", detail };
      }
      return { tone: "down", label: "down", detail };
    case "unknown":
      // Never probed: nothing has asked for AI yet. The chip stays gray
      // unless the fallback is known-good and may serve.
      if (allowFallback && statuses[other].status === "ok") {
        return { tone: "degraded", label: "fallback ready", detail };
      }
      return { tone: "off", label: "not configured", detail };
  }
}
