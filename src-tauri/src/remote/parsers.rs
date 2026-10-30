//! Pure parsers turning remote `git` CLI output into the shared DTO types.
//!
//! Every parser is a free function over a string so it unit-tests against
//! golden fixtures without a device (see the `#[cfg(test)]` block and
//! `testdata/`). Formats are chosen for machine parsing: porcelain v2
//! (status), numstat + unified (diff), NUL-delimited records (log),
//! porcelain (blame, worktrees), for-each-ref with tabs (branches/tags).

use crate::engine::types::{
    BlameLine, BranchInfo, ChangeKind, CommitInfo, GitSignature, StatusEntry, TagInfo, WorktreeInfo,
};

/// File extensions treated as images (mirrors the duplicated constant in
/// `engine/libgit2.rs` and `workspace.rs` so every layer agrees).
pub const IMAGE_EXTENSIONS: [&str; 9] = [
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico", "avif",
];

pub fn is_image_path(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("");
    let ext = ext.to_ascii_lowercase();
    // "tar.gz"-style tails and extensionless files never match an image ext.
    path.contains('.') && IMAGE_EXTENSIONS.contains(&ext.as_str())
}

// ---------- status (porcelain v2 -z --branch) ----------

/// Header fields from the `# branch.*` records.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusHeader {
    /// Commit sha HEAD points at ("(initial)" on an unborn branch).
    pub oid: String,
    /// Branch name, or "(detached)".
    pub head: String,
    /// Upstream full ref, when set.
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

fn change_kind(c: char) -> Option<ChangeKind> {
    Some(match c {
        '.' => ChangeKind::Unmodified,
        'A' => ChangeKind::Added,
        'M' => ChangeKind::Modified,
        'D' => ChangeKind::Deleted,
        'R' => ChangeKind::Renamed,
        'C' => ChangeKind::Copied,
        'U' => ChangeKind::Conflicted,
        'T' => ChangeKind::Modified, // typechange ≈ modified (no dedicated kind)
        '?' => ChangeKind::Untracked,
        '!' => ChangeKind::Ignored,
        _ => return None,
    })
}

/// Parse `git status --porcelain=v2 -z --branch` output.
pub fn parse_status(raw: &str) -> (StatusHeader, Vec<StatusEntry>) {
    let mut header = StatusHeader {
        oid: String::new(),
        head: String::new(),
        upstream: None,
        ahead: 0,
        behind: 0,
    };
    let mut entries = Vec::new();
    // With -z every field is NUL-terminated (rename records carry TWO NUL
    // fields: path then origPath).
    let fields: Vec<&str> = raw.split('\0').collect();
    let mut i = 0;
    while i < fields.len() {
        let field = fields[i];
        if field.is_empty() {
            i += 1;
            continue;
        }
        let mut tokens = field.split_whitespace();
        match tokens.next() {
            Some("#") => match tokens.next() {
                Some("branch.oid") => header.oid = tokens.next().unwrap_or("").to_string(),
                Some("branch.head") => header.head = tokens.next().unwrap_or("").to_string(),
                Some("branch.upstream") => header.upstream = tokens.next().map(|s| s.to_string()),
                Some("branch.ab") => {
                    for part in tokens {
                        if let Some(n) = part.strip_prefix('+') {
                            header.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            header.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            },
            Some("1") => {
                // 1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>
                let mut t = field.split(' ');
                let xy = t.nth(1).unwrap_or("..");
                let path = t.next_back().unwrap_or("");
                let index = change_kind(xy.as_bytes()[0] as char);
                let worktree = change_kind(xy.as_bytes()[1] as char);
                if let (Some(index), Some(worktree)) = (index, worktree) {
                    entries.push(StatusEntry {
                        path: path.to_string(),
                        old_path: None,
                        index,
                        worktree,
                    });
                }
            }
            Some("2") => {
                // 2 <XY> ... <path> NUL <origPath>
                let mut t = field.split(' ');
                let xy = t.nth(1).unwrap_or("..");
                let path = t.next_back().unwrap_or("");
                let orig = fields.get(i + 1).copied().unwrap_or("");
                i += 1; // consume the origPath field
                let index = change_kind(xy.as_bytes()[0] as char);
                let worktree = change_kind(xy.as_bytes()[1] as char);
                if let (Some(index), Some(worktree)) = (index, worktree) {
                    entries.push(StatusEntry {
                        path: path.to_string(),
                        old_path: if orig.is_empty() {
                            None
                        } else {
                            Some(orig.to_string())
                        },
                        index,
                        worktree,
                    });
                }
            }
            Some("u") => {
                // u <XY> <sub> <m1..3> <mW> <h1..3> <path>
                let mut t = field.split(' ');
                let path = t.next_back().unwrap_or("");
                entries.push(StatusEntry {
                    path: path.to_string(),
                    old_path: None,
                    index: ChangeKind::Conflicted,
                    worktree: ChangeKind::Conflicted,
                });
            }
            Some("?") => {
                entries.push(StatusEntry {
                    path: field[2..].to_string(),
                    old_path: None,
                    index: ChangeKind::Unmodified,
                    worktree: ChangeKind::Untracked,
                });
            }
            Some("!") => {
                entries.push(StatusEntry {
                    path: field[2..].to_string(),
                    old_path: None,
                    index: ChangeKind::Unmodified,
                    worktree: ChangeKind::Ignored,
                });
            }
            _ => {}
        }
        i += 1;
    }
    (header, entries)
}

// ---------- diff (numstat -z + unified -U3) ----------

/// One `--numstat -z` record: additions, deletions (both `-` for binary),
/// optional `R100\0old\0new` rename triple, path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumStatRow {
    pub path: String,
    pub old_path: Option<String>,
    pub binary: bool,
    pub additions: u32,
    pub deletions: u32,
}

/// Parse `git diff --numstat -z` output. Layout: `add<TAB>del<TAB>status`
/// followed by the path as its own NUL field; renames carry TWO path fields
/// (origPath, then path).
pub fn parse_numstat(raw: &str) -> Vec<NumStatRow> {
    let fields: Vec<&str> = raw.split('\0').collect();
    let mut rows = Vec::new();
    let mut i = 0;
    while i < fields.len() {
        let meta = fields[i];
        if meta.is_empty() {
            i += 1;
            continue;
        }
        let mut parts = meta.splitn(3, '\t');
        let first = parts.next().unwrap_or("");
        let second = parts.next();
        // Rename/copy records carry ONLY the status in the meta field
        // ("R100"); normal records carry "add<TAB>del<TAB>status".
        let status = match second {
            None => first,
            Some(_) => parts.next().unwrap_or("A"),
        };
        let (add, del) = match second {
            None => (None, None),
            Some(d) => match (first.parse::<u32>(), d.parse::<u32>()) {
                (Ok(a), Ok(b)) => (Some(a), Some(b)),
                _ => (None, None), // "-" — binary
            },
        };
        if status.starts_with('R') || status.starts_with('C') {
            let old = fields.get(i + 1).copied().unwrap_or("");
            let new = fields.get(i + 2).copied().unwrap_or("");
            i += 3;
            rows.push(NumStatRow {
                path: new.to_string(),
                old_path: Some(old.to_string()),
                binary: add.is_none(),
                additions: add.unwrap_or(0),
                deletions: del.unwrap_or(0),
            });
        } else {
            let path = fields.get(i + 1).copied().unwrap_or("");
            i += 2;
            rows.push(NumStatRow {
                path: path.to_string(),
                old_path: None,
                binary: add.is_none(),
                additions: add.unwrap_or(0),
                deletions: del.unwrap_or(0),
            });
        }
    }
    rows
}

/// One file's patch: the leading headers + its `@@` hunks as raw text.
#[derive(Debug, Clone, Default)]
pub struct PatchFile {
    /// `diff --git …` through `+++ …` headers.
    pub header: String,
    /// Raw hunk texts (each starting with `@@`).
    pub hunks: Vec<String>,
    /// Path as parsed from the `+++`/`diff --git` line (b/-side).
    pub path: String,
}

/// Split a unified diff into per-file patches.
pub fn split_patch(raw: &str) -> Vec<PatchFile> {
    let mut files: Vec<PatchFile> = Vec::new();
    let mut current: Option<PatchFile> = None;
    for line in raw.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            if let Some(done) = current.take() {
                files.push(done);
            }
            current = Some(PatchFile {
                path: b_side_of_diff_line(line),
                ..PatchFile::default()
            });
        }
        if let Some(file) = current.as_mut() {
            if line.starts_with("@@ ") {
                file.hunks.push(line.to_string());
            } else if file.hunks.is_empty() {
                file.header.push_str(line);
            } else {
                // Continuation of the current hunk (context/+/-/\ lines).
                if let Some(h) = file.hunks.last_mut() {
                    h.push_str(line);
                }
            }
        }
    }
    if let Some(done) = current.take() {
        files.push(done);
    }
    files
}

