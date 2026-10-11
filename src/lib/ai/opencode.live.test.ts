/**
 * Opt-in integration tests against a REAL opencode server (either
 * generation): set `MYGITUI_OPENCODE_URL` (e.g. `http://127.0.0.1:4096`)
 * and run `MYGITUI_OPENCODE_URL=… npx vitest run src/lib/ai/opencode.live.test.ts`.
 * Without the variable these tests skip — CI stays hermetic. For a 2.x
 * server also set `MYGITUI_OPENCODE_PASSWORD` (2.x always requires
 * basic-auth).
 *
 * Covers the Windows-path regression: the provider must send the
 * `x-opencode-directory` header with forward slashes; backslash values make
 * opencode resolve a garbage project directory and every prompt 500.
 */

import { describe, expect, it } from "vitest";
import { OpenCodeProvider } from "./opencode";

const url = process.env.MYGITUI_OPENCODE_URL;
const password = process.env.MYGITUI_OPENCODE_PASSWORD;
const server = url ? describe : describe.skip;

/** Password wiring so the suite runs against auth-guarded 2.x servers too. */
function liveProvider(extra?: ConstructorParameters<typeof OpenCodeProvider>[0]) {
  return new OpenCodeProvider({
    url: () => url,
    ...(password !== undefined ? { password: async () => password } : {}),
    ...extra,
  });
}

server("OpenCodeProvider against a live opencode server", () => {
  it("health probe identifies the server", async () => {
    const provider = liveProvider();
    const result = await provider.check();
    expect(result.status).toBe("ok");
  });

  it("generates with a backslash Windows directory (forward-slash header)", async () => {
    const provider = liveProvider({
      directory: () => "C:\\Code\\Projects\\mygitui",
    });
    const result = await provider.generate({
      system: "You echo text.",
      prompt: "Reply with exactly: live-ok",
      sessionKey: `live-${Date.now()}`,
    });
    expect(result.backend).toBe("opencode");
    expect(result.text.trim().length).toBeGreaterThan(0);
  });

  it("reports a helpful error for a model the server cannot serve", async () => {
    const provider = liveProvider({
      directory: () => "C:\\Code\\Projects\\mygitui",
    });
    const err = await provider
      .generate({
        prompt: "hi",
        model: "bogus-provider/nonexistent",
        sessionKey: `live-err-${Date.now()}`,
      })
      .catch((e: unknown) => e);
    expect(String((err as Error)?.message)).toMatch(/UnknownError|ref|failed/i);
  });
});
