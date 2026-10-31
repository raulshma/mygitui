/** Styled, safe-to-render pieces of one action output line. */
export interface ActionLogSegment {
  text: string;
  color?: string;
  background?: string;
  bold?: boolean;
  dim?: boolean;
  italic?: boolean;
  underline?: boolean;
  strike?: boolean;
}

interface AnsiStyle {
  color?: string;
  background?: string;
  bold: boolean;
  dim: boolean;
  italic: boolean;
  underline: boolean;
  inverse: boolean;
  conceal: boolean;
  strike: boolean;
}

const DEFAULT_STYLE: AnsiStyle = {
  bold: false,
  dim: false,
  italic: false,
  underline: false,
  inverse: false,
  conceal: false,
  strike: false,
};

// Map the standard 16 terminal colors to theme-aware app colors. This keeps
// common tool output readable in both light and dark themes; extended colors
// still use the exact RGB value supplied by the process.
const ANSI_COLORS = [
  "var(--muted-foreground)",
  "var(--destructive)",
  "var(--app-added)",
  "var(--app-modified)",
  "var(--primary)",
  "var(--app-renamed)",
  "var(--secondary)",
  "var(--foreground)",
  "var(--border)",
  "var(--destructive)",
  "var(--app-added)",
  "var(--app-modified)",
  "var(--primary)",
  "var(--app-renamed)",
  "var(--secondary)",
  "var(--foreground)",
];

/**
 * Converts ANSI SGR styling to normal text segments for Svelte to render.
 * Escape sequences are consumed as control data, never injected as HTML.
 * Carriage-return progress redraws keep their final frame, as a terminal does.
 */
export function actionLogSegments(input: string): ActionLogSegment[] {
  const lastFrame = input.slice(input.lastIndexOf("\r") + 1);
  const segments: ActionLogSegment[] = [];
  let style: AnsiStyle = { ...DEFAULT_STYLE };
  let text = "";

  const flush = () => {
    if (!text) return;
    let color = style.color;
    let background = style.background;
    if (style.inverse) {
      [color, background] = [
        background ?? "var(--accent)",
        color ?? "var(--foreground)",
      ];
    }
    if (style.conceal) color = "transparent";
    segments.push({
      text,
      color,
      background,
      bold: style.bold,
      dim: style.dim,
      italic: style.italic,
      underline: style.underline,
      strike: style.strike,
    });
    text = "";
  };

  let index = 0;
  while (index < lastFrame.length) {
    const code = lastFrame.charCodeAt(index);

    if (code === 0x1b) {
      flush();
      const next = lastFrame[index + 1];

      if (next === "[") {
        // CSI ends at the first byte in the final-byte range (0x40–0x7e).
        let end = index + 2;
        while (
          end < lastFrame.length &&
          (lastFrame.charCodeAt(end) < 0x40 || lastFrame.charCodeAt(end) > 0x7e)
        ) {
          end++;
        }
        if (end < lastFrame.length) {
          if (lastFrame[end] === "m") {
            style = applySgr(style, lastFrame.slice(index + 2, end));
          }
          index = end + 1;
        } else {
          index = lastFrame.length;
        }
        continue;
      }

      if (next === "]") {
        // OSC title and hyperlink sequences end with BEL or ST (ESC backslash).
        let end = index + 2;
        while (end < lastFrame.length) {
          if (lastFrame.charCodeAt(end) === 0x07) {
            end++;
            break;
          }
          if (lastFrame.charCodeAt(end) === 0x1b && lastFrame[end + 1] === "\\") {
            end += 2;
            break;
          }
          end++;
        }
        index = end;
        continue;
      }

      // Consume other two-byte ESC controls (for example, charset selection).
      index += next === undefined ? 1 : 2;
      continue;
    }

    // Keep tabs for column alignment; discard other non-printing C0 controls.
    if (code < 0x20 && code !== 0x09) {
      index++;
      continue;
    }

    const point = lastFrame.codePointAt(index)!;
    const char = String.fromCodePoint(point);
    text += char;
    index += char.length;
  }
  flush();
  return segments;
}

function applySgr(current: AnsiStyle, rawParams: string): AnsiStyle {
  const next = { ...current };
  const params = rawParams === "" ? [0] : rawParams.split(";").map(Number);

  for (let i = 0; i < params.length; i++) {
    const code = Number.isFinite(params[i]) ? params[i] : 0;
    if (code === 0) {
      next.color = undefined;
      next.background = undefined;
      Object.assign(next, DEFAULT_STYLE);
    } else if (code === 1) next.bold = true;
    else if (code === 2) next.dim = true;
    else if (code === 3) next.italic = true;
    else if (code === 4) next.underline = true;
    else if (code === 7) next.inverse = true;
    else if (code === 8) next.conceal = true;
    else if (code === 9) next.strike = true;
    else if (code === 22) {
      next.bold = false;
      next.dim = false;
    } else if (code === 23) next.italic = false;
    else if (code === 24) next.underline = false;
    else if (code === 27) next.inverse = false;
    else if (code === 28) next.conceal = false;
    else if (code === 29) next.strike = false;
    else if (code >= 30 && code <= 37) next.color = ANSI_COLORS[code - 30];
    else if (code >= 90 && code <= 97) next.color = ANSI_COLORS[code - 90 + 8];
    else if (code === 39) next.color = undefined;
    else if (code >= 40 && code <= 47) next.background = ANSI_COLORS[code - 40];
    else if (code >= 100 && code <= 107) {
      next.background = ANSI_COLORS[code - 100 + 8];
    } else if (code === 49) next.background = undefined;
    else if (code === 38 || code === 48) {
      const color = readExtendedColor(params, i + 1);
      if (color !== null) {
        if (code === 38) next.color = color.color;
        else next.background = color.color;
        i += color.consumed;
      }
    }
  }

  return next;
}

/** Reads SGR `5;n` or `2;r;g;b` after a 38/48 parameter. */
function readExtendedColor(
  params: number[],
  start: number,
): { color: string; consumed: number } | null {
  const mode = params[start];
  if (mode === 5 && start + 1 < params.length) {
    return { color: xtermColor(params[start + 1]), consumed: 2 };
  }
  if (mode === 2 && start + 3 < params.length) {
    const rgb = params.slice(start + 1, start + 4).map((v) =>
      Math.max(0, Math.min(255, Math.floor(v))),
    );
    return { color: `rgb(${rgb.join(" ")})`, consumed: 4 };
  }
  return null;
}

function xtermColor(index: number): string {
  const n = Math.max(0, Math.min(255, Math.floor(index)));
  if (n < 16) return ANSI_COLORS[n];
  if (n >= 232) {
    const shade = 8 + (n - 232) * 10;
    return `rgb(${shade} ${shade} ${shade})`;
  }
  const levels = [0, 95, 135, 175, 215, 255];
  const value = n - 16;
  const r = levels[Math.floor(value / 36)];
  const g = levels[Math.floor((value % 36) / 6)];
  const b = levels[value % 6];
  return `rgb(${r} ${g} ${b})`;
}