/// `b/…` path from a `diff --git a/x b/x` line (handles quoted/odd names
/// minimally: the b-side is everything after the last " b/").
fn b_side_of_diff_line(line: &str) -> String {
    let line = line.trim_end();
    let marker = " b/";
    match line.rfind(marker) {
        Some(idx) => line[idx + marker.len()..].to_string(),
        None => String::new(),
    }
}

fn signature_from(name: &str, email: &str, time_unix: i64, tz: &str) -> GitSignature {
    GitSignature {
        name: name.to_string(),
        email: email.to_string(),
        time: time_unix,
        offset_minutes: tz_offset_minutes(tz),
    }
}

/// "+0530" → 330; "-0800" → -480; garbage → 0.
pub fn tz_offset_minutes(tz: &str) -> i32 {
    let tz = tz.trim();
    let (sign, digits) = match tz.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, tz.strip_prefix('+').unwrap_or(tz)),
    };
    if digits.len() < 3 {
        return 0;
    }
    let (h, m) = digits.split_at(digits.len() - 2);
    let hours: i32 = h.parse().unwrap_or(0);
    let mins: i32 = m.parse().unwrap_or(0);
    sign * (hours * 60 + mins)
}

// ---------- log (NUL/record-separator format) ----------

/// Parse one page of `git log --format=%H%x00%P%x00%an%x00%ae%x00%at%x00%ai
/// %x00%cn%x00%ce%x00%ct%x00%ci%x00%D%x00%B%x01` output.
pub fn parse_log_records(raw: &str) -> Vec<CommitInfo> {
    let mut commits = Vec::new();
    for record in raw.split('\u{1}') {
        let record = record.strip_prefix('\n').unwrap_or(record);
        if record.trim().is_empty() {
            continue;
        }
        // H\0P\0an\0ae\0at\0ai\0cn\0ce\0ct\0ci\0D\0B
        let mut fields = record.splitn(12, '\u{0}');
        let sha = fields.next().unwrap_or("").to_string();
        if sha.is_empty() {
            continue;
        }
        let parents: Vec<String> = fields
            .next()
            .unwrap_or("")
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        let an = fields.next().unwrap_or("");
        let ae = fields.next().unwrap_or("");
        let at: i64 = fields.next().unwrap_or("0").parse().unwrap_or(0);
        let ai = fields.next().unwrap_or("");
        let cn = fields.next().unwrap_or("");
        let ce = fields.next().unwrap_or("");
        let ct: i64 = fields.next().unwrap_or("0").parse().unwrap_or(0);
        let ci = fields.next().unwrap_or("");
        let refs_field = fields.next().unwrap_or("");
        let message = fields.next().unwrap_or("");
        // ISO tails carry the numeric timezone (%ai = 2024-01-02 10:20:30 +0530).
        let author_tz = ai.rsplit(' ').next().unwrap_or("");
        let committer_tz = ci.rsplit(' ').next().unwrap_or("");
        let summary = message.lines().next().unwrap_or("").to_string();
        commits.push(CommitInfo {
            sha,
            parents,
            author: signature_from(an, ae, at, author_tz),
            committer: signature_from(cn, ce, ct, committer_tz),
            message: message.trim_end_matches('\n').to_string(),
            summary,
            refs: parse_decorations(refs_field),
        });
    }
    commits
}

