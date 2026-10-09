//! Memory soak for the workdir watcher + churn path (M12).
//!
//! Run (from src-tauri):
//! ```text
//! cargo run --release --example soak -- --minutes 30 --max-rss-mb 250
//! ```
//!
//! Builds a throwaway repo under the OS temp dir, then churns it at ~20
//! filesystem ops/second (create / modify / delete small files, plus churn
//! under a `.gitignore`d directory) while a recursive watcher consumes the
//! events — the same shape of load the app's open-repo watcher sees. Every
//! second the process RSS is sampled via `sysinfo`.
//!
//! Exits 1 when RSS exceeds `--max-rss-mb`, or when RSS grows monotonically
//! for 300 consecutive samples (10 minutes — a real leak trend, distinct
//! from noise). Prints max / median RSS at the end and removes the tempdir.
//!
//! NOTE: the app's own watcher lives in a crate-private module
//! (`mygitui_lib::watcher`), so this example drives `notify` directly with
//! the same recursive-watch + mpsc receiver pattern; the point is the
//! event-stream load on this process, not the debounce logic.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use git2::{IndexAddOption, Repository, Signature, Time};
use notify::{RecursiveMode, Watcher};
use sysinfo::{Pid, ProcessesToUpdate, System};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
/// Straight-line RSS growth (samples) before declaring a leak.
const MONOTONIC_FAIL_SAMPLES: u32 = 300;
/// Target ops per second.
const OPS_PER_SEC: u32 = 20;
/// Tracked-file pool the create/modify/delete cycle rotates through.
const POOL_SLOTS: usize = 200;

struct Args {
    minutes: u64,
    max_rss_mb: u64,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        minutes: 30,
        max_rss_mb: 250,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--minutes" => {
                args.minutes = it
                    .next()
                    .ok_or("--minutes needs a value")?
                    .parse()
                    .map_err(|_| "--minutes needs a number")?
            }
            "--max-rss-mb" => {
                args.max_rss_mb = it
                    .next()
                    .ok_or("--max-rss-mb needs a value")?
                    .parse()
                    .map_err(|_| "--max-rss-mb needs a number")?
            }
            other => {
                return Err(format!(
                    "unknown argument `{other}` (use --minutes, --max-rss-mb)"
                ))
            }
        }
    }
    Ok(args)
}

