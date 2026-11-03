//! AI assist services (contracts.md "AI"): the functions the iced UI calls
//! for the six AI features. Each one gates on the config's AI section
//! FIRST (never any network when disabled), gathers its own git context
//! through the existing service/engine surfaces, builds the prompt via the
//! pure [`crate::ai::prompts`] builders, and routes the generation through
//! the provider layer with the single transport-retry owner.
//!
//! Secrets never pass through here as strings in errors: the OpenRouter
//! key and the opencode server password are read from the OS keyring /
//! serve registry per call and handed to the provider config in memory.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ai::{self, AiConfig, AiFeature, GenRequest, PromptContext};
use crate::engine::types::{CommitInfo, DiffSide, FileDiff, LogFilter, RepoId};
use crate::repo::RepoHandle;
use crate::service::{
    enqueue_read, enqueue_remote_read, finish_op, get_handle, repo_diff, AppServices,
};

/// Keyring key of the OpenRouter API key (old `SECRET_KEYS.openrouterApiKey`).
const KEY_OPENROUTER_API: &str = "openrouter.api-key";
/// Keyring key of the opencode server basic-auth password
/// (old `SECRET_KEYS.opencodeServerPassword`).
const KEY_OPENCODE_PASSWORD: &str = "opencode.server.password";

// ---------------------------------------------------------------------------
// config.json `ai` section (mirror of ui::config::AiSettings)
//
// ui/config.rs owns the persisted shape but sits inside the private `ui`
// module tree, so this service parses the same file section itself. The
// field names below ARE the persistence contract — keep them in sync with
// ui::config::AiSettings (the orchestrator can collapse this mirror once
// services may import ui::config directly). serde defaults everywhere, so
// pre-AI config files keep loading.
// ---------------------------------------------------------------------------

/// Which AI backend serves generation requests (mirror of
/// ui::config::AiProviderChoice; converts to [`ai::AiProvider`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum AiProviderChoice {
    #[default]
    Opencode,
    OpenRouter,
}

impl From<AiProviderChoice> for ai::AiProvider {
    fn from(choice: AiProviderChoice) -> Self {
        match choice {
            AiProviderChoice::Opencode => ai::AiProvider::Opencode,
            AiProviderChoice::OpenRouter => ai::AiProvider::OpenRouter,
        }
    }
}

/// The `ai` section as it is read here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
struct AiConfigSection {
    /// Master switch: AI features stay off until enabled.
    enabled: bool,
    /// Preferred backend for AI requests.
    provider: AiProviderChoice,
    /// Attach mode: manual opencode server base URL. Unset = managed.
    opencode_url: Option<String>,
    /// opencode model id (`provider/model`); unset uses the server default.
    opencode_model: Option<String>,
    /// OpenRouter model slug; backend default when unset.
    openrouter_model: Option<String>,
    /// Per-repository opt-in keyed by canonical repo path.
    repo_opt_in: BTreeMap<String, bool>,
}

impl AiConfigSection {
    /// Loads the `ai` section of `<config>/config.json` (the file is the
    /// whole app config: `{"theme": …, "ai": {…}}`); missing file or
    /// section (or a corrupt file — matching ui::config's loader) yields
    /// the all-off defaults.
    fn load(config_dir: &Path) -> Self {
        #[derive(Deserialize, Default)]
        struct ConfigFile {
            #[serde(default)]
            ai: AiConfigSection,
        }
        let Ok(raw) = std::fs::read_to_string(config_dir.join("config.json")) else {
            return Self::default();
        };
        serde_json::from_str::<ConfigFile>(&raw)
            .map(|file| file.ai)
            .unwrap_or_default()
    }

    /// Whether AI may run for the repo at `path`: master switch AND
    /// per-repo opt-in.
    fn enabled_for(&self, repo_path: &str) -> bool {
        self.enabled
            && self
                .repo_opt_in
                .get(&canonical_repo_key(repo_path))
                .copied()
                .unwrap_or(false)
    }
}