/// Split git's `%D` decoration list ("HEAD -> main, origin/main, tag: v1").
fn parse_decorations(field: &str) -> Vec<String> {
    let field = field.trim();
    if field.is_empty() {
        return Vec::new();
    }
    field.split(", ").map(|s| s.trim().to_string()).collect()
}

// ---------- blame (porcelain) ----------

/// Parse `git blame --porcelain` output into BlameLines (in output order —
/// the caller must not assume sorted order; git emits by line).
pub fn parse_blame(raw: &str) -> Vec<BlameLine> {
    use std::collections::HashMap;
    struct CommitMeta {
        author: GitSignature,
        summary: String,
    }
    let mut meta: HashMap<String, CommitMeta> = HashMap::new();
    let mut lines_out: Vec<BlameLine> = Vec::new();
    let mut current_sha: Option<String> = None;
    let mut final_line = 0u32;

    let flush_line = |lines_out: &mut Vec<BlameLine>,
                      sha: &Option<String>,
                      final_line: u32,
                      text: &str,
                      meta: &HashMap<String, CommitMeta>| {
        if let Some(sha) = sha {
            let entry = meta.get(sha);
            let author = entry
                .map(|m| m.author.clone())
                .unwrap_or_else(|| GitSignature {
                    name: String::new(),
                    email: String::new(),
                    time: 0,
                    offset_minutes: 0,
                });
            lines_out.push(BlameLine {
                line_no: final_line,
                sha: sha.clone(),
                signature: author.clone(),
                final_sha: sha.clone(),
                final_signature: author,
                text: text.to_string(),
            });
        }
    };

    for line in raw.split('\n') {
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('\t') {
            flush_line(&mut lines_out, &current_sha, final_line, rest, &meta);
            continue;
        }
        let mut tokens = line.splitn(3, ' ');
        let first = tokens.next().unwrap_or("");
        // Commit header: "<40-sha> <orig-line> <final-line> [<num>]" OR
        // previously-seen shas appear alone with just "<sha> <final-line>".
        if first.len() >= 40 && first.chars().all(|c| c.is_ascii_hexdigit()) {
            let sha = first.to_string();
            let nums: Vec<u32> = line
                .split(' ')
                .skip(1)
                .filter_map(|t| t.parse().ok())
                .collect();
            // Full header "<sha> <orig> <final> [<num>]" has 2-3 numbers;
            // the short repeat form "<sha> <final>" has exactly 1.
            final_line = match nums.len() {
                0 => 0,
                1 => nums[0],
                _ => nums[1],
            };
            current_sha = Some(sha);
            continue;
        }
        // Metadata for the current commit.
        if let Some(sha) = current_sha.clone() {
            let entry = meta.entry(sha).or_insert_with(|| CommitMeta {
                author: GitSignature {
                    name: String::new(),
                    email: String::new(),
                    time: 0,
                    offset_minutes: 0,
                },
                summary: String::new(),
            });
            if let Some(v) = line.strip_prefix("author ") {
                entry.author.name = v.to_string();
            } else if let Some(v) = line.strip_prefix("author-mail ") {
                entry.author.email = v.trim_matches(|c| c == '<' || c == '>').to_string();
            } else if let Some(v) = line.strip_prefix("author-time ") {
                entry.author.time = v.parse().unwrap_or(0);
            } else if let Some(v) = line.strip_prefix("author-tz ") {
                entry.author.offset_minutes = tz_offset_minutes(v);
            } else if let Some(v) = line.strip_prefix("summary ") {
                entry.summary = v.to_string();
            }
        }
    }
    lines_out
}

// ---------- for-each-ref families ----------

/// `git for-each-ref --format=%(refname)%09%(objectname)` → (refname, sha).
pub fn parse_ref_pairs(raw: &str) -> Vec<(String, String)> {
    raw.lines()
        .filter_map(|l| {
            let (name, sha) = l.split_once('\t')?;
            Some((name.to_string(), sha.to_string()))
        })
        .collect()
}

/// `git for-each-ref refs/heads --format=%(refname:short)%09%(objectname)
/// %09%(HEAD)%09%(upstream:short)%09%(upstream:track)`.
pub fn parse_branches(raw: &str) -> Vec<BranchInfo> {
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let mut f = line.split('\t');
            let name = f.next().unwrap_or("").to_string();
            let sha = f.next().unwrap_or("").to_string();
            let is_head = f.next().unwrap_or("") == "*";
            let upstream = f.next().unwrap_or("");
            let track = f.next().unwrap_or("");
            let (mut ahead, mut behind, mut gone) = (0, 0, false);
            if track.contains("gone") {
                gone = true;
            }
            for part in track.split(',') {
                let part = part.trim().trim_matches(|c| c == '[' || c == ']');
                if let Some(n) = part.strip_prefix("ahead ") {
                    ahead = n.trim().parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix("behind ") {
                    behind = n.trim().parse().unwrap_or(0);
                }
            }
            BranchInfo {
                name,
                sha,
                upstream: if upstream.is_empty() {
                    None
                } else {
                    Some(upstream.to_string())
                },
                ahead,
                behind,
                gone,
                is_head,
            }
        })
        .collect()
}

