/**
 * splitTrailers — the message/trailer block separator behind CommitDetail's
 * Details popover (plan §7.1: raw trailers never render inline).
 */
import { describe, expect, it } from "vitest";
import { splitTrailers } from "./trailers";

describe("splitTrailers", () => {
	it("splits a trailer block off the body", () => {
		const { body, trailers } = splitTrailers(
			"Move head\n\nSome prose.\n\nSigned-off-by: A U Thor <a@t.co>\nT3-Head-Moved-At: 2026-01-01T00:00:00Z\n",
		);
		expect(body).toBe("Move head\n\nSome prose.");
		expect(trailers).toEqual([
			{ token: "Signed-off-by", value: "A U Thor <a@t.co>" },
			{ token: "T3-Head-Moved-At", value: "2026-01-01T00:00:00Z" },
		]);
	});

	it("keeps prose-shaped final paragraphs in the body", () => {
		const message = "Fix bug\n\nSigned-off-by looks like this sometimes:\nbut prose follows.";
		const { body, trailers } = splitTrailers(message);
		expect(body).toBe(message);
		expect(trailers).toEqual([]);
	});

	it("joins folding continuation lines into the previous value", () => {
		const { body, trailers } = splitTrailers(
			"Subject\n\nCo-authored-by: X Y <x@t.co>\n  and Z W <z@t.co>",
		);
		expect(body).toBe("Subject");
		expect(trailers).toEqual([
			{ token: "Co-authored-by", value: "X Y <x@t.co> and Z W <z@t.co>" },
		]);
	});

	it("never strips a lone trailer-looking paragraph (it is the subject)", () => {
		const message = "Signed-off-by: A <a@t.co>";
		const { body, trailers } = splitTrailers(message);
		expect(body).toBe(message);
		expect(trailers).toEqual([]);
	});

	it("handles a message with summary and body but no trailers", () => {
		const { body, trailers } = splitTrailers("Subject\n\nNormal body text.\n");
		expect(body).toBe("Subject\n\nNormal body text.");
		expect(trailers).toEqual([]);
	});

	it("normalizes CRLF line endings", () => {
		const { body, trailers } = splitTrailers("Subject\r\n\r\nAcked-by: R <r@t.co>\r\n");
		expect(body).toBe("Subject");
		expect(trailers).toEqual([{ token: "Acked-by", value: "R <r@t.co>" }]);
	});

	it("accepts a trailer with an empty value", () => {
		const { trailers } = splitTrailers("Subject\n\nT3-Head-Moved-At:");
		expect(trailers).toEqual([{ token: "T3-Head-Moved-At", value: "" }]);
	});
});
