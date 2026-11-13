/**
 * Usage/footer formatting shared by every AI result surface (M13).
 *
 * Pure string helpers — no store, no IO — so panels and tests render the
 * same "via <model> · <backend> · 4s · 1.2k tokens" footer everywhere.
 */

import type { AiResult, AiUsage } from "./types";

/** "150" / "1.2k" / "34.5k" / "1.2m" — compact token counts. */
export function formatTokens(n: number): string {
  if (!Number.isFinite(n) || n < 0) return "0";
  if (n < 1000) return String(Math.round(n));
  if (n < 1_000_000) {
    const k = n / 1000;
    return `${k < 10 ? k.toFixed(1) : Math.round(k)}k`;
  }
  const m = n / 1_000_000;
  return `${m < 10 ? m.toFixed(1) : Math.round(m)}m`;
}

/**
 * Short usage label: "1.2k tokens" (total), or "1.2k in / 300 out" when
 * the backend reports the split but no total. `null` when nothing useful
 * is known (never fabricate numbers).
 */
export function aiUsageLabel(usage: AiUsage | null | undefined): string | null {
  if (usage === null || usage === undefined) return null;
  const { inputTokens, outputTokens, totalTokens } = usage;
  if (typeof totalTokens === "number" && totalTokens > 0) {
    return `${formatTokens(totalTokens)} tokens`;
  }
  const hasIn = typeof inputTokens === "number" && inputTokens > 0;
  const hasOut = typeof outputTokens === "number" && outputTokens > 0;
  if (hasIn && hasOut) return `${formatTokens(inputTokens)} in / ${formatTokens(outputTokens)} out`;
  if (hasOut) return `${formatTokens(outputTokens)} tokens out`;
  if (hasIn) return `${formatTokens(inputTokens)} tokens in`;
  return null;
}

/** Seconds label for a result round trip ("4s", "2m 10s"). */
export function aiResultSeconds(elapsedMs: number): string {
  const total = Math.max(0, Math.round(elapsedMs / 1000));
  if (total < 60) return `${total}s`;
  return `${Math.floor(total / 60)}m ${total % 60}s`;
}

/**
 * The one result footer: "via <model> · <backend> · 4s · 1.2k tokens".
 * Usage/seconds parts drop out when unknown.
 */
export function aiResultMeta(result: Pick<AiResult, "model" | "backend" | "elapsedMs" | "usage">): string {
  const parts = [`via ${result.model}`, result.backend, aiResultSeconds(result.elapsedMs)];
  const usage = aiUsageLabel(result.usage);
  if (usage !== null) parts.push(usage);
  return parts.join(" · ");
}