/// Remote-tracking branches (`refs/remotes`), skipping the `<remote>/HEAD`
/// symlink entries.
pub fn parse_remote_branches(raw: &str) -> Vec<(String, String, String)> {
    // (remote, short name, sha)
    raw.lines()
        .filter_map(|l| {
            let (full, sha) = l.split_once('\t')?;
            let rest = full.strip_prefix("refs/remotes/")?;
            if rest.ends_with("/HEAD") {
                return None;
            }
            let (remote, name) = rest.split_once('/')?;
            Some((remote.to_string(), name.to_string(), sha.to_string()))
        })
        .collect()
}

/// Tags (`for-each-ref refs/tags` with objecttype/peeled/tagger/subject).
pub fn parse_tags(raw: &str) -> Vec<TagInfo> {
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let mut f = line.split('\t');
            let name = f
                .next()
                .unwrap_or("")
                .strip_prefix("refs/tags/")
                .unwrap_or("")
                .to_string();
            let sha = f.next().unwrap_or("").to_string();
            let objtype = f.next().unwrap_or("commit");
            let peeled = f.next().unwrap_or("");
            let tagger_line = f.next().unwrap_or("");
            let message = f.next().unwrap_or("");
            let annotated = objtype == "tag";
            let tagger = if annotated && !tagger_line.is_empty() {
                // "%(taggername)|%(taggeremail)|%(taggerdate:unix)|%(taggerdate:iso-strict)"
                let mut t = tagger_line.split('\u{1}');
                let name = t.next().unwrap_or("").to_string();
                let email = t
                    .next()
                    .unwrap_or("")
                    .trim_matches(|c| c == '<' || c == '>')
                    .to_string();
                let time: i64 = t.next().unwrap_or("0").parse().unwrap_or(0);
                let tz = t.next().unwrap_or("");
                Some(GitSignature {
                    name,
                    email,
                    time,
                    offset_minutes: tz_offset_minutes(tz),
                })
            } else {
                None
            };
            TagInfo {
                name,
                sha: sha.clone(),
                target: if peeled.is_empty() {
                    sha
                } else {
                    peeled.to_string()
                },
                annotated,
                tagger,
                message: if annotated && !message.is_empty() {
                    Some(message.to_string())
                } else {
                    None
                },
            }
        })
        .collect()
}

// ---------- stash ----------

/// `git stash list --format=%gd%x09%H%x09%an%x09%ae%x09%at%x09%gs`.
pub fn parse_stash_list(raw: &str) -> Vec<(u32, String, String, GitSignature)> {
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| {
            let mut f = line.split('\t');
            let selector = f.next()?; // stash@{2}
            let index: u32 = selector
                .trim_start_matches("stash@{")
                .trim_end_matches('}')
                .parse()
                .ok()?;
            let sha = f.next()?.to_string();
            let name = f.next().unwrap_or("").to_string();
            let email = f.next().unwrap_or("").to_string();
            let time: i64 = f.next().unwrap_or("0").parse().unwrap_or(0);
            let message = f.next().unwrap_or("").to_string();
            Some((
                index,
                sha,
                message,
                GitSignature {
                    name,
                    email,
                    time,
                    offset_minutes: 0,
                },
            ))
        })
        .collect()
}

// ---------- worktrees (porcelain) ----------

/// Parse `git worktree list --porcelain`.
pub fn parse_worktrees(raw: &str) -> Vec<WorktreeInfo> {
    let mut out: Vec<WorktreeInfo> = Vec::new();
    let mut path = String::new();
    let mut head: Option<String> = None;
    let mut branch: Option<String> = None;
    let mut detached = false;
    let mut locked = false;
    let mut prunable: Option<String> = None;
    let flush = |out: &mut Vec<WorktreeInfo>,
                 path: &mut String,
                 head: &mut Option<String>,
                 branch: &mut Option<String>,
                 detached: &mut bool,
                 locked: &mut bool,
                 prunable: &mut Option<String>| {
        if path.is_empty() {
            return;
        }
        let name = path.rsplit('/').next().unwrap_or("").to_string();
        out.push(WorktreeInfo {
            path: std::mem::take(path),
            name,
            branch: branch
                .take()
                .map(|b| b.trim_start_matches("refs/heads/").to_string()),
            head: head.take(),
            detached: std::mem::take(detached),
            locked: std::mem::take(locked),
            prunable: prunable.take(),
            is_main: out.is_empty(),
        });
    };
    for line in raw.lines() {
        if let Some(v) = line.strip_prefix("worktree ") {
            flush(
                &mut out,
                &mut path,
                &mut head,
                &mut branch,
                &mut detached,
                &mut locked,
                &mut prunable,
            );
            path = v.to_string();
        } else if let Some(v) = line.strip_prefix("HEAD ") {
            head = Some(v.to_string());
        } else if let Some(v) = line.strip_prefix("branch ") {
            branch = Some(v.to_string());
        } else if line == "detached" {
            detached = true;
        } else if line.starts_with("locked") {
            locked = true;
        } else if let Some(v) = line.strip_prefix("prunable ") {
            prunable = Some(v.to_string());
        }
    }
    flush(
        &mut out,
        &mut path,
        &mut head,
        &mut branch,
        &mut detached,
        &mut locked,
        &mut prunable,
    );
    out
}

// ---------- submodules ----------

/// Parse `git submodule status --recursive` lines:
/// `<status-char> <sha> <path> (<describe>)`.
pub fn parse_submodule_status(raw: &str) -> Vec<(char, String, String)> {
    raw.lines()
        .filter(|l| l.len() > 42)
        .map(|l| {
            let status = l.chars().next().unwrap_or(' ');
            let sha = l[1..41].to_string();
            let rest = &l[42..];
            let path = rest.split(" (").next().unwrap_or(rest).trim().to_string();
            (status, sha, path)
        })
        .collect()
}

