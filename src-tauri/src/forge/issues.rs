//! GitHub Issues domain: stable REST models, tolerant parsing, reads,
//! mutations, metadata, identity, and bounded repository-wide activity scans.
//!
//! Pull requests are removed whenever the REST `pull_request` marker is
//! present. Activity scans make one issues read and one comments read, plus
//! bounded issue-detail fallbacks for comment references missing from that
//! issue page. Pagination is exposed as opaque Link-derived cursors.

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use super::api::{continuation_cursor, link_page, path_segment, request, ApiResponse};
use super::{ForgeError, ForgeRepo, ResolvedRemote, ACTION_TIMEOUT, LIST_TIMEOUT};

const DEFAULT_PER_PAGE: u32 = 30;
const MAX_PER_PAGE: u32 = 100;
const METADATA_PER_PAGE: u32 = 100;
const MAX_METADATA_PAGES: u32 = 100;
const ACTIVITY_PER_PAGE: u32 = 100;

/// A GitHub user/assignee, identified by the stable GraphQL node id.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct IssueUser {
    pub node_id: String,
    pub login: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct IssueLabel {
    pub node_id: String,
    pub name: String,
    pub color: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct IssueMilestone {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub due_on: Option<String>,
    pub html_url: String,
}

/// List-safe issue fields. `body` is deliberately fetched only by detail.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct IssueSummary {
    pub node_id: String,
    pub number: u64,
    pub title: String,
    pub state: String,
    pub author: IssueUser,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
    pub url: String,
    pub labels: Vec<IssueLabel>,
    pub assignees: Vec<IssueUser>,
    pub milestone: Option<IssueMilestone>,
    pub comment_count: u64,
    pub locked: bool,
    pub state_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct IssueType {
    pub name: String,
}

/// Detail is flattened on the wire: summary fields plus body/type, never a
/// nested `summary` property.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct IssueDetail {
    #[serde(flatten)]
    pub summary: IssueSummary,
    pub body: String,
    pub issue_type: Option<IssueType>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct IssueComment {
    pub node_id: String,
    pub issue_number: u64,
    pub author: IssueUser,
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct IssueActivityComment {
    #[serde(flatten)]
    pub comment: IssueComment,
    pub issue_title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct IssueListQuery {
    pub state: String,
    pub search: String,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub milestone: String,
    pub sort: String,
    pub direction: String,
    pub per_page: u32,
    pub page: u32,
}

impl Default for IssueListQuery {
    fn default() -> Self {
        Self {
            state: "open".to_string(),
            search: String::new(),
            labels: Vec::new(),
            assignees: Vec::new(),
            milestone: String::new(),
            sort: "updated".to_string(),
            direction: "desc".to_string(),
            per_page: DEFAULT_PER_PAGE,
            page: 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct IssueListPage {
    pub items: Vec<IssueSummary>,
    pub page: u32,
    pub per_page: u32,
    pub has_next: bool,
    pub has_prev: bool,
    pub total_count: Option<u64>,
    pub incomplete_results: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct IssueCommentPage {
    pub items: Vec<IssueComment>,
    pub page: u32,
    pub per_page: u32,
    pub has_next: bool,
    pub has_prev: bool,
    pub total_count: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct IssueMetadata {
    pub labels: Vec<IssueLabel>,
    pub assignees: Vec<IssueUser>,
    pub milestones: Vec<IssueMilestone>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct IssueIdentity {
    #[ts(type = "\"local\" | \"device\"")]
    pub executor: String,
    pub device_id: Option<String>,
    pub github_host: String,
    pub owner: String,
    pub repo: String,
    pub remote: String,
    pub user: IssueUser,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct IssueActivityBatch {
    pub issues: Vec<IssueSummary>,
    pub comments: Vec<IssueActivityComment>,
    pub next_issue_cursor: Option<String>,
    pub next_comment_cursor: Option<String>,
    pub complete: bool,
}

// ---------------------------------------------------------------------------
// Tolerant REST parsers
// ---------------------------------------------------------------------------

fn string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or_default()
}

fn stable_node_id(value: &Value, prefix: &str) -> String {
    if let Some(node_id) = value
        .get("node_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    {
        return node_id.to_owned();
    }
    let id = value.get("id").and_then(Value::as_u64).unwrap_or_default();
    if id == 0 {
        String::new()
    } else {
        format!("{prefix}:{id}")
    }
}

/// The REST issues endpoints include PRs; this marker is the authoritative
/// discriminator and must not be inferred from titles or labels.
pub fn is_pull_request(value: &Value) -> bool {
    value.get("pull_request").is_some()
}

pub fn parse_issue_user(value: &Value) -> IssueUser {
    IssueUser {
        node_id: stable_node_id(value, "rest-user"),
        login: string(value, "login"),
    }
}

pub fn parse_issue_label(value: &Value) -> Option<IssueLabel> {
    let name = string(value, "name");
    (!name.is_empty()).then_some(IssueLabel {
        node_id: stable_node_id(value, "rest-label"),
        name,
        color: string(value, "color"),
        description: optional_string(value, "description"),
    })
}

pub fn parse_issue_milestone(value: &Value) -> Option<IssueMilestone> {
    let milestone_number = value
        .get("number")
        .and_then(Value::as_u64)
        .filter(|n| *n > 0)?;
    Some(IssueMilestone {
        number: milestone_number,
        title: string(value, "title"),
        state: string(value, "state"),
        due_on: optional_string(value, "due_on"),
        html_url: string(value, "html_url"),
    })
}

fn parse_users(value: &Value) -> Vec<IssueUser> {
    value
        .get("assignees")
        .and_then(Value::as_array)
        .map(|users| users.iter().map(parse_issue_user).collect())
        .unwrap_or_default()
}

fn parse_labels(value: &Value) -> Vec<IssueLabel> {
    value
        .get("labels")
        .and_then(Value::as_array)
        .map(|labels| labels.iter().filter_map(parse_issue_label).collect())
        .unwrap_or_default()
}

/// Parse one issue/PR row. Rows without a positive number and every PR row
/// are skipped; missing optional fields degrade to empty/default values.
pub fn parse_issue_summary(value: &Value) -> Option<IssueSummary> {
    if is_pull_request(value) {
        return None;
    }
    let issue_number = value
        .get("number")
        .and_then(Value::as_u64)
        .filter(|n| *n > 0)?;
    let author = value
        .get("user")
        .map(parse_issue_user)
        .unwrap_or_else(|| IssueUser {
            node_id: String::new(),
            login: String::new(),
        });
    Some(IssueSummary {
        node_id: stable_node_id(value, "rest-issue"),
        number: issue_number,
        title: string(value, "title"),
        state: string(value, "state"),
        author,
        created_at: string(value, "created_at"),
        updated_at: string(value, "updated_at"),
        closed_at: optional_string(value, "closed_at"),
        url: string(value, "html_url"),
        labels: parse_labels(value),
        assignees: parse_users(value),
        milestone: value
            .get("milestone")
            .filter(|v| !v.is_null())
            .and_then(parse_issue_milestone),
        comment_count: number(value, "comments"),
        locked: value
            .get("locked")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        state_reason: optional_string(value, "state_reason"),
    })
}

pub fn parse_issue_detail(value: &Value) -> Option<IssueDetail> {
    let summary = parse_issue_summary(value)?;
    let issue_type = value
        .get("issue_type")
        .filter(|value| !value.is_null())
        .or_else(|| value.get("type").filter(|value| !value.is_null()))
        .map(|value| IssueType {
            name: string(value, "name"),
        });
    Some(IssueDetail {
        summary,
        body: string(value, "body"),
        issue_type,
    })
}

pub fn parse_issue_array_json(payload: &str) -> Vec<IssueSummary> {
    serde_json::from_str::<Vec<Value>>(payload)
        .unwrap_or_default()
        .iter()
        .filter_map(parse_issue_summary)
        .collect()
}

/// Extract `/issues/<number>` from either an API issue URL or an HTML issue /
/// issue-comment URL.
pub fn issue_number_from_url(url: &str) -> Option<u64> {
    let path = url.split(['?', '#']).next().unwrap_or_default();
    let mut segments = path.split('/');
    segments.find(|segment| *segment == "issues")?;
    segments.next()?.parse().ok().filter(|number| *number > 0)
}

pub fn parse_issue_comment(value: &Value, fallback_issue_number: u64) -> Option<IssueComment> {
    let issue_number = issue_number_from_url(&string(value, "issue_url"))
        .or_else(|| issue_number_from_url(&string(value, "html_url")))
        .or_else(|| (fallback_issue_number > 0).then_some(fallback_issue_number))?;
    Some(IssueComment {
        node_id: stable_node_id(value, "rest-comment"),
        issue_number,
        author: value
            .get("user")
            .map(parse_issue_user)
            .unwrap_or_else(|| IssueUser {
                node_id: String::new(),
                login: String::new(),
            }),
        body: string(value, "body"),
        created_at: string(value, "created_at"),
        updated_at: string(value, "updated_at"),
        url: string(value, "html_url"),
    })
}

pub fn parse_comment_array_json(payload: &str, issue_number: u64) -> Vec<IssueComment> {
    serde_json::from_str::<Vec<Value>>(payload)
        .unwrap_or_default()
        .iter()
        .filter_map(|value| parse_issue_comment(value, issue_number))
        .collect()
}

/// Build actual-issue titles/numbers from a mixed issues response. PR numbers
/// are intentionally absent, so their repository-wide comments are excluded.
pub fn activity_issue_index(values: &[Value]) -> (Vec<IssueSummary>, HashMap<u64, String>) {
    let mut issues = Vec::new();
    let mut titles = HashMap::new();
    merge_activity_issues(values, &mut issues, &mut titles);
    (issues, titles)
}

fn merge_activity_issues(
    values: &[Value],
    issues: &mut Vec<IssueSummary>,
    titles: &mut HashMap<u64, String>,
) {
    for value in values {
        if let Some(issue) = parse_issue_summary(value) {
            if titles.insert(issue.number, issue.title.clone()).is_none() {
                issues.push(issue);
            }
        }
    }
}

pub fn activity_comments(
    values: &[Value],
    titles: &HashMap<u64, String>,
) -> Vec<IssueActivityComment> {
    values
        .iter()
        .filter_map(|value| parse_issue_comment(value, 0))
        .filter(|comment| titles.contains_key(&comment.issue_number))
        .map(|comment| IssueActivityComment {
            issue_title: titles
                .get(&comment.issue_number)
                .cloned()
                .unwrap_or_default(),
            comment,
        })
        .collect()
}

/// Resolve only issue numbers not present in the bounded issue page. The
/// comments page caps this at 100 unique lookups; failures abort the batch so
/// its cursors cannot advance past an unresolved comment.
fn resolve_activity_issues(
    repo: &ForgeRepo,
    remote: &ResolvedRemote,
    issues: &mut Vec<IssueSummary>,
    titles: &mut HashMap<u64, String>,
    comment_values: &[Value],
) -> Result<(), ForgeError> {
    let missing: BTreeSet<u64> = comment_values
        .iter()
        .filter_map(|value| parse_issue_comment(value, 0))
        .map(|comment| comment.issue_number)
        .filter(|number| !titles.contains_key(number))
        .collect();
    for number in missing {
        let response = request(
            repo,
            remote,
            "GET",
            &issue_endpoint(remote, number),
            &[],
            None,
            LIST_TIMEOUT,
        )?;
        let value = response_object(&response)?;
        if is_pull_request(&value) {
            continue;
        }
        let issue = parse_issue_summary(&value)
            .filter(|issue| issue.number == number)
            .ok_or_else(|| ForgeError::other(format!("GitHub did not return issue #{number}")))?;
        titles.insert(number, issue.title.clone());
        issues.push(issue);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Query and endpoint helpers
// ---------------------------------------------------------------------------

fn clean_string_list(values: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for value in values {
        let value = value.trim();
        if !value.is_empty() && !out.iter().any(|entry| entry == value) {
            out.push(value.to_owned());
        }
    }
    out
}

pub fn normalize_issue_query(mut query: IssueListQuery) -> IssueListQuery {
    if !matches!(query.state.as_str(), "open" | "closed" | "all") {
        query.state = "open".to_string();
    }
    if !matches!(query.sort.as_str(), "created" | "updated" | "comments") {
        query.sort = "updated".to_string();
    }
    if !matches!(query.direction.as_str(), "asc" | "desc") {
        query.direction = "desc".to_string();
    }
    query.search = query
        .search
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    query.labels = clean_string_list(&query.labels);
    query.assignees = clean_string_list(&query.assignees);
    query.milestone = query.milestone.trim().to_string();
    query.per_page = query.per_page.clamp(1, MAX_PER_PAGE);
    query.page = query.page.max(1);
    query
}

fn quote_search_value(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Search qualifiers are always quoted and escaped; free text remains the
/// caller's explicitly submitted search expression.
pub fn build_search_query(query: &IssueListQuery, owner: &str, repo: &str) -> String {
    let mut parts = vec![
        format!("repo:{}", quote_search_value(&format!("{owner}/{repo}"))),
        "is:issue".to_string(),
    ];
    if !query.search.is_empty() {
        parts.push(query.search.clone());
    }
    if query.state != "all" {
        parts.push(format!("state:{}", query.state));
    }
    for label in &query.labels {
        parts.push(format!("label:{}", quote_search_value(label)));
    }
    for assignee in &query.assignees {
        parts.push(format!("assignee:{}", quote_search_value(assignee)));
    }
    if !query.milestone.is_empty() {
        parts.push(format!(
            "milestone:{}",
            quote_search_value(&query.milestone)
        ));
    }
    parts.join(" ")
}

pub fn repository_path(remote: &ResolvedRemote) -> String {
    format!(
        "/repos/{}/{}",
        path_segment(&remote.owner),
        path_segment(&remote.repo)
    )
}

fn issue_endpoint(remote: &ResolvedRemote, number: u64) -> String {
    format!("{}/issues/{number}", repository_path(remote))
}

fn validate_issue_number(number: u64) -> Result<(), ForgeError> {
    if number == 0 {
        Err(ForgeError::other("issue number must be greater than zero"))
    } else {
        Ok(())
    }
}

fn response_array(response: &ApiResponse) -> Result<Vec<Value>, ForgeError> {
    match response.json()? {
        Value::Array(values) => Ok(values),
        _ => Err(ForgeError::other(
            "GitHub API returned an unexpected response shape",
        )),
    }
}

fn response_object(response: &ApiResponse) -> Result<Value, ForgeError> {
    match response.json()? {
        Value::Object(value) => Ok(Value::Object(value)),
        _ => Err(ForgeError::other(
            "GitHub API returned an unexpected response shape",
        )),
    }
}

fn parse_detail_response(response: &ApiResponse, number: u64) -> Result<IssueDetail, ForgeError> {
    let value = response_object(response)?;
    parse_issue_detail(&value)
        .filter(|detail| detail.summary.number == number)
        .ok_or_else(|| ForgeError::other(format!("GitHub did not return issue #{number}")))
}

// ---------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------

pub fn issue_list_sync(
    repo: &ForgeRepo,
    remote_override: Option<&str>,
    query: IssueListQuery,
) -> Result<IssueListPage, ForgeError> {
    let query = normalize_issue_query(query);
    let remote = super::resolve_remote(repo, remote_override)?;
    // REST accepts one assignee; repeated search qualifiers match any assignee.
    let searching = !query.search.is_empty() || query.assignees.len() > 1;
    let params = if searching {
        vec![
            ("q", build_search_query(&query, &remote.owner, &remote.repo)),
            ("sort", query.sort.clone()),
            ("order", query.direction.clone()),
            ("per_page", query.per_page.to_string()),
            ("page", query.page.to_string()),
        ]
    } else {
        let mut params = vec![
            ("state", query.state.clone()),
            ("sort", query.sort.clone()),
            ("direction", query.direction.clone()),
            ("per_page", query.per_page.to_string()),
            ("page", query.page.to_string()),
        ];
        if !query.labels.is_empty() {
            params.push(("labels", query.labels.join(",")));
        }
        if let Some(assignee) = query.assignees.first() {
            params.push(("assignee", assignee.clone()));
        }
        if !query.milestone.is_empty() {
            params.push(("milestone", query.milestone.clone()));
        }
        params
    };
    let endpoint = if searching {
        "/search/issues".to_string()
    } else {
        format!("{}/issues", repository_path(&remote))
    };
    let param_refs: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    let response = request(
        repo,
        &remote,
        "GET",
        &endpoint,
        &param_refs,
        None,
        LIST_TIMEOUT,
    )?;
    let links = response.links();
    let (items, total_count, incomplete_results) = if searching {
        let value = response_object(&response)?;
        let items = value
            .get("items")
            .and_then(Value::as_array)
            .map(|values| values.iter().filter_map(parse_issue_summary).collect())
            .unwrap_or_default();
        (
            items,
            value.get("total_count").and_then(Value::as_u64),
            value
                .get("incomplete_results")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        )
    } else {
        (parse_issue_array_json(&response.body), None, false)
    };
    Ok(IssueListPage {
        items,
        page: query.page,
        per_page: query.per_page,
        has_next: links.next.is_some(),
        has_prev: links.prev.is_some(),
        total_count,
        incomplete_results,
    })
}

pub fn issue_detail_sync(
    repo: &ForgeRepo,
    remote_override: Option<&str>,
    number: u64,
) -> Result<IssueDetail, ForgeError> {
    validate_issue_number(number)?;
    let remote = super::resolve_remote(repo, remote_override)?;
    let endpoint = issue_endpoint(&remote, number);
    let response = request(repo, &remote, "GET", &endpoint, &[], None, LIST_TIMEOUT)?;
    if is_pull_request(&response_object(&response)?) {
        return Err(ForgeError::other(format!(
            "#{number} is a pull request, not an issue"
        )));
    }
    parse_detail_response(&response, number)
}

fn fetch_comment_page(
    repo: &ForgeRepo,
    remote: &ResolvedRemote,
    number: u64,
    page: u32,
    per_page: u32,
) -> Result<(IssueCommentPage, ApiResponse), ForgeError> {
    let endpoint = format!("{}/issues/{number}/comments", repository_path(remote));
    let params = [
        ("per_page", per_page.to_string()),
        ("page", page.to_string()),
    ];
    let param_refs: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    let response = request(
        repo,
        remote,
        "GET",
        &endpoint,
        &param_refs,
        None,
        LIST_TIMEOUT,
    )?;
    let links = response.links();
    let items = parse_comment_array_json(&response.body, number);
    Ok((
        IssueCommentPage {
            items,
            page,
            per_page,
            has_next: links.next.is_some(),
            has_prev: links.prev.is_some(),
            total_count: None,
        },
        response,
    ))
}

/// `page = None` resolves the final page from `Link: rel=last`; later calls
/// request that concrete page or an earlier one.
pub fn issue_comments_sync(
    repo: &ForgeRepo,
    remote_override: Option<&str>,
    number: u64,
    page: Option<u32>,
    per_page: u32,
) -> Result<IssueCommentPage, ForgeError> {
    validate_issue_number(number)?;
    let per_page = per_page.clamp(1, MAX_PER_PAGE);
    let remote = super::resolve_remote(repo, remote_override)?;
    let (first, first_response) = fetch_comment_page(repo, &remote, number, 1, per_page)?;
    if page.is_none() {
        if let Some(last) = link_page(&first_response, "last").filter(|last| *last > 1) {
            return fetch_comment_page(repo, &remote, number, last as u32, per_page)
                .map(|(page, _)| page);
        }
        return Ok(first);
    }
    let page = page.unwrap_or(1).max(1);
    if page == 1 {
        return Ok(first);
    }
    fetch_comment_page(repo, &remote, number, page, per_page).map(|(page, _)| page)
}

fn fetch_metadata_page<T>(
    repo: &ForgeRepo,
    remote: &ResolvedRemote,
    endpoint: &str,
    page: u32,
    extra_params: &[(&str, &str)],
    parse: impl Fn(&Value) -> Option<T>,
) -> Result<(Vec<T>, bool), ForgeError> {
    let mut params = vec![
        ("per_page", METADATA_PER_PAGE.to_string()),
        ("page", page.to_string()),
    ];
    params.extend(
        extra_params
            .iter()
            .map(|(name, value)| (*name, (*value).to_string())),
    );
    let param_refs: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    let response = request(
        repo,
        remote,
        "GET",
        endpoint,
        &param_refs,
        None,
        LIST_TIMEOUT,
    )?;
    let has_next = response.links().next.is_some();
    let values = response_array(&response)?;
    Ok((values.iter().filter_map(&parse).collect(), has_next))
}

fn collect_metadata<T>(
    repo: &ForgeRepo,
    remote: &ResolvedRemote,
    endpoint: &str,
    extra_params: &[(&str, &str)],
    parse: impl Fn(&Value) -> Option<T>,
) -> Result<Vec<T>, ForgeError> {
    let mut out = Vec::new();
    for page in 1..=MAX_METADATA_PAGES {
        let (mut items, has_next) =
            fetch_metadata_page(repo, remote, endpoint, page, extra_params, &parse)?;
        out.append(&mut items);
        if !has_next {
            return Ok(out);
        }
    }
    Err(ForgeError::other(
        "GitHub metadata pagination exceeded the safe page limit",
    ))
}

pub fn issue_metadata_sync(
    repo: &ForgeRepo,
    remote_override: Option<&str>,
) -> Result<IssueMetadata, ForgeError> {
    let remote = super::resolve_remote(repo, remote_override)?;
    let root = repository_path(&remote);
    let labels = collect_metadata(
        repo,
        &remote,
        &format!("{root}/labels"),
        &[],
        parse_issue_label,
    )?;
    let assignees = collect_metadata(repo, &remote, &format!("{root}/assignees"), &[], |value| {
        Some(parse_issue_user(value))
    })?;
    let milestones = collect_metadata(
        repo,
        &remote,
        &format!("{root}/milestones"),
        &[("state", "all")],
        parse_issue_milestone,
    )?;
    Ok(IssueMetadata {
        labels,
        assignees,
        milestones,
    })
}

// ---------------------------------------------------------------------------
// Mutations
// ---------------------------------------------------------------------------

fn mutation_metadata(
    title: String,
    body: String,
    labels: Vec<String>,
    assignees: Vec<String>,
    milestone: Option<u64>,
) -> Result<Value, ForgeError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(ForgeError::other("issue title cannot be empty"));
    }
    if milestone == Some(0) {
        return Err(ForgeError::other(
            "milestone number must be greater than zero",
        ));
    }
    Ok(serde_json::json!({
        "title": title,
        "body": body,
        "labels": clean_string_list(&labels),
        "assignees": clean_string_list(&assignees),
        "milestone": milestone,
    }))
}

fn string_set(value: &Value, key: &str) -> BTreeSet<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// GitHub can accept an issue PATCH while silently omitting metadata the
/// token cannot change. Compare the refreshed detail with the exact payload
/// so the caller gets an explicit report instead of a false success.
fn unchanged_issue_fields(payload: &Value, detail: &IssueDetail) -> Vec<&'static str> {
    let mut unchanged = Vec::new();
    if payload.get("title").and_then(Value::as_str) != Some(detail.summary.title.as_str()) {
        unchanged.push("title");
    }
    if payload.get("body").and_then(Value::as_str) != Some(detail.body.as_str()) {
        unchanged.push("body");
    }
    let returned_labels: BTreeSet<String> = detail
        .summary
        .labels
        .iter()
        .map(|label| label.name.clone())
        .collect();
    if string_set(payload, "labels") != returned_labels {
        unchanged.push("labels");
    }
    let returned_assignees: BTreeSet<String> = detail
        .summary
        .assignees
        .iter()
        .map(|user| user.login.clone())
        .collect();
    if string_set(payload, "assignees") != returned_assignees {
        unchanged.push("assignees");
    }
    let requested_milestone = payload.get("milestone").and_then(Value::as_u64);
    let returned_milestone = detail
        .summary
        .milestone
        .as_ref()
        .map(|milestone| milestone.number);
    if requested_milestone != returned_milestone {
        unchanged.push("milestone");
    }
    unchanged
}

pub fn issue_create_sync(
    repo: &ForgeRepo,
    remote_override: Option<&str>,
    title: String,
    body: String,
    labels: Vec<String>,
    assignees: Vec<String>,
    milestone: Option<u64>,
) -> Result<IssueDetail, ForgeError> {
    let payload = mutation_metadata(title, body, labels, assignees, milestone)?;
    let remote = super::resolve_remote(repo, remote_override)?;
    let endpoint = format!("{}/issues", repository_path(&remote));
    let response = request(
        repo,
        &remote,
        "POST",
        &endpoint,
        &[],
        Some(&payload),
        ACTION_TIMEOUT,
    )?;
    let number = response_object(&response)?
        .get("number")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    parse_detail_response(&response, number)
}

#[allow(clippy::too_many_arguments)] // Fixed Issues IPC mutation contract.
pub fn issue_update_sync(
    repo: &ForgeRepo,
    remote_override: Option<&str>,
    number: u64,
    title: String,
    body: String,
    labels: Vec<String>,
    assignees: Vec<String>,
    milestone: Option<u64>,
) -> Result<IssueDetail, ForgeError> {
    validate_issue_number(number)?;
    let payload = mutation_metadata(title, body, labels, assignees, milestone)?;
    let remote = super::resolve_remote(repo, remote_override)?;
    let endpoint = issue_endpoint(&remote, number);
    let response = request(
        repo,
        &remote,
        "PATCH",
        &endpoint,
        &[],
        Some(&payload),
        ACTION_TIMEOUT,
    )?;
    let _ = response_object(&response)?;
    // Re-read so silently dropped metadata updates are reflected in the
    // returned detail instead of presenting the requested values as saved.
    let detail = issue_detail_sync(repo, remote_override, number)?;
    let unchanged = unchanged_issue_fields(&payload, &detail);
    if unchanged.is_empty() {
        Ok(detail)
    } else {
        Err(ForgeError::other(format!(
            "issue updated, but GitHub did not apply requested changes to: {}",
            unchanged.join(", ")
        )))
    }
}

pub fn issue_comment_sync(
    repo: &ForgeRepo,
    remote_override: Option<&str>,
    number: u64,
    body: String,
) -> Result<IssueComment, ForgeError> {
    validate_issue_number(number)?;
    if body.trim().is_empty() {
        return Err(ForgeError::other("comment body cannot be empty"));
    }
    let remote = super::resolve_remote(repo, remote_override)?;
    let endpoint = format!("{}/issues/{number}/comments", repository_path(&remote));
    let payload = serde_json::json!({ "body": body });
    let response = request(
        repo,
        &remote,
        "POST",
        &endpoint,
        &[],
        Some(&payload),
        ACTION_TIMEOUT,
    )?;
    let value = response_object(&response)?;
    parse_issue_comment(&value, number)
        .ok_or_else(|| ForgeError::other("GitHub did not return the created comment"))
}

pub fn issue_set_state_sync(
    repo: &ForgeRepo,
    remote_override: Option<&str>,
    number: u64,
    state: String,
    comment: Option<String>,
) -> Result<IssueDetail, ForgeError> {
    validate_issue_number(number)?;
    if !matches!(state.as_str(), "open" | "closed") {
        return Err(ForgeError::other("issue state must be `open` or `closed`"));
    }
    let remote = super::resolve_remote(repo, remote_override)?;
    let endpoint = issue_endpoint(&remote, number);
    let payload = serde_json::json!({ "state": state });
    let response = request(
        repo,
        &remote,
        "PATCH",
        &endpoint,
        &[],
        Some(&payload),
        ACTION_TIMEOUT,
    )?;
    let _ = response_object(&response)?;
    if let Some(comment) = comment.filter(|comment| !comment.trim().is_empty()) {
        let comments_endpoint = format!("{}/issues/{number}/comments", repository_path(&remote));
        let payload = serde_json::json!({ "body": comment });
        let response = request(
            repo,
            &remote,
            "POST",
            &comments_endpoint,
            &[],
            Some(&payload),
            ACTION_TIMEOUT,
        )?;
        let _ = response_object(&response)?;
    }
    issue_detail_sync(repo, remote_override, number)
}

// ---------------------------------------------------------------------------
// Identity and bounded activity
// ---------------------------------------------------------------------------

pub fn issue_identity_sync(
    repo: &ForgeRepo,
    remote_override: Option<&str>,
) -> Result<IssueIdentity, ForgeError> {
    let remote = super::resolve_remote(repo, remote_override)?;
    let response = request(repo, &remote, "GET", "/user", &[], None, LIST_TIMEOUT)?;
    let value = response_object(&response)?;
    let user = parse_issue_user(&value);
    if user.login.is_empty() {
        return Err(ForgeError::other(
            "GitHub did not return the authenticated user",
        ));
    }
    let (executor, device_id) = match &repo.remote {
        None => ("local", None),
        Some(remote) => ("device", Some(remote.device_id.clone())),
    };
    Ok(IssueIdentity {
        executor: executor.to_string(),
        device_id,
        github_host: remote.host,
        owner: remote.owner,
        repo: remote.repo,
        remote: remote.remote_name,
        user,
    })
}

fn activity_endpoint(cursor: Option<&str>, endpoint: &str) -> Result<String, ForgeError> {
    match cursor {
        Some(cursor) if cursor.starts_with('/') => {
            let path = cursor.split('?').next().unwrap_or_default();
            if path == endpoint {
                Ok(cursor.to_string())
            } else {
                Err(ForgeError::other("invalid Issues activity cursor"))
            }
        }
        Some(_) => Ok(endpoint.to_string()),
        None => Ok(endpoint.to_string()),
    }
}

fn activity_params(cursor: Option<&str>, comments: bool) -> Vec<(&str, String)> {
    if cursor.is_some_and(|cursor| cursor.starts_with('/')) {
        // Link URLs already carry the original filters and page number.
        return Vec::new();
    }
    let mut params = vec![("per_page", ACTIVITY_PER_PAGE.to_string())];
    if !comments {
        params.push(("state", "all".to_string()));
        params.push(("sort", "updated".to_string()));
        params.push(("direction", "desc".to_string()));
    }
    if let Some(cursor) = cursor.filter(|cursor| !cursor.starts_with('/')) {
        params.push(("since", cursor.to_string()));
    }
    params
}

fn exact_cursor(response: &ApiResponse, endpoint: &str) -> Option<String> {
    continuation_cursor(response, endpoint)
        .filter(|cursor| cursor.split('?').next() == Some(endpoint))
}

/// One bounded page per lane, with at most one issue-detail fallback per
/// unresolved comment reference. A non-null cursor is an opaque, same-endpoint
/// Link path safe to persist; `complete` means both lanes have no next page.
pub fn issue_activity_sync(
    repo: &ForgeRepo,
    remote_override: Option<&str>,
    issue_since: Option<String>,
    comment_since: Option<String>,
) -> Result<IssueActivityBatch, ForgeError> {
    let remote = super::resolve_remote(repo, remote_override)?;
    let root = repository_path(&remote);
    let issues_endpoint = format!("{root}/issues");
    let comments_endpoint = format!("{root}/issues/comments");
    let issue_cursor = activity_endpoint(issue_since.as_deref(), &issues_endpoint)?;
    let comment_cursor = activity_endpoint(comment_since.as_deref(), &comments_endpoint)?;
    let issue_params = activity_params(issue_since.as_deref(), false);
    let comment_params = activity_params(comment_since.as_deref(), true);
    let issue_refs: Vec<(&str, &str)> = issue_params
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    let comment_refs: Vec<(&str, &str)> = comment_params
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    let issue_response = request(
        repo,
        &remote,
        "GET",
        &issue_cursor,
        &issue_refs,
        None,
        LIST_TIMEOUT,
    )?;
    let comment_response = request(
        repo,
        &remote,
        "GET",
        &comment_cursor,
        &comment_refs,
        None,
        LIST_TIMEOUT,
    )?;
    let issue_values = response_array(&issue_response)?;
    let comment_values = response_array(&comment_response)?;
    let (mut issues, mut titles) = activity_issue_index(&issue_values);
    resolve_activity_issues(repo, &remote, &mut issues, &mut titles, &comment_values)?;
    let comments = activity_comments(&comment_values, &titles);
    let next_issue_cursor = exact_cursor(&issue_response, &issues_endpoint);
    let next_comment_cursor = exact_cursor(&comment_response, &comments_endpoint);
    let complete = next_issue_cursor.is_none() && next_comment_cursor.is_none();
    Ok(IssueActivityBatch {
        issues,
        comments,
        next_issue_cursor,
        next_comment_cursor,
        complete,
    })
}

// ---------------------------------------------------------------------------
// Focused pure tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn issue(number: u64, marker: bool) -> Value {
        let mut value = json!({
            "id": number,
            "node_id": format!("I_{number}"),
            "number": number,
            "title": format!("Issue {number}"),
            "state": "open",
            "user": {"id": 1, "node_id": "U_1", "login": "alice"},
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-02T00:00:00Z",
            "closed_at": null,
            "html_url": format!("https://github.com/a/b/issues/{number}"),
            "labels": [{"id": 2, "node_id": "L_2", "name": "bug", "color": "ff0000", "description": null}],
            "assignees": [{"id": 1, "node_id": "U_1", "login": "alice"}],
            "milestone": null,
            "comments": 3,
            "locked": false,
            "state_reason": null,
            "body": "body"
        });
        if marker {
            value["pull_request"] = json!({"url": "https://api.github.com/repos/a/b/pulls/1"});
        }
        value
    }

    fn comment(id: u64, number: u64) -> Value {
        json!({
            "id": id,
            "node_id": format!("IC_{id}"),
            "issue_url": format!("https://api.github.com/repos/a/b/issues/{number}"),
            "html_url": format!("https://github.com/a/b/issues/{number}#issuecomment-{id}"),
            "user": {"id": 1, "node_id": "U_1", "login": "alice"},
            "body": "hello",
            "created_at": "2026-01-03T00:00:00Z",
            "updated_at": "2026-01-03T00:00:00Z"
        })
    }

    #[test]
    fn parsers_keep_stable_ids_and_exclude_only_marked_prs() {
        let summary = parse_issue_summary(&issue(12, false)).unwrap();
        assert_eq!(summary.node_id, "I_12");
        assert_eq!(summary.author.node_id, "U_1");
        assert_eq!(summary.labels[0].node_id, "L_2");
        assert_eq!(summary.comment_count, 3);
        assert!(parse_issue_summary(&issue(13, true)).is_none());
        assert!(is_pull_request(&issue(13, true)));
        assert!(!is_pull_request(&issue(12, false)));
        let detail = parse_issue_detail(&issue(12, false)).unwrap();
        assert_eq!(detail.summary.number, 12);
        assert_eq!(detail.body, "body");
        assert!(detail.issue_type.is_none());
    }

    #[test]
    fn parsers_are_tolerant_of_missing_and_malformed_fields() {
        let value = json!({"number": 4, "title": "minimal", "labels": [null, {"name": "ok"}], "user": null});
        let summary = parse_issue_summary(&value).unwrap();
        assert_eq!(summary.node_id, "");
        assert_eq!(summary.author.login, "");
        assert_eq!(summary.labels.len(), 1);
        assert!(parse_issue_summary(&json!({"number": 0})).is_none());
        assert!(parse_issue_summary(&json!({"title": "no number"})).is_none());
        assert!(parse_issue_array_json("not json").is_empty());
        assert!(parse_comment_array_json("not json", 1).is_empty());
    }

    #[test]
    fn detail_type_and_activity_comment_flatten_for_the_wire() {
        let mut value = issue(8, false);
        value["type"] = json!({"name": "Task"});
        let detail = serde_json::to_value(parse_issue_detail(&value).unwrap()).unwrap();
        assert_eq!(detail["number"], 8);
        assert_eq!(detail["body"], "body");
        assert_eq!(detail["issue_type"]["name"], "Task");
        assert!(detail.get("summary").is_none());

        let values = vec![comment(20, 8)];
        let titles = HashMap::from([(8, "Issue 8".to_string())]);
        let activity = activity_comments(&values, &titles);
        let wire = serde_json::to_value(&activity[0]).unwrap();
        assert_eq!(wire["issue_number"], 8);
        assert_eq!(wire["issue_title"], "Issue 8");
        assert!(wire.get("comment").is_none());
    }

    #[test]
    fn comment_issue_numbers_come_from_urls_with_fallback() {
        assert_eq!(
            issue_number_from_url("https://api.github.com/repos/a/b/issues/42"),
            Some(42)
        );
        assert_eq!(
            issue_number_from_url("https://github.com/a/b/issues/42#issuecomment-7"),
            Some(42)
        );
        assert_eq!(
            issue_number_from_url("https://github.com/a/b/pull/42"),
            None
        );
        let parsed = parse_issue_comment(&comment(7, 42), 0).unwrap();
        assert_eq!(parsed.issue_number, 42);
        let fallback = parse_issue_comment(&json!({"id": 7, "body": "x"}), 9).unwrap();
        assert_eq!(fallback.issue_number, 9);
        assert!(parse_issue_comment(&json!({"id": 7}), 0).is_none());
    }

    #[test]
    fn activity_index_excludes_pr_rows_and_comments() {
        let values = vec![issue(1, false), issue(2, true), issue(3, false)];
        let (issues, titles) = activity_issue_index(&values);
        assert_eq!(
            issues.iter().map(|issue| issue.number).collect::<Vec<_>>(),
            vec![1, 3]
        );
        let comments =
            activity_comments(&[comment(10, 1), comment(11, 2), comment(12, 3)], &titles);
        assert_eq!(
            comments
                .iter()
                .map(|entry| entry.comment.issue_number)
                .collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert_eq!(comments[0].issue_title, "Issue 1");
    }

    #[test]
    fn activity_resolution_keeps_real_issues_and_excludes_prs() {
        let (mut issues, mut titles) = activity_issue_index(&[]);
        merge_activity_issues(
            &[issue(101, false), issue(102, true)],
            &mut issues,
            &mut titles,
        );
        assert_eq!(
            issues.iter().map(|issue| issue.number).collect::<Vec<_>>(),
            vec![101]
        );
        assert_eq!(titles.get(&101).map(String::as_str), Some("Issue 101"));
        assert!(!titles.contains_key(&102));
        let comments = activity_comments(&[comment(20, 101), comment(21, 102)], &titles);
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].issue_title, "Issue 101");
    }

    #[test]
    fn query_normalization_clamps_and_deduplicates() {
        let query = normalize_issue_query(IssueListQuery {
            state: "bad".into(),
            search: "  help   wanted ".into(),
            labels: vec![" bug ".into(), "bug".into(), "".into()],
            assignees: vec![" alice ".into(), "alice".into(), "".into()],
            milestone: " v1 ".into(),
            sort: "bad".into(),
            direction: "bad".into(),
            per_page: 500,
            page: 0,
        });
        assert_eq!(query.state, "open");
        assert_eq!(query.search, "help wanted");
        assert_eq!(query.labels, vec!["bug"]);
        assert_eq!(query.assignees, vec!["alice"]);
        assert_eq!(query.per_page, 100);
        assert_eq!(query.page, 1);
        assert_eq!(
            normalize_issue_query(IssueListQuery {
                per_page: 0,
                ..Default::default()
            })
            .per_page,
            1
        );
    }

    #[test]
    fn search_quotes_qualifiers_and_scopes_repository() {
        let query = normalize_issue_query(IssueListQuery {
            search: "unexpected failure".into(),
            state: "all".into(),
            labels: vec!["help wanted".into()],
            assignees: vec!["a\"b".into(), "carol".into()],
            milestone: "v1\\beta".into(),
            ..Default::default()
        });
        let search = build_search_query(&query, "acme", "widget");
        assert!(search.contains("repo:\"acme/widget\""));
        assert!(search.contains("is:issue"));
        assert!(search.contains("unexpected failure"));
        assert!(search.contains("label:\"help wanted\""));
        assert!(search.contains("assignee:\"a\\\"b\""));
        assert!(search.contains("assignee:\"carol\""));
        assert!(search.contains("milestone:\"v1\\\\beta\""));
        assert!(!search.contains("state:"));
    }

    #[test]
    fn repository_paths_are_percent_encoded() {
        let remote = ResolvedRemote {
            remote_name: "origin".into(),
            remote_url: String::new(),
            host: "github.com".into(),
            owner: "a b".into(),
            repo: "c/d".into(),
        };
        assert_eq!(repository_path(&remote), "/repos/a%20b/c%2Fd");
        assert_eq!(issue_endpoint(&remote, 12), "/repos/a%20b/c%2Fd/issues/12");
    }

    #[test]
    fn activity_cursors_are_opaque_and_endpoint_scoped() {
        let response = super::super::api::parse_api_response(
            "HTTP/2 200 OK\nLink: <https://api.github.com/repos/a/b/issues?page=2>; rel=\"next\"\n\n[]",
        )
        .unwrap();
        let endpoint = "/repos/a/b/issues";
        assert_eq!(
            exact_cursor(&response, endpoint).as_deref(),
            Some("/repos/a/b/issues?page=2")
        );
        assert!(exact_cursor(&response, "/repos/a/b/issues/1/comments").is_none());
        assert_eq!(
            activity_endpoint(Some("/repos/a/b/issues?page=2"), endpoint).unwrap(),
            "/repos/a/b/issues?page=2"
        );
        assert!(activity_endpoint(Some("/user"), endpoint).is_err());
        let params = activity_params(Some("2026-01-01T00:00:00Z"), false);
        assert!(params.contains(&("since", "2026-01-01T00:00:00Z".to_string())));
    }

    #[test]
    fn mutations_validate_inputs_without_touching_the_network() {
        assert!(mutation_metadata("".into(), String::new(), vec![], vec![], None).is_err());
        assert!(mutation_metadata("x".into(), String::new(), vec![], vec![], Some(0)).is_err());
        let payload = mutation_metadata(
            " title ".into(),
            "body".into(),
            vec![" bug ".into()],
            vec!["alice".into()],
            Some(2),
        )
        .unwrap();
        assert_eq!(payload["title"], "title");
        assert_eq!(payload["labels"], json!(["bug"]));
        assert_eq!(payload["milestone"], 2);
        let mut value = issue(5, false);
        value["title"] = json!("title");
        value["milestone"] = json!({"number": 2, "title": "v2"});
        let detail = parse_issue_detail(&value).unwrap();
        assert!(unchanged_issue_fields(&payload, &detail).is_empty());
        value["labels"] = json!([]);
        let detail = parse_issue_detail(&value).unwrap();
        assert_eq!(unchanged_issue_fields(&payload, &detail), vec!["labels"]);
        assert!(validate_issue_number(0).is_err());
        assert!(validate_issue_number(1).is_ok());
    }

    #[test]
    fn metadata_and_activity_budgets_are_bounded() {
        assert_eq!(METADATA_PER_PAGE, 100);
        assert_eq!(MAX_METADATA_PAGES, 100);
        assert_eq!(ACTIVITY_PER_PAGE, 100);
        // Keep the orchestration timeouts visibly non-zero.
        assert!(LIST_TIMEOUT > std::time::Duration::ZERO);
        assert!(ACTION_TIMEOUT > std::time::Duration::ZERO);
    }
}
