//! Syntax highlighting for diff rows, backed by `syntect`.
//!
//! The [`Highlighter`] loads syntect's default syntax set and one theme per
//! app scheme ("base16-ocean.dark" / "base16-ocean.light") exactly once into
//! statics; instances for both modes are built lazily on first render and
//! shared for the rest of the process ([`for_mode`] returns a `&'static`).
//!
//! # Per-line, stateless parsing (documented tradeoff)
//!
//! The diff viewer virtualizes rows, so a row can be rendered in any order
//! at any time — there is no sequential stream to carry syntect's
//! multi-line parse state across. Each visible line is therefore parsed
//! with a fresh [`ParseState`] + [`HighlightState`]: this is the documented
//! cheap approach for per-line APIs and costs one small regex pass per
//! visible row per frame (dozens of rows, not thousands). The visible
//! tradeoff: constructs spanning lines (block comments, heredocs, C
//! `/* … */`) only color their first line. Whole-file highlighting on load
//! was rejected for the same virtualization reasons (any scroll/option
//! change would need re-rendering from cached per-line runs).
//!
//! Lines longer than [`MAX_HIGHLIGHT_CHARS`] are highlighted for their
//! first [`MAX_HIGHLIGHT_CHARS`] chars only; the remainder is appended as a
//! plain run in the fallback color (mirrors Sublime's own "too long, don't
//! highlight" guard, without dropping any text).
//!
//! # Colors
//!
//! Runs whose resolved foreground equals the theme's default foreground are
//! remapped to the caller's `fallback` color, so unhighlighted text stays
//! pixel-identical to the surrounding UI (the base16 palettes are designed
//! for their own backgrounds, not our card color).
//!
//! # Feature gate
//!
//! Rendering is gated by a process-wide [`AtomicBool`] (default on, matching
//! `Config::highlight`'s serde default). The shell wires the config flag in
//! one line — `ui::diff_view::highlight::set_enabled(config.highlight)` —
//! because `diff_view::view`'s signature is fixed by its call sites.
//!
//! # Module location note
//!
//! This file is declared from `diff_view.rs` via `#[path = "highlight.rs"]
//! pub mod highlight;` so the temporary `ui/mod.rs` registrations can be
//! reverted after gating; the orchestrator may move the declaration to
//! `ui/mod.rs` (as `pub mod highlight;`) when wiring the config flag.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use iced::widget::text::Span;
use iced::{Background, Color};
use syntect::highlighting::{
    HighlightIterator, HighlightState, Highlighter as ThemeHighlighter, Theme, ThemeSet,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

/// Master switch for the feature (see the module docs).
static ENABLED: AtomicBool = AtomicBool::new(true);

/// Set/clear the process-wide highlight switch (wired from `Config::highlight`).
// Unwired lane: the shell's one-line wiring (`set_enabled(config.highlight)`)
// lands later; today only `enabled()` is read at render time.
#[allow(dead_code)]
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

/// Whether diff rows should be syntax-highlighted.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Lines longer than this many chars highlight only their head; the rest
/// renders as a plain tail run (Sublime's own behavior for long lines).
pub const MAX_HIGHLIGHT_CHARS: usize = 2000;

/// syntect theme for the dark scheme.
const DARK_THEME: &str = "base16-ocean.dark";
/// syntect theme for the light scheme.
const LIGHT_THEME: &str = "base16-ocean.light";

/// One colored run of a diff line. Concatenated `text`s reproduce the
/// original line exactly (including the plain tail of truncated lines).
#[derive(Debug, Clone, PartialEq)]
pub struct HLRun {
    pub text: String,
    pub color: Color,
}

/// Shared syntect state for one app scheme: the (static) default syntax
/// set, a precompiled theme and a per-extension syntax cache.
pub struct Highlighter {
    syntax_set: &'static SyntaxSet,
    theme: ThemeHighlighter<'static>,
    /// Theme-wide default foreground; runs in this color are remapped to
    /// the caller's fallback so plain text matches the surrounding UI.
    default_fg: Option<syntect::highlighting::Color>,
    /// `find_syntax_by_extension` is a linear scan; cache by extension.
    syntaxes: Mutex<HashMap<String, &'static SyntaxReference>>,
}

/// Shared highlighter for `dark` (or light) mode, built once per process.
pub fn for_mode(dark: bool) -> &'static Highlighter {
    static DARK: OnceLock<Highlighter> = OnceLock::new();
    static LIGHT: OnceLock<Highlighter> = OnceLock::new();
    let slot = if dark { &DARK } else { &LIGHT };
    slot.get_or_init(|| Highlighter::build(dark))
}

fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn theme_set() -> &'static ThemeSet {
    static SETS: OnceLock<ThemeSet> = OnceLock::new();
    SETS.get_or_init(ThemeSet::load_defaults)
}

fn theme_for(dark: bool) -> &'static Theme {
    let sets = theme_set();
    let name = if dark { DARK_THEME } else { LIGHT_THEME };
    sets.themes
        .get(name)
        .or_else(|| sets.themes.values().next())
        .expect("syntect default themes are compiled in")
}

impl Highlighter {
    fn build(dark: bool) -> Self {
        let theme = theme_for(dark);
        let default_fg = theme.settings.foreground;
        Self {
            syntax_set: syntax_set(),
            theme: ThemeHighlighter::new(theme),
            default_fg,
            syntaxes: Mutex::new(HashMap::new()),
        }
    }

    /// Syntax for a file path: matched by extension, plain text fallback.
    /// Lookups are cached per extension.
    pub fn syntax_for_path(&self, path: &str) -> &'static SyntaxReference {
        let Some(ext) = extension_of(path) else {
            return self.syntax_set.find_syntax_plain_text();
        };
        let mut cache = self.syntaxes.lock().expect("syntax cache poisoned");
        if let Some(syntax) = cache.get(ext) {
            return syntax;
        }
        let syntax = self
            .syntax_set
            .find_syntax_by_extension(ext)
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());
        cache.insert(ext.to_owned(), syntax);
        syntax
    }

    /// Syntax display name for a path (test/inspection helper; also the
    /// cheapest way to assert the extension mapping).
    #[allow(dead_code)] // tests exercise it; production code has no use yet
    pub fn syntax_name_for_path(&self, path: &str) -> &'static str {
        self.syntax_for_path(path).name.as_str()
    }

    /// Highlight one source line of the file at `path`.
    ///
    /// `fallback` colors unscoped text (the theme's own default foreground
    /// is remapped to it) and the plain tail of over-long lines. Never
    /// drops text: the returned runs concatenate to exactly `line`
    /// (minus nothing; only colors change). An empty line yields no runs.
    pub fn highlight_line(&self, path: &str, line: &str, fallback: Color) -> Vec<HLRun> {
        if line.is_empty() {
            return Vec::new();
        }
        let (head, tail) = split_head_tail(line);
        let syntax = self.syntax_for_path(path);

        // The default syntax set is the "newlines" variant: per-line parsing
        // is more accurate when lines keep their terminator.
        let mut line_nl = String::with_capacity(head.len() + 1);
        line_nl.push_str(head);
        line_nl.push('\n');

        let mut runs = match Self::parse(&line_nl, syntax, self.syntax_set, &self.theme) {
            Some(raw) => runs_from_slices(&raw, head.len(), fallback, self.default_fg),
            None => vec![HLRun {
                text: head.to_owned(),
                color: fallback,
            }],
        };
        if !tail.is_empty() {
            runs.push(HLRun {
                text: tail.to_owned(),
                color: fallback,
            });
        }
        runs
    }

    /// One stateless parse+style pass over `line` (see the module docs for
    /// why the state is not carried across lines).
    fn parse<'l>(
        line: &'l str,
        syntax: &'l SyntaxReference,
        syntax_set: &SyntaxSet,
        theme: &ThemeHighlighter<'_>,
    ) -> Option<Vec<(syntect::highlighting::Style, &'l str)>> {
        let mut parse_state = ParseState::new(syntax);
        let ops = parse_state.parse_line(line, syntax_set).ok()?;
        let mut highlight_state = HighlightState::new(theme, ScopeStack::new());
        Some(HighlightIterator::new(&mut highlight_state, &ops[..], line, theme).collect())
    }
}