fn temp_repo() -> (PathBuf, Repository) {
    let dir = std::env::temp_dir().join(format!("mygitui-soak-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create tempdir");
    let repo = Repository::init(&dir).expect("init repo");
    {
        let mut cfg = repo.config().expect("config");
        let _ = cfg.set_str("user.name", "Soak");
        let _ = cfg.set_str("user.email", "soak@local");
    }
    // The ignored directory churns constantly; keep it out of git's way.
    std::fs::write(dir.join(".gitignore"), "ignored/\n").expect("write gitignore");
    let sig = Signature::new("Soak", "soak@local", &Time::new(1_700_000_000, 0)).unwrap();
    let tree_oid = {
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
            .unwrap();
        let tree_oid = index.write_tree().unwrap();
        index.write().unwrap();
        tree_oid
    };
    {
        let tree = repo.find_tree(tree_oid).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
    }
    (dir, repo)
}

fn pool_path(dir: &Path, slot: usize) -> PathBuf {
    let (sub, name) = match slot % 4 {
        0 => ("a", format!("f{slot}.txt")),
        1 => ("b", format!("f{slot}.txt")),
        2 => ("c", format!("f{slot}.txt")),
        _ => ("d", format!("f{slot}.txt")),
    };
    dir.join("pool").join(sub).join(name)
}

fn ignored_path(dir: &Path, slot: usize) -> PathBuf {
    dir.join("ignored")
        .join("churn")
        .join(format!("tmp{slot}.dat"))
}

fn main() {
    if let Err(msg) = run() {
        eprintln!("soak FAILED: {msg}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    println!(
        "soak: {} min, max RSS {} MB, ~{OPS_PER_SEC} ops/s",
        args.minutes, args.max_rss_mb
    );

    let (dir, repo) = temp_repo();
    // Recursive watcher over the repo, events drained into a channel — the
    // same shape as the app's per-repo watcher (its module is crate-private,
    // see the header note).
    let (tx, rx) = mpsc::channel();
    let mut watcher =
        notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
            let _ = tx.send(res);
        })
        .map_err(|e| format!("cannot create watcher: {e}"))?;
    let watched = dir.clone();
    watcher
        .watch(&watched, RecursiveMode::Recursive)
        .map_err(|e| format!("cannot watch {watched:?}: {e}"))?;
    // Repo handle is only held for its lock/config effects; drop it so the
    // churn loop sees a plain directory (watcher keeps the live side).
    drop(repo);

    let deadline = Instant::now() + Duration::from_secs(args.minutes * 60);
    let mut sys = System::new();
    let pid = Pid::from_u32(std::process::id());
    let mut samples: Vec<u64> = Vec::new();
    let mut growing_streak: u32 = 0;
    let mut prev_rss: Option<u64> = None;
    let mut last_sample = Instant::now();
    let mut op_no: u64 = 0;
    let step = Duration::from_millis(1000 / u64::from(OPS_PER_SEC));
    let mut next_op = Instant::now();

    let mut fail: Option<String> = None;
    while Instant::now() < deadline {
        // ---- churn step (one op) --------------------------------------
        let slot = (op_no as usize) % POOL_SLOTS;
        let phase = (op_no / POOL_SLOTS as u64) % 3;
        let tracked = pool_path(&dir, slot);
        match phase {
            0 => {
                if let Some(parent) = tracked.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&tracked, format!("payload {op_no}\n"));
            }
            1 => {
                let _ = std::fs::write(&tracked, format!("payload mod {op_no}\n"));
            }
            _ => {
                let _ = std::fs::remove_file(&tracked);
                // .gitignore-respected churn: same write volume under an
                // ignored directory (the app's watcher must absorb it too).
                let ignored = ignored_path(&dir, slot);
                if let Some(parent) = ignored.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&ignored, format!("junk {op_no}\n"));
            }
        }
        op_no += 1;
        next_op += step;
        let now = Instant::now();
        if next_op > now {
            std::thread::sleep(next_op - now);
        } else {
            next_op = now; // fell behind: do not sleep-catch-up in a burst
        }

        // ---- 1 Hz RSS sample -------------------------------------------
        if last_sample.elapsed() >= SAMPLE_INTERVAL {
            last_sample += SAMPLE_INTERVAL;
            if last_sample < Instant::now() {
                // Fell behind (slow host): resync instead of burst-sampling.
                last_sample = Instant::now();
            }
            sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
            let rss = sys
                .process(pid)
                .map(|p| p.memory()) // bytes
                .unwrap_or(0);
            samples.push(rss);
            match prev_rss {
                Some(prev) if rss > prev => {
                    growing_streak += 1;
                    if growing_streak >= MONOTONIC_FAIL_SAMPLES {
                        fail = Some(format!(
                            "RSS grew monotonically for {growing_streak} straight samples"
                        ));
                    }
                }
                _ => growing_streak = 0,
            }
            prev_rss = Some(rss);
            let mb = rss / (1024 * 1024);
            if mb > args.max_rss_mb {
                fail = Some(format!(
                    "RSS {mb} MB exceeded --max-rss-mb {}",
                    args.max_rss_mb
                ));
            }
            // Drain watcher events so the channel does not grow unboundedly.
            while rx.try_recv().is_ok() {}
            if let Some(msg) = &fail {
                println!("sample {: >5}: {mb} MB — {msg}", samples.len());
                break;
            }
            println!("sample {: >5}: {mb} MB", samples.len());
        }
    }

    // ---- summary + cleanup ---------------------------------------------
    // Drop the watcher's directory handles BEFORE removing the tempdir (on
    // Windows an open watch handle can block the removal).
    drop(watcher);
    let _ = std::fs::remove_dir_all(&dir);

    if samples.is_empty() {
        return Err("no RSS samples taken (runtime too short?)".into());
    }
    let mut sorted = samples.clone();
    sorted.sort_unstable();
    let max = sorted[samples.len() - 1];
    let median = sorted[samples.len() / 2];
    let max_mb = max / (1024 * 1024);
    let median_mb = median / (1024 * 1024);
    println!(
        "soak summary: {} samples, max RSS {max_mb} MB, median {median_mb} MB, {op_no} ops",
        samples.len()
    );
    match fail {
        Some(msg) => Err(msg),
        None => {
            if max_mb > args.max_rss_mb {
                Err(format!(
                    "final max RSS {max_mb} MB exceeded --max-rss-mb {}",
                    args.max_rss_mb
                ))
            } else {
                Ok(())
            }
        }
    }
}