/// Canonical repo-path comparison key for the opt-in map — mirror of
/// ui::config::canonical_key (same folding: `\\?\` prefixes, slash styles
/// and trailing separators; Windows-shaped paths lowercase, POSIX keep
/// case).
fn canonical_repo_key(path: &str) -> String {
    let windowsish = path.get(..2).is_some_and(|prefix| {
        prefix.as_bytes()[1] == b':' && prefix.as_bytes()[0].is_ascii_alphabetic()
    }) || path.starts_with("\\\\");
    let stripped = path.strip_prefix("\\\\?\\").unwrap_or(path);
    let stripped = stripped.trim_end_matches(['\\', '/']).replace('\\', "/");
    if windowsish {
        stripped.to_lowercase()
    } else {
        stripped
    }
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// AI subsystem status for the settings panel / health chip.
#[derive(Debug, Clone, Serialize)]
pub struct AiStatus {
    pub enabled: bool,
    pub provider: ai::AiProvider,
    /// Model override for the configured provider.
    pub model: Option<String>,
    pub opencode: AiOpencodeStatus,
    pub openrouter: AiOpenrouterStatus,
}

/// opencode half of the status: what is installed, what the app manages,
/// and whether the URL this status would use identifies as opencode.
#[derive(Debug, Clone, Serialize)]
pub struct AiOpencodeStatus {
    /// CLI detection (PATH scan + `--version`); `None` when not probed
    /// (AI disabled or another provider selected — detection spawns a
    /// subprocess, so it only runs when it can matter).
    pub detection: Option<crate::opencode::OpencodeDetection>,
    /// Managed serve state (starts nothing — see `opencode_serve_status`).
    pub managed: crate::opencode::OpencodeServeState,
    /// Strict health of the effective URL (manual attach URL, else the
    /// running managed server), when one is known.
    pub health: Option<ai::Health>,
    /// The URL probed (or that would be probed).
    pub url: Option<String>,
}

/// OpenRouter half of the status.
#[derive(Debug, Clone, Serialize)]
pub struct AiOpenrouterStatus {
    /// True when an API key exists in the OS keyring (never the key itself).
    pub has_key: bool,
}

/// Detection + health + configured provider/model (starts no server).
pub async fn ai_status(services: &AppServices) -> AiStatus {
    let settings = AiConfigSection::load(&services.config_dir);
    let managed = services.opencode.status();

    let mut opencode = AiOpencodeStatus {
        detection: None,
        managed,
        health: None,
        url: None,
    };
    let mut openrouter = AiOpenrouterStatus { has_key: false };

    if settings.enabled {
        match settings.provider {
            AiProviderChoice::Opencode => {
                opencode.detection = crate::rt::spawn_blocking(crate::opencode::detect_binary)
                    .await
                    .ok();
                let password = opencode_password(services, &opencode.managed);
                let url = settings
                    .opencode_url
                    .clone()
                    .filter(|url| !url.trim().is_empty())
                    .or_else(|| opencode.managed.url.clone());
                opencode.url = url.clone();
                if let Some(url) = url {
                    opencode.health = Some(ai::probe(&url, password.as_deref()).await);
                }
            }
            AiProviderChoice::OpenRouter => {
                openrouter.has_key = keyring_get(services, KEY_OPENROUTER_API).is_some();
            }
        }
    }

    let model = match settings.provider {
        AiProviderChoice::Opencode => settings.opencode_model.clone(),
        AiProviderChoice::OpenRouter => settings.openrouter_model.clone(),
    };
    AiStatus {
        enabled: settings.enabled,
        provider: settings.provider.into(),
        model,
        opencode,
        openrouter,
    }
}

// ---------------------------------------------------------------------------
// The six features
// ---------------------------------------------------------------------------

/// Conventional-commits commit message from the staged diff.
pub async fn ai_commit_message(services: &AppServices, repo_id: RepoId) -> Result<String, String> {
    let handle = gate(services, &repo_id)?;
    let files = staged_files(services, &repo_id).await?;
    if files.is_empty() {
        return Err("nothing is staged to write a commit message for".into());
    }
    let context = PromptContext {
        staged_diff: Some(ai::budgeted_diff_text(&files, &ai::DiffBudget::default())),
        ..PromptContext::default()
    };
    run_feature(
        services,
        &repo_id,
        &handle,
        AiFeature::CommitMessage,
        context,
    )
    .await
}

/// Pull request title + description from the branch's recent commits and
/// the staged-diff file stats.
pub async fn ai_pr_title_body(services: &AppServices, repo_id: RepoId) -> Result<String, String> {
    let handle = gate(services, &repo_id)?;
    let commits = recent_commits(services, &repo_id, 50).await?;
    let files = staged_files(services, &repo_id).await?;
    let context = PromptContext {
        commits: commits
            .into_iter()
            .map(|commit| ai::CommitRef {
                sha: commit.sha,
                summary: commit.summary,
            })
            .collect(),
        diff_summary: Some(ai::diff_summary_text(&files)),
        ..PromptContext::default()
    };
    run_feature(services, &repo_id, &handle, AiFeature::PrTitleBody, context).await
}

/// Explanation of one diff hunk (`path` + the hunk text the diff view
/// already has rendered).
pub async fn ai_explain_hunk(
    services: &AppServices,
    repo_id: RepoId,
    path: String,
    hunk: String,
) -> Result<String, String> {
    let handle = gate(services, &repo_id)?;
    let context = PromptContext {
        path: Some(path),
        hunk: Some(hunk),
        ..PromptContext::default()
    };
    run_feature(services, &repo_id, &handle, AiFeature::ExplainHunk, context).await
}

/// Review of the whole staged diff (risks, edge cases, security, style).
pub async fn ai_review_staged(services: &AppServices, repo_id: RepoId) -> Result<String, String> {
    let handle = gate(services, &repo_id)?;
    let files = staged_files(services, &repo_id).await?;
    if files.is_empty() {
        return Err("nothing is staged to review".into());
    }
    let context = PromptContext {
        staged_diff: Some(ai::budgeted_diff_text(&files, &ai::DiffBudget::default())),
        ..PromptContext::default()
    };
    run_feature(
        services,
        &repo_id,
        &handle,
        AiFeature::ReviewStaged,
        context,
    )
    .await
}

/// Short stash message for the parked work-in-progress (the staged diff
/// capped to a small summary, like the old runner).
pub async fn ai_stash_message(services: &AppServices, repo_id: RepoId) -> Result<String, String> {
    let handle = gate(services, &repo_id)?;
    let files = staged_files(services, &repo_id).await?;
    let summary = ai::cap_diff(&ai::staged_diff_text(&files), 2_000);
    let context = PromptContext {
        staged_diff: Some(summary),
        ..PromptContext::default()
    };
    run_feature(
        services,
        &repo_id,
        &handle,
        AiFeature::StashMessage,
        context,
    )
    .await
}

/// Kebab-case branch name suggestion for a free-form description, flavored
/// with the recent commit subjects (parsed to the bare name).
pub async fn ai_branch_name(
    services: &AppServices,
    repo_id: RepoId,
    description: String,
) -> Result<String, String> {
    let handle = gate(services, &repo_id)?;
    let recent_subjects = match recent_commits(services, &repo_id, 5).await {
        Ok(commits) => commits.into_iter().map(|commit| commit.summary).collect(),
        Err(_) => Vec::new(),
    };
    let context = PromptContext {
        description: Some(description),
        recent_subjects,
        ..PromptContext::default()
    };
    let text = run_feature(services, &repo_id, &handle, AiFeature::BranchName, context).await?;
    ai::parse_branch_name(&text).map_err(|err| err.to_string())
}

// ---------------------------------------------------------------------------
// Plumbing: gating, provider resolution, generation
// ---------------------------------------------------------------------------

/// Reads a secret from the OS keyring (`None` on missing entry or backend
/// failure — never an error carrying secret material).
fn keyring_get(services: &AppServices, key: &str) -> Option<String> {
    crate::keyring_store::secrets_get(services, key.to_string())
        .ok()
        .flatten()
}

/// Password for requests to the managed/attached opencode server: a
/// managed 2.x server only accepts its generated password (serve registry,
/// in-memory); a managed 1.x server carries none, so the keyring value
/// applies. Same precedence as the old adapter.
fn opencode_password(
    services: &AppServices,
    managed: &crate::opencode::OpencodeServeState,
) -> Option<String> {
    managed
        .password
        .clone()
        .or_else(|| keyring_get(services, KEY_OPENCODE_PASSWORD))
}

/// Per-repo opt-in gate, checked BEFORE any data is gathered or sent
/// (contract inherited from the old feature runners).
fn ensure_ai_allowed(settings: &AiConfigSection, repo_path: &str) -> Result<(), String> {
    if !settings.enabled {
        return Err("AI features are disabled in settings — enable them in AI settings".into());
    }
    if !settings.enabled_for(repo_path) {
        return Err(
            "AI features are not enabled for this repository — allow them in the AI prompt or AI settings"
                .into(),
        );
    }
    Ok(())
}

/// The pre-flight every feature runs first: repo lookup + config gate.
fn gate(services: &AppServices, repo_id: &RepoId) -> Result<Arc<RepoHandle>, String> {
    let handle = get_handle(&services.repos, repo_id)?;
    let settings = AiConfigSection::load(&services.config_dir);
    let repo_path = handle.root.to_string_lossy().into_owned();
    ensure_ai_allowed(&settings, &repo_path)?;
    Ok(handle)
}

/// Builds the provider config for one generation: opencode resolves its
/// server (attach URL from settings, or the managed registry — lazily
/// started for this repo like the old adapter did), OpenRouter reads its
/// keyring key.
async fn provider_config(
    services: &AppServices,
    settings: &AiConfigSection,
    repo_root: &Path,
) -> Result<AiConfig, String> {
    match settings.provider {
        AiProviderChoice::Opencode => {
            let (url, password) = match settings
                .opencode_url
                .as_deref()
                .map(str::trim)
                .filter(|url| !url.is_empty())
            {
                // Attach mode: the user runs the server; the keyring
                // password (when stored) authenticates it.
                Some(url) => (
                    url.to_string(),
                    keyring_get(services, KEY_OPENCODE_PASSWORD),
                ),
                None => {
                    // Managed mode: reuse the running server, else start it
                    // with the repo as its workspace.
                    let mut state = services.opencode.status();
                    if !state.running || state.url.is_none() {
                        state = services.opencode.start(Some(repo_root));
                    }
                    if !state.running {
                        return Err(state.error.clone().unwrap_or_else(|| {
                            "failed to reach the managed opencode server".into()
                        }));
                    }
                    let url = state
                        .url
                        .clone()
                        .ok_or_else(|| "managed opencode server has no URL".to_string())?;
                    (url, opencode_password(services, &state))
                }
            };
            Ok(AiConfig {
                provider: ai::AiProvider::Opencode,
                opencode_url: Some(url),
                opencode_password: password,
                model: settings.opencode_model.clone(),
                // OpenCode mis-parses backslash workspaces (1.x resolves the
                // x-opencode-directory value with forward slashes only).
                directory: Some(ai::normalize_directory(&repo_root.to_string_lossy())),
                ..AiConfig::default()
            })
        }
        AiProviderChoice::OpenRouter => {
            let key = keyring_get(services, KEY_OPENROUTER_API).ok_or_else(|| {
                "no OpenRouter API key configured — add one in AI settings".to_string()
            })?;
            Ok(AiConfig {
                provider: ai::AiProvider::OpenRouter,
                openrouter_api_key: Some(key),
                model: settings.openrouter_model.clone(),
                ..AiConfig::default()
            })
        }
    }
}

/// Builds the prompts, resolves the provider, and generates with the
/// single transport-retry owner. Callers gate first.
async fn run_feature(
    services: &AppServices,
    repo_id: &RepoId,
    handle: &Arc<RepoHandle>,
    feature: AiFeature,
    context: PromptContext,
) -> Result<String, String> {
    let (system, prompt) = ai::prompts(feature, &context).map_err(|err| err.to_string())?;
    let settings = AiConfigSection::load(&services.config_dir);
    let cfg = provider_config(services, &settings, &handle.root).await?;
    let request = GenRequest {
        feature,
        system,
        prompt,
        max_tokens: None,
        session_key: repo_id.0.clone(),
    };
    // Single retry owner: only transport-class failures back off here. A
    // retried generation repeats the prompt server-side (an extra session
    // message / orphaned temp session at worst) and never touches
    // repository state.
    ai::with_transport_retries(|| {
        let request = request.clone();
        let cfg = cfg.clone();
        async move { ai::generate(request, &cfg).await }
    })
    .await
    .map_err(|err| err.to_string())
}

/// The staged diff (HEAD → index) through the existing diff service.
async fn staged_files(services: &AppServices, repo_id: &RepoId) -> Result<Vec<FileDiff>, String> {
    repo_diff(
        services,
        repo_id.clone(),
        DiffSide::Head,
        DiffSide::Index,
        None,
    )
    .await
}

/// The first `limit` commits of the walk (PR context, branch-name flavor)
/// through a silent queued read — the same engine call the log stream uses.
async fn recent_commits(
    services: &AppServices,
    repo_id: &RepoId,
    limit: usize,
) -> Result<Vec<CommitInfo>, String> {
    let handle = get_handle(&services.repos, repo_id)?;
    let filter = LogFilter::default();
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "log",
            move |git, _ws| {
                git.log(&filter, limit, None)
                    .map(|(commits, _cursor)| commits)
            },
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "log", move |_ctx, repo| {
        engine
            .log(repo, &filter, limit, None)
            .map(|(commits, _cursor)| commits)
    }))
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mygitui-service-ai-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// Writes a config.json whose `ai` section uses the same field names
    /// ui::config persists — the shared persistence contract both sides
    /// read/write.
    fn write_config(config_dir: &Path, ai: serde_json::Value) {
        std::fs::create_dir_all(config_dir).unwrap();
        std::fs::write(
            config_dir.join("config.json"),
            serde_json::json!({ "ai": ai }).to_string(),
        )
        .unwrap();
    }

    /// An enabled section with the repo opted in by canonical key.
    fn enabled_section(repo_root: &str, extra: serde_json::Value) -> serde_json::Value {
        let mut section = serde_json::json!({
            "enabled": true,
            "provider": "opencode",
            "repo_opt_in": { canonical_repo_key(repo_root): true }
        });
        if let Some(map) = extra.as_object() {
            for (key, value) in map {
                section[key] = value.clone();
            }
        }
        section
    }

    /// A real local repo with an initial commit; `stage_change` adds a
    /// staged modification when requested. Config lives in its own dir
    /// (written per test).
    fn setup_repo(tag: &str, stage_change: bool) -> (AppServices, RepoId, PathBuf) {
        let repo_dir = temp_dir(tag);
        let config_dir = temp_dir(&format!("{tag}-config"));
        let repo = git2::Repository::init(&repo_dir).expect("init");
        std::fs::write(repo_dir.join("hello.txt"), "one\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("hello.txt")).unwrap();
        // Persist the index too: the diff service reads the on-disk index,
        // and an unwritten one would show the file as deleted.
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let signature = git2::Signature::now("mygitui", "ai@test").unwrap();
        repo.commit(Some("HEAD"), &signature, &signature, "init", &tree, &[])
            .unwrap();
        if stage_change {
            std::fs::write(repo_dir.join("hello.txt"), "one\ntwo\n").unwrap();
            let mut index = repo.index().unwrap();
            index.add_path(Path::new("hello.txt")).unwrap();
            index.write().unwrap();
        }
        let services = AppServices::with_dirs(temp_dir(&format!("{tag}-data")), &config_dir);
        let info = services.repos.open_unwatched(&repo_dir).expect("open repo");
        (services, info.repo_id, repo_dir)
    }

    fn config_dir_of(services: &AppServices) -> PathBuf {
        services.config_dir.clone()
    }

    fn repo_root_of(services: &AppServices, repo_id: &RepoId) -> String {
        get_handle(&services.repos, repo_id)
            .unwrap()
            .root
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn config_section_loads_with_defaults_and_canonical_keys() {
        let dir = temp_dir("mirror");
        // Missing file: all-off defaults.
        let section = AiConfigSection::load(&dir);
        assert_eq!(section, AiConfigSection::default());
        assert!(!section.enabled_for("/any/repo"));
        // Corrupt file: defaults too (ui::config's loader contract).
        std::fs::write(dir.join("config.json"), "{not json").unwrap();
        assert_eq!(AiConfigSection::load(&dir), AiConfigSection::default());
        // Section absent from a pre-AI file: still defaults.
        std::fs::write(dir.join("config.json"), r#"{"theme": "Dark"}"#).unwrap();
        assert_eq!(AiConfigSection::load(&dir), AiConfigSection::default());
    }

    #[test]
    fn canonical_repo_key_folds_paths_like_the_config_writer() {
        assert_eq!(canonical_repo_key("/home/me/Repo/"), "/home/me/Repo");
        assert_eq!(canonical_repo_key("C:\\Repos\\Alpha\\"), "c:/repos/alpha");
        assert_eq!(
            canonical_repo_key("\\\\?\\C:\\Repos\\Alpha"),
            "c:/repos/alpha"
        );
        assert_eq!(canonical_repo_key("c:/repos/alpha"), "c:/repos/alpha");
    }

    #[test]
    fn opt_in_map_resolves_canonical_variants() {
        let section = AiConfigSection {
            enabled: true,
            repo_opt_in: BTreeMap::from([("c:/repos/alpha".to_string(), true)]),
            ..AiConfigSection::default()
        };
        assert!(section.enabled_for(r"C:\Repos\Alpha"));
        assert!(section.enabled_for("c:/repos/alpha/"));
        assert!(!section.enabled_for("/repos/other"));
        // Master switch off wins even for opted-in repos.
        let dark = AiConfigSection {
            enabled: false,
            ..section
        };
        assert!(!dark.enabled_for("c:/repos/alpha"));
    }

    #[tokio::test]
    async fn disabled_ai_fails_before_any_gathering_or_network() {
        let (services, repo_id, _repo_dir) = setup_repo("disabled", true);
        write_config(
            &config_dir_of(&services),
            serde_json::json!({ "enabled": false }),
        );
        let err = ai_commit_message(&services, repo_id).await.unwrap_err();
        assert!(err.contains("disabled in settings"), "{err}");
    }

    #[tokio::test]
    async fn repo_opt_in_gate_blocks_before_gathering() {
        let (services, repo_id, _repo_dir) = setup_repo("notopted", true);
        write_config(
            &config_dir_of(&services),
            serde_json::json!({ "enabled": true }),
        );
        let err = ai_commit_message(&services, repo_id).await.unwrap_err();
        assert!(
            err.contains("not enabled for this repository"),
            "gate copy must name the fix: {err}"
        );
    }

    #[tokio::test]
    async fn opted_in_repo_with_nothing_staged_reports_it_without_network() {
        let (services, repo_id, _repo_dir) = setup_repo("nothing", false);
        let root = repo_root_of(&services, &repo_id);
        write_config(
            &config_dir_of(&services),
            enabled_section(&root, serde_json::json!({})),
        );

        let err = ai_commit_message(&services, repo_id).await.unwrap_err();
        assert_eq!(err, "nothing is staged to write a commit message for");
    }

    #[tokio::test]
    async fn commit_message_round_trips_through_a_mock_opencode_v1_server() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let (services, repo_id, _repo_dir) = setup_repo("roundtrip", true);
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/global/health"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"healthy": true, "version": "1.18.35"})),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/session"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "ses_1"})),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/session/ses_1/message"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "info": {},
                "parts": [{"type": "text", "text": "feat(core): stage a second line"}]
            })))
            .mount(&server)
            .await;

        let root = repo_root_of(&services, &repo_id);
        write_config(
            &config_dir_of(&services),
            enabled_section(&root, serde_json::json!({ "opencode_url": server.uri() })),
        );

        let message = ai_commit_message(&services, repo_id).await.unwrap();
        assert_eq!(message, "feat(core): stage a second line");

        // The prompt carried the staged diff (the "+two" change). The mock's
        // request log lands asynchronously after the response, so poll.
        let mut prompt_body = None;
        for _ in 0..200 {
            let requests = server.received_requests().await.unwrap_or_default();
            if let Some(request) = requests.iter().find(|r| r.url.path().contains("/message")) {
                prompt_body =
                    Some(serde_json::from_slice::<serde_json::Value>(&request.body).unwrap());
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let prompt_body = prompt_body.expect("prompt request logged");
        let prompt = prompt_body["parts"][0]["text"].as_str().unwrap();
        assert!(
            prompt.contains("hello.txt"),
            "staged file in the prompt: {prompt}"
        );
        assert!(
            prompt.contains("+two") || prompt.contains("two"),
            "staged line in the prompt"
        );
    }

    #[tokio::test]
    async fn ai_status_reports_attach_health_without_starting_anything() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let (services, _repo_id, _repo_dir) = setup_repo("status", false);
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/info"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"version": "2.0.18", "pid": 7})),
            )
            .mount(&server)
            .await;

        write_config(
            &config_dir_of(&services),
            serde_json::json!({
                "enabled": true,
                "provider": "opencode",
                "opencode_url": server.uri(),
                "opencode_model": "anthropic/claude"
            }),
        );

        let status = ai_status(&services).await;
        assert!(status.enabled);
        assert_eq!(status.provider, ai::AiProvider::Opencode);
        assert_eq!(status.model.as_deref(), Some("anthropic/claude"));
        let health = status.opencode.health.expect("attach URL probed");
        assert_eq!(health.kind, ai::HealthKind::V2);
        assert_eq!(health.version.as_deref(), Some("2.0.18"));
        assert!(!status.opencode.managed.running, "status starts no server");

        // Disabled: no probe, no detection, no keyring reads.
        write_config(
            &config_dir_of(&services),
            serde_json::json!({ "enabled": false }),
        );
        let status = ai_status(&services).await;
        assert!(!status.enabled);
        assert!(status.opencode.health.is_none());
        assert!(status.opencode.detection.is_none());
    }

    #[tokio::test]
    async fn branch_name_parses_the_answer_to_a_bare_name() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let (services, repo_id, _repo_dir) = setup_repo("branchname", false);
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/global/health"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"healthy": true, "version": "1.18.35"})),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/session"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "ses_1"})),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/session/ses_1/message"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "info": {},
                "parts": [{"type": "text", "text": "Here you go:\n- `fix-login-crash`"}]
            })))
            .mount(&server)
            .await;

        let root = repo_root_of(&services, &repo_id);
        write_config(
            &config_dir_of(&services),
            enabled_section(&root, serde_json::json!({ "opencode_url": server.uri() })),
        );

        let name = ai_branch_name(&services, repo_id, "fix the login crash".into())
            .await
            .unwrap();
        assert_eq!(name, "fix-login-crash");
    }
}