/// Byte offset of the first `MAX_HIGHLIGHT_CHARS` chars (char-boundary safe).
fn split_head_tail(line: &str) -> (&str, &str) {
    if line.len() <= MAX_HIGHLIGHT_CHARS || line.chars().count() <= MAX_HIGHLIGHT_CHARS {
        return (line, "");
    }
    let end = line
        .char_indices()
        .nth(MAX_HIGHLIGHT_CHARS)
        .map_or(line.len(), |(i, _)| i);
    line.split_at(end)
}

/// Extension of a path's file name (`None` for dotfiles, trailing dots and
/// extension-less names).
fn extension_of(path: &str) -> Option<&str> {
    let name = path.rsplit(['/', '\\']).next()?;
    let (stem, ext) = name.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() {
        return None;
    }
    Some(ext)
}

/// syntect sRGB (0-255, gamma-encoded) → iced linear-ish float color.
pub fn to_iced(color: syntect::highlighting::Color) -> Color {
    Color {
        r: f32::from(color.r) / 255.0,
        g: f32::from(color.g) / 255.0,
        b: f32::from(color.b) / 255.0,
        a: f32::from(color.a) / 255.0,
    }
}

/// Convert the parsed `(Style, &str)` slices of `line` into runs, clipping
/// the trailing newline region (`head_len` is the pre-newline byte length)
/// and remapping the theme default foreground to `fallback`.
fn runs_from_slices(
    slices: &[(syntect::highlighting::Style, &str)],
    head_len: usize,
    fallback: Color,
    default_fg: Option<syntect::highlighting::Color>,
) -> Vec<HLRun> {
    let mut runs: Vec<HLRun> = Vec::new();
    let mut offset = 0usize;
    for (style, text) in slices {
        let take = text.len().min(head_len.saturating_sub(offset));
        offset += text.len();
        if take == 0 {
            continue;
        }
        let color = if default_fg == Some(style.foreground) {
            fallback
        } else {
            to_iced(style.foreground)
        };
        match runs.last_mut() {
            // Coalesce adjacent same-colored slices (regex backtracking can
            // emit many tiny identical regions).
            Some(last) if last.color == color => last.text.push_str(&text[..take]),
            _ => runs.push(HLRun {
                text: text[..take].to_owned(),
                color,
            }),
        }
    }
    runs
}

/// Merge highlight runs with word-diff byte ranges into rich-text spans.
///
/// Syntax colors always paint the glyphs; when `emphasis` is set, the
/// (sorted or unsorted, inclusive-exclusive) byte `word_ranges` additionally
/// get a background tint — the word-diff emphasis wins visually without
/// destroying the syntax colors (the "clean" combination from the design
/// notes: colored glyphs + word-diff background, no priority conflict).
pub fn to_spans<'a>(
    runs: Vec<HLRun>,
    word_ranges: &[(u32, u32)],
    emphasis: Option<Color>,
) -> Vec<Span<'a>> {
    let Some(emphasis) = emphasis.filter(|_| !word_ranges.is_empty()) else {
        return runs
            .into_iter()
            .map(|run| Span::new(run.text).color(run.color))
            .collect();
    };
    let mut spans = Vec::new();
    let mut base = 0usize; // byte offset of the current run within the line
    for run in runs {
        let len = run.text.len();
        let mut points = vec![0usize, len];
        for (start, end) in word_ranges {
            for bound in [usize_from_u32(*start), usize_from_u32(*end)] {
                if bound > base && bound < base + len && run.text.is_char_boundary(bound - base) {
                    points.push(bound - base);
                }
            }
        }
        points.sort_unstable();
        points.dedup();
        for pair in points.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let global_start = base + a;
            let global_end = base + b;
            let emphasized = word_ranges.iter().any(|(s, e)| {
                global_start >= usize_from_u32(*s) && global_end <= usize_from_u32(*e)
            });
            let mut span = Span::new(run.text[a..b].to_owned()).color(run.color);
            if emphasized {
                span = span.background(Background::Color(emphasis));
            }
            spans.push(span);
        }
        base += len;
    }
    spans
}