// ---------- health ----------

/// Parse `git count-objects -v`.
pub fn parse_count_objects(raw: &str) -> (u64, u64, u64, u64, u64) {
    // (loose_count, loose_size_bytes, packs, pack_size_bytes, in_pack)
    let get = |key: &str| -> u64 {
        raw.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix(": ")?.parse().ok())
            .unwrap_or(0)
    };
    (
        get("count"),
        get("size") * 1024, // size is KiB
        get("packs"),
        get("size-pack") * 1024,
        get("in-pack"),
    )
}

/// Dangling shas from `git fsck --no-progress --dangling` (cap 10 applied by
/// the caller).
pub fn parse_fsck_dangling(raw: &str) -> Vec<String> {
    raw.lines()
        .filter_map(|l| l.strip_prefix("dangling "))
        .filter_map(|l| l.split(' ').nth(1))
        .map(|s| s.to_string())
        .collect()
}

// ---------- reflog ----------

/// Parse `git reflog --format=%H%x09%at%x09%an%x09%ae%x09%gs` (newest
/// first) into entries with chained old shas: old(i) = new(i+1).
pub fn parse_reflog(raw: &str) -> Vec<(String, String, GitSignature, String)> {
    // (old, new, signature, message)
    let rows: Vec<(&str, i64, &str, &str, &str)> = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| {
            let mut f = l.split('\t');
            let new = f.next()?;
            let at: i64 = f.next().unwrap_or("0").parse().unwrap_or(0);
            let an = f.next().unwrap_or("");
            let ae = f.next().unwrap_or("");
            let msg = f.next().unwrap_or("");
            Some((new, at, an, ae, msg))
        })
        .collect();
    let mut out = Vec::new();
    for (i, (new, at, an, ae, msg)) in rows.iter().enumerate() {
        let old = rows
            .get(i + 1)
            .map(|(prev, ..)| prev.to_string())
            .unwrap_or_else(|| "0000000000000000000000000000000000000000".to_string());
        out.push((
            old,
            new.to_string(),
            GitSignature {
                name: an.to_string(),
                email: ae.to_string(),
                time: *at,
                offset_minutes: 0,
            },
            msg.to_string(),
        ));
    }
    out
}

// ---------- rebase / bisect ----------

/// One line of a rebase todo file: `pick abc123 Message…`.
pub fn parse_rebase_todo(raw: &str) -> Vec<(String, String, String)> {
    // (action, sha, message) — comment lines are skipped.
    raw.lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .filter_map(|l| {
            let mut t = l.splitn(3, ' ');
            let action = t.next()?.trim().to_string();
            let sha = t.next()?.trim().to_string();
            let msg = t.next().unwrap_or("").trim().to_string();
            Some((action, sha, msg))
        })
        .collect()
}

/// Parse `git bisect log` into (marks, orig_head-ish info). Marks: lines
/// `# good: [sha] subject` / `git bisect good <sha>`.
pub fn parse_bisect_log(raw: &str) -> Vec<(String, String)> {
    // (mark, sha) oldest first.
    let mut out = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if let Some(rest) = line
            .strip_prefix("git bisect ")
            .and_then(|r| r.strip_prefix("good "))
        {
            out.push(("good".into(), first_sha(rest)));
        } else if let Some(rest) = line
            .strip_prefix("git bisect ")
            .and_then(|r| r.strip_prefix("bad "))
        {
            out.push(("bad".into(), first_sha(rest)));
        } else if let Some(rest) = line
            .strip_prefix("git bisect ")
            .and_then(|r| r.strip_prefix("skip "))
        {
            out.push(("skip".into(), first_sha(rest)));
        }
    }
    out
}

fn first_sha(s: &str) -> String {
    s.split_whitespace().next().unwrap_or("").to_string()
}

// ---------- network progress ----------

/// Accrued fetch/push progress parsed from one stderr line.
#[derive(Debug, Clone, Copy, Default)]
pub struct TransferProgress {
    pub phase_objects: bool,
    pub objects_received: u32,
    pub objects_total: u32,
    pub bytes: u64,
    pub done: bool,
}

/// Recognize git's transfer progress lines:
/// "Receiving objects:  45% (56/123)", "Writing objects: 100% (9/9), 1.2 KiB",
/// "Resolving deltas: 100% (7/7)", "Updating files: ..."
pub fn parse_transfer_line(line: &str) -> Option<TransferProgress> {
    let line = line.trim_start_matches(['\r', '\n']);
    for prefix in [
        "Receiving objects:",
        "Writing objects:",
        "Enumerating objects:",
        "Counting objects:",
        "Compressing objects:",
        "Resolving deltas:",
        "Updating files:",
    ] {
        if let Some(rest) = line.strip_prefix(prefix) {
            let mut progress = TransferProgress {
                phase_objects: prefix.contains("objects") && !prefix.starts_with("Enumerating"),
                done: false,
                ..TransferProgress::default()
            };
            if let (Some(open), Some(close)) = (rest.find('('), rest.find('/')) {
                if close > open {
                    progress.objects_received = rest[open + 1..close].trim().parse().unwrap_or(0);
                    let close_rest = &rest[close + 1..];
                    let total: String = close_rest
                        .chars()
                        .take_while(|c| c.is_ascii_digit())
                        .collect();
                    progress.objects_total = total.parse().unwrap_or(0);
                    if let Some(pct) = rest.trim().split('%').next() {
                        progress.done =
                            pct.trim().parse::<u32>().map(|p| p >= 100).unwrap_or(false)
                                || prefix.starts_with("Resolving") && rest.contains("100%");
                    }
                }
            }
            // "…, 1.21 MiB | 3.5 MiB/s" — first size token.
            for token in rest.split(", ") {
                let token = token.trim();
                if let Some(bytes) = parse_size_token(token) {
                    progress.bytes = bytes;
                    break;
                }
            }
            return Some(progress);
        }
    }
    None
}

