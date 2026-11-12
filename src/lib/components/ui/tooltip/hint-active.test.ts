import { afterEach, describe, expect, vi } from "vitest";
import { test } from "vitest";
import { claimActiveHint, releaseActiveHint } from "./hint-active.js";

/** Registry semantics: one open slot app-wide, idempotent close/release. */
describe("hint-active", () => {
	afterEach(() => {
		vi.restoreAllMocks();
	});

	test("claiming closes the previous holder", () => {
		const closes = { a: vi.fn(), b: vi.fn() };
		claimActiveHint(closes.a);
		claimActiveHint(closes.b);
		expect(closes.a).toHaveBeenCalledTimes(1);
		expect(closes.b).not.toHaveBeenCalled();
		releaseActiveHint(closes.b);
	});

	test("re-claiming the same closer does not close itself", () => {
		const close = vi.fn();
		claimActiveHint(close);
		claimActiveHint(close);
		expect(close).not.toHaveBeenCalled();
		releaseActiveHint(close);
	});

	test("release is a no-op when someone else holds the slot", () => {
		const closes = { a: vi.fn(), b: vi.fn() };
		claimActiveHint(closes.a);
		claimActiveHint(closes.b);
		// `a`'s teardown runs after `b` already claimed the slot — it must
		// not release `b`'s claim.
		releaseActiveHint(closes.a);
		const c = vi.fn();
		claimActiveHint(c);
		expect(closes.b).toHaveBeenCalledTimes(1);
		releaseActiveHint(c);
	});
});
