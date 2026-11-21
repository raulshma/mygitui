/**
 * Commit-message trailer parsing (plan §7.1: raw trailers like
 * `T3-Head-Moved-At` must never render inline).
 *
 * Git trailers are the message's final paragraph when EVERY line there is
 * either a `token: value` line or a folding continuation of the previous
 * one (a line starting with whitespace, RFC 2822-style). This module splits
 * them off so the full message can render trailer-free while the Details
 * popover lists the trailers as structured rows.
 *
 * The rules here are deliberately conservative — a paragraph that mixes a
 * trailer-shaped line with prose stays in the body (git itself would also
 * refuse to interpret it as a trailer block), and a message consisting of
 * nothing but one trailer-looking paragraph is never split (the paragraph
 * IS the subject).
 */

export interface CommitTrailer {
	/** Token before the colon, trimmed (e.g. `Signed-off-by`). */
	token: string;
	/** Value after the colon with folded continuation lines joined. */
	value: string;
}

export function splitTrailers(message: string): {
	body: string;
	trailers: CommitTrailer[];
} {
	const text = message.replace(/\r\n/g, "\n").trimEnd();
	// The trailer block, if any, is the LAST paragraph (blank-line
	// separated); the body is everything before that blank-line anchor.
	const cut = text.lastIndexOf("\n\n");
	if (cut < 0) return { body: text, trailers: [] };
	const lines = text.slice(cut + 2).split("\n");
	if (!isTrailerBlock(lines)) return { body: text, trailers: [] };
	return {
		body: text.slice(0, cut).trimEnd(),
		trailers: parseTrailers(lines),
	};
}

/** Every line must be a `token: value` line or a folding continuation. */
function isTrailerBlock(lines: string[]): boolean {
	let sawTrailer = false;
	for (const line of lines) {
		if (/^\s/.test(line)) {
			// A continuation must follow a trailer line and carry content.
			if (!sawTrailer || line.trim() === "") return false;
			continue;
		}
		if (!/^[-A-Za-z0-9_]+:(\s.*)?$/.test(line)) return false;
		sawTrailer = true;
	}
	return sawTrailer;
}

/** Joins folding continuations into the preceding trailer's value. */
function parseTrailers(lines: string[]): CommitTrailer[] {
	const trailers: CommitTrailer[] = [];
	for (const line of lines) {
		if (/^\s/.test(line)) {
			const previous = trailers.at(-1);
			if (previous) previous.value += ` ${line.trim()}`;
			continue;
		}
		const separator = line.indexOf(":");
		trailers.push({
			token: line.slice(0, separator).trim(),
			value: line.slice(separator + 1).trim(),
		});
	}
	return trailers;
}