fn parse_size_token(token: &str) -> Option<u64> {
    let token = token.trim();
    let digits: String = token
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let value: f64 = digits.parse().ok()?;
    let unit: String = token[digits.len()..]
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_ascii_lowercase();
    let mult = match unit.as_str() {
        "b" => 1.0,
        "kib" => 1024.0,
        "mib" => 1024.0 * 1024.0,
        "gib" => 1024.0 * 1024.0 * 1024.0,
        "kb" => 1000.0,
        "mb" => 1000.0 * 1000.0,
        "gb" => 1000.0 * 1000.0 * 1000.0,
        _ => return None,
    };
    Some((value * mult) as u64)
}

/// Redact credential-prompt-ish lines from raw remote output passthrough
/// (`action-output` / op-log streams — spec: "redact prompt-ish lines in
/// op-log passthrough"). Remote lines reach the FE verbatim; a line that
/// looks like a credential prompt (or anything feeding one) is replaced
/// with a fixed placeholder instead of being relayed. Over-matching is
/// deliberate: a wrongly-redacted build log line is cosmetic, a relayed
/// credential prompt is a leak.
pub fn redact_prompt(line: &str) -> String {
    const MARKERS: [&str; 6] = [
        "password",
        "passphrase",
        "username for ",
        "username:",
        "token:",
        "api key:",
    ];
    let lower = line.to_ascii_lowercase();
    if MARKERS.iter().any(|m| lower.contains(m)) {
        "[redacted: credential prompt]".to_string()
    } else {
        line.to_string()
    }
}