/// u32 → usize widening (`usize::From<u32>` does not exist in std).
fn usize_from_u32(value: u32) -> usize {
    value as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    const FALLBACK: Color = Color {
        r: 0.98,
        g: 0.98,
        b: 0.98,
        a: 1.0,
    };

    #[test]
    fn extension_detection() {
        assert_eq!(extension_of("src/lib.rs"), Some("rs"));
        assert_eq!(extension_of("app.ts"), Some("ts"));
        assert_eq!(extension_of("script.py"), Some("py"));
        assert_eq!(extension_of("README.md"), Some("md"));
        assert_eq!(extension_of("archive.tar.gz"), Some("gz"));
        assert_eq!(extension_of("Makefile"), None);
        assert_eq!(extension_of(".gitignore"), None);
        assert_eq!(extension_of("weird."), None);
        assert_eq!(extension_of(""), None);
        // Directories never leak into the extension.
        assert_eq!(extension_of("dir.v1/file"), None);
        assert_eq!(extension_of(r"win\path\file.tsx"), Some("tsx"));
    }

    #[test]
    fn syntax_mapping_by_extension() {
        let hl = for_mode(true);
        assert!(hl.syntax_name_for_path("a.rs").contains("Rust"));
        assert!(hl.syntax_name_for_path("a.js").contains("JavaScript"));
        assert!(hl.syntax_name_for_path("a.py").contains("Python"));
        assert!(hl.syntax_name_for_path("a.md").contains("Markdown"));
        // `ts` has no grammar in syntect's bundled defaults (Sublime's
        // default package set ships no TypeScript syntax) — it exercises
        // the plain-text fallback, as do unknown extensions and names
        // without extensions.
        assert!(hl.syntax_name_for_path("a.ts").contains("Plain Text"));
        assert!(hl
            .syntax_name_for_path("a.certainly-unknown-ext")
            .contains("Plain Text"));
        assert!(hl.syntax_name_for_path("Makefile").contains("Plain Text"));
        // And the cache returns the same syntax afterwards.
        assert!(hl.syntax_name_for_path("b.rs").contains("Rust"));
    }

    #[test]
    fn run_splitting_is_stable_for_degenerate_lines() {
        let hl = for_mode(true);
        // Empty line: no runs at all.
        assert!(hl.highlight_line("a.rs", "", FALLBACK).is_empty());

        // Whitespace-only: runs concatenate back to the input.
        for line in [" ", "\t", "   \t  "] {
            let runs = hl.highlight_line("a.rs", line, FALLBACK);
            let joined: String = runs.iter().map(|r| r.text.as_str()).collect::<String>();
            assert_eq!(joined, line);
            assert!(runs.iter().all(|r| !r.text.is_empty()));
        }
    }

    #[test]
    fn plain_text_fallback_paints_everything_in_fallback() {
        let hl = for_mode(false);
        let runs = hl.highlight_line("file.unknown-ext", "just words here", FALLBACK);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "just words here");
        assert_eq!(runs[0].color, FALLBACK);
    }

    #[test]
    fn code_lines_produce_more_than_one_color() {
        let hl = for_mode(true);
        let runs = hl.highlight_line("a.rs", "pub fn main() { let x = 1; }", FALLBACK);
        assert!(runs.len() > 1, "expected keyword/string runs, got {runs:?}");
        let joined: String = runs.iter().map(|r| r.text.as_str()).collect::<String>();
        assert_eq!(joined, "pub fn main() { let x = 1; }");
    }

    #[test]
    fn very_long_lines_keep_a_plain_tail() {
        let hl = for_mode(true);
        let line = "x".repeat(MAX_HIGHLIGHT_CHARS + 500);
        let runs = hl.highlight_line("a.rs", &line, FALLBACK);
        // Nothing is dropped.
        let joined: String = runs.iter().map(|r| r.text.as_str()).collect::<String>();
        assert_eq!(joined.chars().count(), MAX_HIGHLIGHT_CHARS + 500);
        // The tail is a plain run of exactly the remainder.
        let tail = runs.last().expect("runs");
        assert_eq!(tail.color, FALLBACK);
        assert_eq!(tail.text.chars().count(), 500);
        // Short lines stay in one piece.
        let short = hl.highlight_line("a.rs", &"y".repeat(64), FALLBACK);
        assert_eq!(
            short.iter().map(|r| r.text.chars().count()).sum::<usize>(),
            64
        );
    }

    #[test]
    fn color_conversion_is_normalized() {
        let converted = to_iced(syntect::highlighting::Color {
            r: 0,
            g: 128,
            b: 255,
            a: 255,
        });
        assert!((converted.r - 0.0).abs() < f32::EPSILON);
        assert!((converted.g - 128.0 / 255.0).abs() < 1e-6);
        assert!((converted.b - 1.0).abs() < f32::EPSILON);
        assert_eq!(converted.a, 1.0);
        // Theme alpha is preserved.
        let translucent = to_iced(syntect::highlighting::Color {
            r: 10,
            g: 20,
            b: 30,
            a: 128,
        });
        assert!((translucent.a - 128.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn to_spans_without_word_ranges_maps_one_to_one() {
        let runs = vec![
            HLRun {
                text: "pub ".into(),
                color: Color::from_rgb8(0, 0, 255),
            },
            HLRun {
                text: "fn".into(),
                color: FALLBACK,
            },
        ];
        let spans = to_spans(runs, &[], None);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].color, Some(Color::from_rgb8(0, 0, 255)));
        assert!(spans.iter().all(|s| s.highlight.is_none()));
    }

    #[test]
    fn to_spans_splits_at_word_ranges_and_emphasizes_them() {
        let runs = vec![HLRun {
            text: "let changed = 1;".into(),
            color: FALLBACK,
        }];
        // "changed" occupies bytes 4..11.
        let spans = to_spans(runs, &[(4, 11)], Some(Color::from_rgb8(255, 0, 0)));
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].text, "let ");
        assert!(spans[0].highlight.is_none());
        assert_eq!(spans[1].text, "changed");
        assert!(spans[1].highlight.is_some());
        assert_eq!(spans[2].text, " = 1;");
        assert!(spans[2].highlight.is_none());
        let joined: String = spans.iter().map(|s| s.text.to_string()).collect();
        assert_eq!(joined, "let changed = 1;");
    }

    #[test]
    fn to_spans_handles_ranges_crossing_run_boundaries_and_clamps() {
        // Two runs; the word range spans the boundary between them.
        let runs = vec![
            HLRun {
                text: "ab".into(),
                color: FALLBACK,
            },
            HLRun {
                text: "cd".into(),
                color: Color::from_rgb8(0, 0, 255),
            },
        ];
        let spans = to_spans(runs, &[(1, 3)], Some(Color::from_rgb8(255, 0, 0)));
        let texts: Vec<&str> = spans.iter().map(|s| s.text.as_ref()).collect();
        assert_eq!(texts, vec!["a", "b", "c", "d"]);
        assert!(spans[0].highlight.is_none());
        assert!(spans[1].highlight.is_some());
        assert!(spans[2].highlight.is_some());
        assert!(spans[3].highlight.is_none());
        // Ranges beyond the text clamp to nothing.
        let spans = to_spans(
            vec![HLRun {
                text: "ab".into(),
                color: FALLBACK,
            }],
            &[(10, 20)],
            Some(Color::from_rgb8(255, 0, 0)),
        );
        assert_eq!(spans.len(), 1);
        assert!(spans[0].highlight.is_none());
    }

    #[test]
    fn enabled_switch_round_trips() {
        let before = enabled();
        set_enabled(!before);
        assert_eq!(enabled(), !before);
        set_enabled(before);
        assert_eq!(enabled(), before);
    }

    #[test]
    fn both_modes_build_and_share_their_instance() {
        let dark = for_mode(true);
        let light = for_mode(false);
        assert!(std::ptr::eq(dark, for_mode(true)));
        assert!(std::ptr::eq(light, for_mode(false)));
        assert!(!std::ptr::eq(dark, light));
        // Both can highlight the same line (different palettes).
        let a = dark.highlight_line("a.rs", "pub fn f() {}", FALLBACK);
        let b = light.highlight_line("a.rs", "pub fn f() {}", FALLBACK);
        assert_eq!(
            a.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(),
            b.iter().map(|r| r.text.as_str()).collect::<Vec<_>>()
        );
    }
}