/// Updated ref pairs from fetch/push stderr ("old..new  branch -> target").
pub fn parse_updated_refs(stderr: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in stderr.lines() {
        let Some((left, right)) = line.split_once(" -> ") else {
            continue;
        };
        let target = right.split(" (").next().unwrap_or("").trim().to_string();
        if target.is_empty() {
            continue;
        }
        let left = left.trim();
        // New sha is the token after the range separator, when present.
        let new_sha = if let Some(idx) = left.find("..") {
            let after = &left[idx + 2..];
            let sha: String = after
                .chars()
                .take_while(|c| c.is_ascii_hexdigit())
                .collect();
            sha
        } else {
            String::new() // [new branch] — sha not shown on the line
        };
        out.push((target, new_sha));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::{DiffHunk, DiffLine, FileDiff};

    #[test]
    fn prompt_ish_lines_are_redacted() {
        assert_eq!(
            redact_prompt("Password for raul@dev:"),
            "[redacted: credential prompt]"
        );
        assert_eq!(
            redact_prompt("Enter passphrase for key '/home/u/.ssh/id_ed25519':"),
            "[redacted: credential prompt]"
        );
        assert_eq!(
            redact_prompt("Username for 'https://github.com':"),
            "[redacted: credential prompt]"
        );
        assert_eq!(
            redact_prompt("  TOKEN: abc123"),
            "[redacted: credential prompt]"
        );
        assert_eq!(
            redact_prompt("API KEY: deadbeef"),
            "[redacted: credential prompt]"
        );
    }

    #[test]
    fn ordinary_output_passes_through() {
        assert_eq!(redact_prompt("main -> origin/main"), "main -> origin/main");
        assert_eq!(redact_prompt(""), "");
        // "keypad" contains no marker; a standalone "key" never matches.
        assert_eq!(redact_prompt(" keypad hit 4"), " keypad hit 4");
    }

    #[test]
    fn status_parses_header_and_entries() {
        let raw = "# branch.oid 0123456789012345678901234567890123456789\u{0}\
# branch.head main\u{0}\
# branch.upstream origin/main\u{0}\
# branch.ab +2 -1\u{0}\
1 .M NNNN 100644 100644 100644 hH hI src/a.rs\u{0}\
1 M. NNNN 100644 100644 100644 hH hI src/b.rs\u{0}\
? notes.txt\u{0}\
u UU UUU 0 0 0 0 0 0 conflicted.txt\u{0}\
";
        let (header, entries) = parse_status(raw);
        assert_eq!(header.oid, "0123456789012345678901234567890123456789");
        assert_eq!(header.head, "main");
        assert_eq!(header.upstream.as_deref(), Some("origin/main"));
        assert_eq!(header.ahead, 2);
        assert_eq!(header.behind, 1);
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].path, "src/a.rs");
        assert_eq!(entries[0].index, ChangeKind::Unmodified);
        assert_eq!(entries[0].worktree, ChangeKind::Modified);
        assert_eq!(entries[1].index, ChangeKind::Modified);
        assert_eq!(entries[2].worktree, ChangeKind::Untracked);
        assert_eq!(entries[3].index, ChangeKind::Conflicted);
    }

    #[test]
    fn status_rename_and_detached() {
        let raw = "# branch.oid 0123456789012345678901234567890123456789\u{0}\
# branch.head (detached)\u{0}\
2 R. NNNN 100644 100644 100644 hH hI R100 new.txt\u{0}old.txt\u{0}\
";
        let (header, entries) = parse_status(raw);
        assert!(header.head == "(detached)");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "new.txt");
        assert_eq!(entries[0].old_path.as_deref(), Some("old.txt"));
        assert_eq!(entries[0].index, ChangeKind::Renamed);
    }

    #[test]
    fn numstat_parses_binary_and_renames() {
        let raw = "3\t1\tM\u{0}README.md\u{0}-\t-\t-\u{0}assets/logo.png\u{0}R100\u{0}old.rs\u{0}new.rs\u{0}";
        let rows = parse_numstat(raw);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].path, "README.md");
        assert_eq!(rows[0].additions, 3);
        assert_eq!(rows[0].deletions, 1);
        assert!(!rows[0].binary);
        assert!(rows[1].binary);
        assert_eq!(rows[1].path, "assets/logo.png");
        assert_eq!(rows[2].path, "new.rs");
        assert_eq!(rows[2].old_path.as_deref(), Some("old.rs"));
    }

    #[test]
    fn split_patch_extracts_headers_and_hunks() {
        let raw = "diff --git a/a.txt b/a.txt\n\
index 111..222 100644\n\
--- a/a.txt\n\
+++ b/a.txt\n\
@@ -1,2 +1,3 @@\n\
 one\n\
-two\n\
+TWO\n\
+three\n\
diff --git a/b.txt b/b.txt\n\
--- a/b.txt\n\
+++ b/b.txt\n\
@@ -1 +1 @@\n\
-x\n\
+y\n\
";
        let files = split_patch(raw);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "a.txt");
        assert_eq!(files[0].hunks.len(), 1);
        assert!(files[0].hunks[0].starts_with("@@ -1,2 +1,3 @@"));
        assert!(files[0].hunks[0].contains("+three\n"));
        assert_eq!(files[1].path, "b.txt");
        assert!(files[0].header.contains("index 111..222"));
    }

    #[test]
    fn diff_hunks_map_to_lines() {
        let files = split_patch(
            "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,2 @@\n keep\n-del\n+add\n",
        );
        let patch = &files[0];
        let mut hunks = Vec::new();
        for hunk in &patch.hunks {
            let mut lines = Vec::new();
            let header_end = hunk.find('\n').unwrap_or(hunk.len());
            let header = &hunk[..header_end];
            let body = &hunk[header_end + 1..];
            let nums: Vec<u32> = header
                .trim_start_matches("@@")
                .trim_end_matches("@@")
                .trim()
                .split(" ")
                .map(|r| r[1..].split(',').next().unwrap_or("0").parse().unwrap_or(0))
                .collect();
            let (mut old_no, mut new_no) = (nums[0], nums[1]);
            for l in body.split('\n').filter(|l| !l.is_empty()) {
                match l.chars().next() {
                    Some('+') => {
                        lines.push(DiffLine {
                            old_no: None,
                            new_no: Some(new_no),
                            origin: '+',
                            text: l[1..].to_string(),
                            highlights: vec![],
                        });
                        new_no += 1;
                    }
                    Some('-') => {
                        lines.push(DiffLine {
                            old_no: Some(old_no),
                            new_no: None,
                            origin: '-',
                            text: l[1..].to_string(),
                            highlights: vec![],
                        });
                        old_no += 1;
                    }
                    Some(' ') => {
                        lines.push(DiffLine {
                            old_no: Some(old_no),
                            new_no: Some(new_no),
                            origin: ' ',
                            text: l[1..].to_string(),
                            highlights: vec![],
                        });
                        old_no += 1;
                        new_no += 1;
                    }
                    _ => {}
                }
            }
            hunks.push(DiffHunk {
                old_start: nums[0],
                new_start: nums[1],
                lines,
            });
        }
        let fd = FileDiff {
            path: patch.path.clone(),
            old_path: None,
            binary: false,
            is_image: is_image_path(&patch.path),
            additions: 1,
            deletions: 1,
            hunks,
        };
        assert_eq!(fd.hunks[0].lines.len(), 3);
        assert_eq!(fd.hunks[0].lines[0].text, "keep");
        assert_eq!(fd.hunks[0].lines[1].origin, '-');
        assert_eq!(fd.hunks[0].lines[2].new_no, Some(2));
    }

    const GOLDEN_LOG: &str = "0123456789abcdef0123456789abcdef01234567\u{0}89abcdef\u{0}\
Alice A\u{0}alice@x.io\u{0}1700000000\u{0}2024-01-02 10:20:30 +0530\u{0}\
Alice A\u{0}alice@x.io\u{0}1700000100\u{0}2024-01-02 10:21:40 +0530\u{0}\
HEAD -> main, origin/main\u{0}\
subject line\n\nbody line\n\u{1}";

    #[test]
    fn log_records_parse() {
        let commits = parse_log_records(GOLDEN_LOG);
        assert_eq!(commits.len(), 1);
        let c = &commits[0];
        assert_eq!(c.sha, "0123456789abcdef0123456789abcdef01234567");
        assert_eq!(c.parents, vec!["89abcdef"]);
        assert_eq!(c.author.name, "Alice A");
        assert_eq!(c.author.email, "alice@x.io");
        assert_eq!(c.author.time, 1700000000);
        assert_eq!(c.author.offset_minutes, 330);
        assert_eq!(c.committer.time, 1700000100);
        assert_eq!(c.summary, "subject line");
        assert!(c.message.ends_with("body line"));
        assert_eq!(c.refs, vec!["HEAD -> main", "origin/main"]);
    }

    const GOLDEN_BLAME: &str = "0123456789012345678901234567890123456789 1 1 1\n\
author Bob\n\
author-mail <bob@x.io>\n\
author-time 1700000000\n\
author-tz +0200\n\
summary fix\n\
filename f.txt\n\
\tfirst line\n\
0123456789012345678901234567890123456789 2 2\n\
\tsecond line\n\
";

    #[test]
    fn blame_porcelain_parses() {
        let lines = parse_blame(GOLDEN_BLAME);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].line_no, 1);
        assert_eq!(lines[0].signature.name, "Bob");
        assert_eq!(lines[0].signature.email, "bob@x.io");
        assert_eq!(lines[0].text, "first line");
        assert_eq!(lines[1].line_no, 2);
        assert_eq!(lines[1].text, "second line");
    }

    #[test]
    fn branches_parse_track_fields() {
        let raw =
            "main\t0123456789abcdef0123456789abcdef01234567\t*\torigin/main\t[ahead 2, behind 1]\n\
dev\t1111111111111111111111111111111111111111\t \torigin/dev\t[gone]\n\
local\t2222222222222222222222222222222222222222\t \t\t\n";
        let branches = parse_branches(raw);
        assert_eq!(branches.len(), 3);
        assert!(branches[0].is_head);
        assert_eq!(branches[0].ahead, 2);
        assert_eq!(branches[0].behind, 1);
        assert_eq!(branches[0].upstream.as_deref(), Some("origin/main"));
        assert!(branches[1].gone);
        assert!(!branches[2].is_head);
        assert!(branches[2].upstream.is_none());
    }

    #[test]
    fn worktrees_porcelain_parses() {
        let raw = "worktree /repo\nHEAD 0123456789012345678901234567890123456789\nbranch refs/heads/main\n\nworktree /repo/wt2\nHEAD 1111111111111111111111111111111111111111\ndetached\n";
        let wts = parse_worktrees(raw);
        assert_eq!(wts.len(), 2);
        assert!(wts[0].is_main);
        assert_eq!(wts[0].branch.as_deref(), Some("main"));
        assert!(wts[1].detached);
        assert_eq!(wts[1].name, "wt2");
    }

    #[test]
    fn count_objects_and_fsck() {
        let raw = "count: 4\nsize: 2\nin-pack: 300\npacks: 2\nsize-pack: 45\nprune-packable: 0\ngarbage: 0\n";
        let (loose, loose_b, packs, pack_b, in_pack) = parse_count_objects(raw);
        assert_eq!(
            (loose, loose_b, packs, pack_b, in_pack),
            (4, 2048, 2, 46080, 300)
        );
        let dangling = parse_fsck_dangling("dangling commit 0123456789012345678901234567890123456789\ndangling blob abc\nchecking objects\n");
        assert_eq!(dangling.len(), 2);
        assert_eq!(dangling[0], "0123456789012345678901234567890123456789");
    }

    #[test]
    fn reflog_chains_old_sha() {
        let raw =
            "aaa\t1700000001\tA\ta@x\tcommit: third\nbbb\t1700000000\tA\ta@x\tcommit: second\n";
        let entries = parse_reflog(raw);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].1, "aaa"); // newest new-sha
        assert_eq!(entries[0].0, "bbb"); // old = next entry's new
        assert_eq!(entries[1].0, "0000000000000000000000000000000000000000");
    }

    #[test]
    fn transfer_lines_parse() {
        let p = parse_transfer_line("Receiving objects:  45% (56/123)").unwrap();
        assert!(p.phase_objects);
        assert_eq!(p.objects_received, 56);
        assert_eq!(p.objects_total, 123);
        let p = parse_transfer_line("Writing objects: 100% (9/9), 1.21 MiB | 3.5 MiB/s").unwrap();
        assert!(p.done);
        assert_eq!(p.bytes, 1_268_776); // 1.21 MiB, truncated
    }

    #[test]
    fn updated_refs_parse() {
        let stderr = "From origin\n   1111111..2222222  main       -> origin/main\n * [new branch]      feat       -> origin/feat\n";
        let refs = parse_updated_refs(stderr);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0], ("origin/main".into(), "2222222".into()));
        assert_eq!(refs[1], ("origin/feat".into(), String::new()));
    }

    #[test]
    fn rebase_todo_and_bisect_log() {
        let todo =
            "pick 1234567 first\n# comment\nfixup 89abcde squashed\nsquash def0123 message\n";
        let plan = parse_rebase_todo(todo);
        assert_eq!(plan.len(), 3);
        assert_eq!(plan[0], ("pick".into(), "1234567".into(), "first".into()));
        assert_eq!(plan[1].0, "fixup");

        let blog = "# bad: [aaa] bad commit\n# good: [bbb] good one\ngit bisect good ccc\ngit bisect bad ddd\n";
        let marks = parse_bisect_log(blog);
        // '# '-prefixed history lines are comments carrying marks too, but our
        // parser deliberately reads only the command lines (replayable form).
        assert_eq!(
            marks,
            vec![("good".into(), "ccc".into()), ("bad".into(), "ddd".into())]
        );
    }

    #[test]
    fn tz_offsets() {
        assert_eq!(tz_offset_minutes("+0530"), 330);
        assert_eq!(tz_offset_minutes("-0800"), -480);
        assert_eq!(tz_offset_minutes("+0000"), 0);
        assert_eq!(tz_offset_minutes(""), 0);
    }

    #[test]
    fn image_paths() {
        assert!(is_image_path("a/b.PNG"));
        assert!(is_image_path("x.svg"));
        assert!(!is_image_path("a.txt"));
        assert!(!is_image_path("noext"));
        assert!(!is_image_path("a.tar.gz"));
    }

    #[test]
    fn stash_and_submodules() {
        let stashes = parse_stash_list("stash@{0}\t0123456789012345678901234567890123456789\tAnn\ta@x\t1700000000\tWIP on main: abc msg\n");
        assert_eq!(stashes.len(), 1);
        assert_eq!(stashes[0].0, 0);
        assert_eq!(stashes[0].2, "WIP on main: abc msg");
        let subs = parse_submodule_status(" 0123456789012345678901234567890123456789 libs/goog (heads/main)\n-1111111111111111111111111111111111111111 libs/missing\n");
        assert_eq!(subs[0].2, "libs/goog");
        assert_eq!(subs[1].0, '-');
    }
}
