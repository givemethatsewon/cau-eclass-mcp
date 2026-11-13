//! Read/download behavior ported from the Python service.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, Datelike, SecondsFormat, Utc};
use chrono_tz::Asia::Seoul;
use futures_util::{StreamExt, stream};
use regex::Regex;
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::{
    catalog::{
        Material, board_materials, compact_modules, course_matches, filter_materials,
        material_catalog, parse_resource_id, term_key, term_key_for_month, week_from_title,
    },
    documents::{inspect_pdf, read_pdf, sanitize_text},
    error::EclassError,
    http::HttpTransport,
};

/// A paginated Canvas response.
#[derive(Debug, Clone)]
pub struct Paged {
    pub items: Vec<Value>,
    pub complete: bool,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct FileFingerprint {
    path: String,
    name: String,
    size: u64,
    sha256: String,
}

#[derive(Debug, Clone, Serialize)]
struct SubmissionPlan {
    plan_id: String,
    course_id: u32,
    assignment_id: u32,
    assignment_name: String,
    user_id: u64,
    files: Vec<FileFingerprint>,
    previous_attempt: Option<i64>,
    previous_submitted_at: Option<String>,
    expires_at: DateTime<Utc>,
    problems: Vec<String>,
    ready: bool,
    state: String,
}

#[derive(Debug, Clone)]
struct CachedMaterial {
    revision: (Option<String>, Option<String>),
    result: Value,
}

/// One catalog pass shared by `list_materials` and `get_week_materials`.
#[derive(Debug, Clone)]
struct MaterialView {
    catalog: Vec<Material>,
    modules: Vec<Value>,
    complete: bool,
    warnings: Vec<String>,
    notes: Vec<String>,
}

/// e-Class reads, materials and downloads.
#[derive(Debug)]
pub struct EClassService {
    transport: Arc<HttpTransport>,
    output_dir: PathBuf,
    oz_helper: PathBuf,
    enable_submission: bool,
    enable_delete: bool,
    material_cache: Mutex<HashMap<String, CachedMaterial>>,
    material_lock: Mutex<()>,
    plans: Mutex<HashMap<String, SubmissionPlan>>,
    submission_lock: Mutex<()>,
}

impl EClassService {
    /// Build a service around a transport.
    ///
    /// # Errors
    ///
    /// Returns an error when the output directory cannot be resolved.
    pub fn new(
        transport: Arc<HttpTransport>,
        output_dir: Option<PathBuf>,
        enable_submission: bool,
    ) -> Self {
        let output_dir = output_dir.unwrap_or_else(default_output_dir);
        let oz_helper = std::env::var_os("ECLASS_OZ_HELPER")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("eclass-oz-export"));
        Self {
            transport,
            output_dir,
            oz_helper,
            enable_submission,
            enable_delete: std::env::var("ECLASS_ENABLE_DELETE").as_deref() == Ok("1"),
            material_cache: Mutex::new(HashMap::new()),
            material_lock: Mutex::new(()),
            plans: Mutex::new(HashMap::new()),
            submission_lock: Mutex::new(()),
        }
    }

    /// Override the OZ helper executable (used by tests and packaging).
    #[must_use]
    pub fn with_oz_helper(mut self, path: PathBuf) -> Self {
        self.oz_helper = path;
        self
    }

    /// Allow or refuse file deletion (used by tests).
    #[must_use]
    pub fn with_delete_enabled(mut self, enabled: bool) -> Self {
        self.enable_delete = enabled;
        self
    }

    /// The underlying transport.
    #[must_use]
    pub fn transport(&self) -> Arc<HttpTransport> {
        Arc::clone(&self.transport)
    }

    /// Clear cached GET responses.
    pub async fn invalidate(&self) {
        self.transport.invalidate().await;
    }

    async fn get(&self, path: &str, learningx: bool, html: bool) -> Result<Value, EclassError> {
        Ok(self.transport.get(path, learningx, html).await?.body)
    }

    async fn paged(
        &self,
        path: &str,
        learning_x: bool,
        max_pages: usize,
    ) -> Result<Paged, EclassError> {
        paged_on(&self.transport, path, learning_x, max_pages).await
    }

    /// Current-user session status.
    pub async fn session_status(&self) -> Result<Value, EclassError> {
        if !self.transport.has_session() {
            return Ok(json!({
                "authenticated": false,
                "session_state": "missing",
                "submission_enabled": self.enable_submission,
                "transport": "http"
            }));
        }
        let user = self.get("/api/v1/users/self/profile", false, false).await?;
        if user.get("id").is_none() {
            return Err(EclassError::NoProfile);
        }
        let session = self.transport.session();
        Ok(json!({
            "authenticated": true,
            "session_state": "authenticated",
            "user": select(&user, &["id", "name"]),
            "learningx_token": session
                .as_ref()
                .and_then(|session| session.token("xn_api_token"))
                .is_some(),
            "learningx_token_age_secs": session
                .as_ref()
                .and_then(crate::session::Session::learningx_token_age_secs),
            "submission_enabled": self.enable_submission,
            "transport": "http"
        }))
    }

    /// Structured first-run status for an agent guiding the user.
    pub async fn onboarding_status(&self) -> Value {
        if !self.transport.has_session() {
            return json!({
                "schema_version": 1,
                "session_state": "missing",
                "authenticated": false,
                "learningx_token": false,
                "courses_visible": null,
                "ready": false,
                "next_actions": [action(
                    "RUN_ONBOARD",
                    "Sign in to e-Class once in the dedicated browser.",
                    Some("eclass-mcp onboard --client <client>")
                )],
                "session_file": self.transport.session_file(),
                "error": null
            });
        }
        let mut session = self.transport.session();
        let (authenticated, auth_error) = match self.session_status().await {
            Ok(status) => (
                status
                    .get("authenticated")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                None,
            ),
            Err(error) => (false, Some(error)),
        };
        let session_state = if session.is_none() {
            "missing"
        } else if authenticated {
            "authenticated"
        } else if auth_error.as_ref().is_some_and(is_session_expired) {
            "expired"
        } else {
            "unavailable"
        };
        if authenticated
            && session
                .as_ref()
                .and_then(|session| session.token("xn_api_token"))
                .is_none()
            && self.transport.mint_learningx_token().await.unwrap_or(false)
        {
            session = self.transport.session();
        }
        let learningx_token = session
            .as_ref()
            .and_then(|session| session.token("xn_api_token"))
            .is_some();
        let mut learningx_token_age_secs = session
            .as_ref()
            .and_then(crate::session::Session::learningx_token_age_secs);
        let courses = if authenticated {
            self.courses(None, None, false).await
        } else {
            Err(EclassError::SessionExpired)
        };
        let courses_visible = courses
            .as_ref()
            .ok()
            .and_then(|value| value.get("visible"))
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok());
        let first_course = courses
            .as_ref()
            .ok()
            .and_then(|value| value.get("items"))
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|course| course.get("course_id"))
            .and_then(positive_u64)
            .and_then(|value| u32::try_from(value).ok());
        let mut learningx_ok = false;
        if authenticated
            && learningx_token
            && let Some(course_id) = first_course
        {
            learningx_ok = self.boards(course_id).await.is_ok();
            if !learningx_ok
                && self
                    .transport
                    .refresh_learningx_token()
                    .await
                    .unwrap_or(false)
            {
                learningx_ok = self.boards(course_id).await.is_ok();
                if learningx_ok {
                    session = self.transport.session();
                    learningx_token_age_secs = session
                        .as_ref()
                        .and_then(crate::session::Session::learningx_token_age_secs);
                }
            }
        }

        let mut actions = Vec::new();
        match session_state {
            "missing" => actions.push(action(
                "RUN_ONBOARD",
                "Sign in to e-Class once in the dedicated browser.",
                Some("eclass-mcp onboard --client <client>"),
            )),
            "expired" => actions.push(action(
                "SESSION_EXPIRED",
                "The saved session expired; sign in again in the browser.",
                Some("eclass-mcp login"),
            )),
            "unavailable" => actions.push(action(
                "CHECK_CONNECTION",
                "The saved session could not be verified; check the network and sign in again if needed.",
                Some("eclass-mcp doctor"),
            )),
            _ => {}
        }
        if authenticated {
            if !learningx_ok {
                actions.push(action(
                    "REFRESH_LEARNINGX_TOKEN",
                    "LearningX needs a fresh API token; mint it with the browser session or Aside.",
                    Some("eclass-mcp auth refresh --via aside"),
                ));
            }
            if let Err(error) = &courses {
                actions.push(action(
                    "CHECK_ENROLLMENT",
                    &format!(
                        "Course access check failed ({error}); verify enrollment or sign in again."
                    ),
                    None,
                ));
            } else if courses_visible == Some(0) {
                actions.push(action(
                    "CHECK_ENROLLMENT",
                    "No active courses were returned; verify the account and term.",
                    None,
                ));
            }
        }
        let ready = authenticated && learningx_ok && courses_visible.is_some_and(|count| count > 0);
        if ready {
            actions.push(action(
                "CONFIGURE_CLIENT",
                "Configure the MCP client and restart the connection.",
                Some("eclass-mcp config --client <client>"),
            ));
        }
        json!({
            "schema_version": 1,
            "session_state": session_state,
            "authenticated": authenticated,
            "canvas_ok": authenticated,
            "learningx_token": learningx_token,
            "learningx_ok": learningx_ok,
            "learningx_token_age_secs": learningx_token_age_secs,
            "courses_visible": courses_visible,
            "ready": ready,
            "next_actions": actions,
            "session_file": self.transport.session_file(),
            "error": auth_error.map(|error| error.to_string())
        })
    }

    /// List courses with optional title/code and term filters.
    ///
    /// Without an explicit term the list is scoped to the academic term that
    /// contains today, so a default call returns the courses a student is
    /// taking right now instead of every course they have ever taken.
    pub async fn courses(
        &self,
        query: Option<&str>,
        term: Option<&str>,
        all_terms: bool,
    ) -> Result<Value, EclassError> {
        let mut result = self
            .paged("/api/v1/courses?include[]=term&per_page=100", false, 20)
            .await?;
        let visible = result.items.len();
        // The term that contains today is decided from every visible course,
        // not from the query results: a query that only matches old courses
        // must not pretend one of those old terms is the current one.
        let current = current_term(&result.items);
        result
            .items
            .retain(|course| course_matches(course, query, term));
        let matched = result.items.len();
        let mut warnings = Vec::new();
        let mut term_applied: Option<String> = None;
        if term.is_none()
            && !all_terms
            && let Some((key, name)) = current
        {
            let scoped: Vec<Value> = result
                .items
                .iter()
                .filter(|course| course_term_key(course) == Some(key))
                .cloned()
                .collect();
            if !scoped.is_empty() {
                warnings.push(format!(
                    "showing {} of {matched} matching courses in {name}; pass all_terms=true or term=\"...\" to widen",
                    scoped.len()
                ));
                result.items = scoped;
                term_applied = Some(name);
            }
        }
        let items = result.items.iter().map(compact_course).collect::<Vec<_>>();
        Ok(json!({
            "items": items,
            "total": result.items.len(),
            "visible": visible,
            "term": term_applied,
            "complete": result.complete,
            "has_more": false,
            "next": null,
            "warnings": warnings
        }))
    }

    /// Resolve a human course name/code to a single course ID.
    ///
    /// The current term wins over older terms. A query that still matches more
    /// than one course returns [`EclassError::AmbiguousCourse`] with candidates
    /// instead of silently picking one.
    pub async fn resolve_course(&self, query: &str) -> Result<u32, EclassError> {
        let listed = self.courses(None, None, true).await?;
        let courses = listed
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let search = normalized(Some(query));
        if search.is_empty() {
            return Err(EclassError::CourseNotFound);
        }
        let current = current_term(&courses).map(|(key, _)| key);
        let haystack = |course: &Value| {
            ["name", "course_code"]
                .iter()
                .filter_map(|key| course.get(*key).and_then(Value::as_str))
                .any(|value| normalized(Some(value)).contains(&search))
        };
        let exact = |course: &Value| {
            ["name", "course_code"]
                .iter()
                .filter_map(|key| course.get(*key).and_then(Value::as_str))
                .any(|value| normalized(Some(value)) == search)
        };
        // Exact name, then substring; each pass prefers the current term.
        for matcher in [&exact as &dyn Fn(&Value) -> bool, &haystack] {
            for require_current in [true, false] {
                let mut matches: Vec<Value> = courses
                    .iter()
                    .filter(|course| matcher(course))
                    .filter(|course| {
                        !require_current
                            || current.is_some_and(|key| course_term_key(course) == Some(key))
                    })
                    .cloned()
                    .collect();
                match matches.len() {
                    0 => {}
                    1 => {
                        return matches
                            .remove(0)
                            .get("course_id")
                            .and_then(positive_u64)
                            .and_then(|id| u32::try_from(id).ok())
                            .ok_or(EclassError::CourseNotFound);
                    }
                    count => {
                        sort_courses_for_briefing(&mut matches);
                        let candidates = matches
                            .iter()
                            .take(10)
                            .map(|course| {
                                json!({
                                    "course_id": course.get("course_id").and_then(positive_u64)
                                        .map_or(Value::Null, |id| json!(id)),
                                    "name": course.get("name").cloned().unwrap_or(Value::Null),
                                    "term": course.get("term").cloned().unwrap_or(Value::Null)
                                })
                            })
                            .collect::<Vec<_>>();
                        return Err(EclassError::AmbiguousCourse {
                            query: query.to_string(),
                            count,
                            candidates: Value::Array(candidates),
                        });
                    }
                }
            }
        }
        Err(EclassError::CourseNotFound)
    }

    /// Canvas todo items.
    pub async fn todo(&self) -> Result<Value, EclassError> {
        let result = self
            .paged("/api/v1/users/self/todo?per_page=100", false, 20)
            .await?;
        let truncated = result.next_cursor.is_some();
        let mut payload = list_response(result.items, result.complete && !truncated, None, None);
        if truncated && let Some(object) = payload.as_object_mut() {
            object.insert(
                "warnings".to_string(),
                json!(["more todo items exist beyond the page limit"]),
            );
        }
        Ok(payload)
    }

    /// One bounded pass over courses, todo items and recent announcements.
    pub async fn briefing(&self, days: u32, course_limit: usize) -> Result<Value, EclassError> {
        if !(1..=60).contains(&days) {
            return Err(EclassError::InvalidArgument(
                "days must be between 1 and 60".to_string(),
            ));
        }
        if !(1..=30).contains(&course_limit) {
            return Err(EclassError::InvalidArgument(
                "course_limit must be between 1 and 30".to_string(),
            ));
        }
        let now = Utc::now().with_timezone(&Seoul);
        let today = now.date_naive();
        // Announcements look back, deadlines look forward: two explicit windows.
        let recent_start = today - chrono::Duration::days(i64::from(days) - 1);
        let upcoming_end = today + chrono::Duration::days(i64::from(days));
        let mut warnings = Vec::new();

        let listed = self.courses(None, None, false).await?;
        let mut complete = listed
            .get("complete")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut ordered = listed
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let visible = ordered.len();
        sort_courses_for_briefing(&mut ordered);
        let selected: Vec<Value> = ordered.iter().take(course_limit).cloned().collect();
        if visible > selected.len() {
            warnings.push(format!(
                "course_limit={course_limit} truncated {} of {visible} current-term courses",
                visible - selected.len()
            ));
        }

        let todo = match self.todo().await {
            Ok(todo) => todo,
            Err(error) => {
                warnings.push(format!("Todo list unavailable: {error}"));
                json!({"items": [], "complete": false})
            }
        };
        complete &= todo
            .get("complete")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let tasks = selected.iter().map(|course| {
            let transport = Arc::clone(&self.transport);
            let course = course.clone();
            let start = recent_start.to_string();
            let today = today.to_string();
            async move {
                let course_id = course
                    .get("course_id")
                    .and_then(positive_u64)
                    .unwrap_or_default();
                let result = announcements_on(&transport, course_id, &start, &today).await;
                (course, result)
            }
        });
        let gathered: Vec<_> = stream::iter(tasks).buffer_unordered(4).collect().await;

        let mut announcements = Vec::new();
        for (course, result) in gathered {
            let course_id = course
                .get("course_id")
                .and_then(positive_u64)
                .unwrap_or_default();
            let course_name = course
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match result {
                Ok(result) => {
                    complete &= result.complete;
                    for announcement in result.items {
                        let posted = announcement
                            .get("posted_at")
                            .or_else(|| announcement.get("created_at"))
                            .cloned()
                            .unwrap_or(Value::Null);
                        announcements.push(json!({
                            "id": announcement.get("id").cloned().unwrap_or(Value::Null),
                            "course_id": course_id,
                            "course_name": course_name,
                            "title": announcement.get("title").or_else(|| announcement.get("topic_title")).cloned().unwrap_or(Value::Null),
                            "posted_at": posted,
                            "read_state": announcement.get("read_state").cloned().unwrap_or(Value::Null),
                            "author": announcement.get("author").and_then(|author| author.get("display_name")).cloned().unwrap_or(Value::Null)
                        }));
                    }
                }
                Err(error) => {
                    complete = false;
                    warnings.push(format!(
                        "Course {course_id} announcements unavailable: {error}"
                    ));
                }
            }
        }
        announcements.sort_by(|left, right| {
            right
                .get("posted_at")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .cmp(
                    left.get("posted_at")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                )
        });
        let unread = announcements
            .iter()
            .filter(|announcement| {
                announcement.get("read_state").and_then(Value::as_str) != Some("read")
            })
            .count();

        let course_names: HashMap<String, String> = ordered
            .iter()
            .filter_map(|course| {
                let id = course.get("course_id").and_then(positive_u64)?;
                let name = course.get("name").and_then(Value::as_str)?;
                Some((id.to_string(), name.to_string()))
            })
            .collect();
        let mut deadlines = Vec::new();
        let mut overdue = Vec::new();
        let mut later = 0_usize;
        for entry in todo
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            for key in ["assignment", "quiz"] {
                let Some(value) = entry.get(key).and_then(Value::as_object) else {
                    continue;
                };
                let Some(due_at) = value.get("due_at").and_then(Value::as_str) else {
                    continue;
                };
                let Some(due_date) = DateTime::parse_from_rfc3339(due_at)
                    .ok()
                    .map(|date| date.with_timezone(&Seoul).date_naive())
                else {
                    continue;
                };
                let mut item = Map::new();
                item.insert(
                    "kind".to_string(),
                    json!(if key == "assignment" {
                        "assignment"
                    } else {
                        "quiz"
                    }),
                );
                item.insert(
                    "id".to_string(),
                    value.get("id").cloned().unwrap_or(Value::Null),
                );
                item.insert(
                    "title".to_string(),
                    value
                        .get("name")
                        .or_else(|| value.get("title"))
                        .cloned()
                        .unwrap_or(Value::Null),
                );
                item.insert(
                    "course_id".to_string(),
                    entry.get("course_id").cloned().unwrap_or(Value::Null),
                );
                let course_id = entry
                    .get("course_id")
                    .and_then(positive_u64)
                    .map(|id| id.to_string());
                let fallback_name = course_id
                    .as_deref()
                    .and_then(|id| course_names.get(id))
                    .cloned()
                    .map(Value::String)
                    .unwrap_or(Value::Null);
                item.insert(
                    "course_name".to_string(),
                    entry
                        .get("context_name")
                        .filter(|value| !value.is_null())
                        .cloned()
                        .unwrap_or(fallback_name),
                );
                item.insert(
                    "points_possible".to_string(),
                    value.get("points_possible").cloned().unwrap_or(Value::Null),
                );
                item.extend(date_fields(&Value::Object(value.clone())));
                if due_date < today {
                    item.insert(
                        "days_overdue".to_string(),
                        json!((today - due_date).num_days()),
                    );
                    overdue.push(Value::Object(item));
                } else if due_date > upcoming_end {
                    later += 1;
                } else {
                    item.insert(
                        "days_left".to_string(),
                        json!((due_date - today).num_days()),
                    );
                    deadlines.push(Value::Object(item));
                }
                break;
            }
        }
        deadlines.sort_by(|left, right| {
            left.get("due_at")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .cmp(
                    right
                        .get("due_at")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                )
        });
        overdue.sort_by(|left, right| {
            right
                .get("due_at")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .cmp(
                    left.get("due_at")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                )
        });
        let overdue_total = overdue.len();
        overdue.truncate(5);

        Ok(json!({
            "timezone": "Asia/Seoul",
            "today": today.to_string(),
            "windows": {
                "deadlines": {"start_date": today.to_string(), "end_date": upcoming_end.to_string()},
                "announcements": {"start_date": recent_start.to_string(), "end_date": today.to_string()}
            },
            "courses": selected,
            "deadlines": deadlines,
            "overdue": overdue,
            "announcements": announcements,
            "counts": {
                "courses": selected.len(),
                "deadlines": deadlines.len(),
                "overdue": overdue_total,
                "deadlines_beyond_window": later,
                "announcements": announcements.len(),
                "unread_announcements": unread
            },
            "complete": complete,
            "warnings": warnings
        }))
    }

    /// Raw LearningX modules.
    pub async fn modules(&self, course_id: u32) -> Result<Vec<Value>, EclassError> {
        let value = self
            .get(
                &format!("/learningx/api/v1/courses/{course_id}/modules?include_detail=true"),
                true,
                false,
            )
            .await?;
        value.as_array().cloned().ok_or(EclassError::UnexpectedList)
    }

    async fn learningx_boards(&self, course_id: u32) -> Result<Vec<Value>, EclassError> {
        let body = self
            .get(
                &format!("/learningx/api/v1/learningx_board/courses/{course_id}/boards"),
                true,
                false,
            )
            .await?;
        body.as_array().cloned().ok_or(EclassError::UnexpectedList)
    }

    /// Board posts with attachment metadata, newest first.
    async fn board_posts_with_attachments(
        &self,
        course_id: u32,
        board_id: u32,
        week: Option<u32>,
    ) -> Result<Vec<Value>, EclassError> {
        let mut posts = Vec::new();
        for page in 1..=10 {
            let body = self
                .get(
                    &format!(
                        "/learningx/api/v1/learningx_board/courses/{course_id}/boards/{board_id}/posts?page={page}&per_page=100"
                    ),
                    true,
                    false,
                )
                .await?;
            posts.extend(
                body.get("items")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
            );
            let last = body
                .get("pagination")
                .and_then(|pagination| pagination.get("last_page"))
                .and_then(Value::as_u64)
                .unwrap_or(1);
            if page >= last || last <= 1 {
                break;
            }
        }
        posts.sort_by(|left, right| {
            right
                .get("created_at")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .cmp(
                    left.get("created_at")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                )
        });
        if let Some(week) = week {
            posts.retain(|post| {
                post.get("title")
                    .and_then(Value::as_str)
                    .and_then(week_from_title)
                    == Some(week)
            });
        }
        posts.truncate(20);
        let mut detailed = Vec::new();
        for post in posts {
            if post.get("attachment_count").and_then(Value::as_u64) == Some(0) {
                continue;
            }
            let Some(post_id) = post.get("id").and_then(positive_u64) else {
                continue;
            };
            // One unreadable post must not hide the rest of the board.
            let detail = match self
                .get(
                    &format!(
                        "/learningx/api/v1/learningx_board/courses/{course_id}/boards/{board_id}/posts/{post_id}"
                    ),
                    true,
                    false,
                )
                .await
            {
                Ok(detail) => detail,
                Err(error) => {
                    crate::mcp::eprintln_flush(&format!(
                        "skipped board post {post_id}: {error}"
                    ));
                    continue;
                }
            };
            let mut detail = detail;
            if let Some(object) = detail.as_object_mut() {
                object.insert("board_id".to_string(), json!(board_id));
            }
            detailed.push(detail);
        }
        Ok(detailed)
    }

    /// Materials posted as attachments on LearningX boards (강의자료실).
    async fn learningx_board_materials(
        &self,
        course_id: u32,
        week: Option<u32>,
    ) -> Result<Vec<Material>, EclassError> {
        let boards = self.learningx_boards(course_id).await?;
        let preferred: Vec<&Value> = boards
            .iter()
            .filter(|board| {
                board
                    .get("title")
                    .and_then(Value::as_str)
                    .is_some_and(|title| {
                        title.contains("자료") || title.to_ascii_lowercase().contains("material")
                    })
            })
            .collect();
        let selected: Vec<&Value> = if preferred.is_empty() {
            boards.iter().collect()
        } else {
            preferred
        };
        let mut posts = Vec::new();
        for board in selected {
            let Some(board_id) = board.get("id").and_then(positive_u64) else {
                continue;
            };
            match self
                .board_posts_with_attachments(
                    course_id,
                    u32::try_from(board_id).map_err(|_| EclassError::InvalidPath)?,
                    week,
                )
                .await
            {
                Ok(mut items) => posts.append(&mut items),
                Err(EclassError::LearningxUnauthorized | EclassError::LearningxTokenMissing) => {
                    return Err(EclassError::LearningxTokenMissing);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(board_materials(course_id, &posts))
    }

    /// Week numbers advertised by modules and board post titles.
    async fn available_weeks(&self, course_id: u32, modules: &[Value]) -> Vec<u32> {
        let mut weeks = Vec::new();
        for module in modules {
            let direct = ["week_position", "week", "position"]
                .iter()
                .find_map(|key| module.get(*key).and_then(positive_u64))
                .and_then(|value| u32::try_from(value).ok());
            if let Some(week) = direct {
                weeks.push(week);
            } else if let Some(title) = module.get("title").and_then(Value::as_str)
                && let Some(week) = week_from_title(title)
            {
                weeks.push(week);
            }
        }
        if let Ok(boards) = self.learningx_boards(course_id).await {
            let preferred: Vec<&Value> = boards
                .iter()
                .filter(|board| {
                    board
                        .get("title")
                        .and_then(Value::as_str)
                        .is_some_and(|title| {
                            title.contains("자료")
                                || title.to_ascii_lowercase().contains("material")
                        })
                })
                .collect();
            let selected = if preferred.is_empty() {
                boards.iter().collect::<Vec<_>>()
            } else {
                preferred
            };
            for board in selected {
                let Some(board_id) = board.get("id").and_then(positive_u64) else {
                    continue;
                };
                let Ok(board_id) = u32::try_from(board_id) else {
                    continue;
                };
                let path = format!(
                    "/learningx/api/v1/learningx_board/courses/{course_id}/boards/{board_id}/posts?page=1&per_page=100"
                );
                if let Ok(body) = self.get(&path, true, false).await
                    && let Some(posts) = body.get("items").and_then(Value::as_array)
                {
                    weeks.extend(
                        posts
                            .iter()
                            .filter(|post| {
                                post.get("attachment_count")
                                    .and_then(Value::as_u64)
                                    .unwrap_or_default()
                                    > 0
                            })
                            .filter_map(|post| {
                                post.get("title")
                                    .and_then(Value::as_str)
                                    .and_then(week_from_title)
                            })
                            .take(20),
                    );
                }
            }
        }
        weeks.sort_unstable();
        weeks.dedup();
        weeks
    }

    /// Materials for one week, defaulting to the latest week that has any.
    pub async fn week_materials(
        &self,
        course_id: u32,
        week: Option<u32>,
    ) -> Result<Value, EclassError> {
        let view = self.material_view(course_id, None).await?;
        let mut warnings = view.warnings.clone();
        let course_weeks = module_weeks(&view.modules);
        let latest_course_week = course_weeks.last().copied();
        let mut weeks_with_materials: Vec<u32> = view
            .catalog
            .iter()
            .flat_map(|material| {
                material
                    .weeks
                    .clone()
                    .unwrap_or_else(|| material.week.into_iter().collect())
            })
            .collect();
        weeks_with_materials.sort_unstable();
        weeks_with_materials.dedup();
        let target = week.or_else(|| weeks_with_materials.last().copied());
        let Some(target) = target else {
            return Ok(json!({
                "items": [],
                "total": 0,
                "week": null,
                "requested_week": week,
                "current_week": latest_course_week,
                "course_weeks": course_weeks,
                "weeks_with_materials": weeks_with_materials,
                "complete": view.complete,
                "has_more": false,
                "next": null,
                "sources": ["commons", "canvas_file", "learningx_board"],
                "warnings": warnings,
                "notes": view.notes
            }));
        };
        let items = filter_materials(&view.catalog, Some(target), None, None);
        let mut complete = view.complete;
        if let Some(max) = course_weeks.last()
            && week.is_some_and(|requested| requested > *max)
        {
            warnings.push(format!(
                "week {target} is outside this course's weeks 1..{max}"
            ));
            complete = false;
        } else if items.is_empty() && week.is_some() {
            warnings.push(format!(
                "no materials in week {target}; weeks with materials: {}",
                format_weeks(&weeks_with_materials)
            ));
        }
        if week.is_none() {
            warnings.retain(|warning| !warning.starts_with("week "));
        }
        Ok(json!({
            "items": items,
            "total": items.len(),
            "week": target,
            "requested_week": week,
            "current_week": latest_course_week,
            "course_weeks": course_weeks,
            "weeks_with_materials": weeks_with_materials,
            "complete": complete,
            "has_more": false,
            "next": null,
            "sources": ["commons", "canvas_file", "learningx_board"],
            "warnings": warnings,
            "notes": view.notes
        }))
    }

    /// Compact weekly items or the raw provider view.
    pub async fn learning_items(
        &self,
        course_id: u32,
        week: Option<u32>,
        query: Option<&str>,
        content_type: Option<&str>,
        detail: bool,
    ) -> Result<Value, EclassError> {
        let modules = self.modules(course_id).await?;
        let weeks = self.available_weeks(course_id, &modules).await;
        let current_week = weeks.last().copied();
        if detail {
            return Ok(json!({
                "items": modules,
                "weeks": weeks,
                "current_week": current_week,
                "complete": true,
                "has_more": false,
                "next": null,
                "detail": true
            }));
        }
        let items = compact_modules(course_id, &modules, week, query, content_type);
        Ok(json!({
            "items": items,
            "weeks": weeks,
            "current_week": current_week,
            "complete": true,
            "has_more": false,
            "next": null,
            "detail": false
        }))
    }

    /// One catalog pass over modules, Canvas files and board attachments.
    async fn material_view(
        &self,
        course_id: u32,
        week: Option<u32>,
    ) -> Result<MaterialView, EclassError> {
        let mut warnings = Vec::new();
        let modules = match self.modules(course_id).await {
            Ok(modules) => modules,
            Err(EclassError::LearningxTokenMissing | EclassError::LearningxUnauthorized) => {
                warnings.push(
                    "LearningX weekly modules unavailable: run `eclass-mcp auth refresh --via aside`"
                        .to_string(),
                );
                Vec::new()
            }
            Err(error) => return Err(error),
        };
        let mut notes = Vec::new();
        let files = match self
            .paged(
                &format!("/api/v1/courses/{course_id}/files?per_page=100"),
                false,
                20,
            )
            .await
        {
            Ok(files) => files,
            Err(
                EclassError::CanvasSessionExpired
                | EclassError::PermissionDenied
                | EclassError::Http(401 | 403),
            ) => {
                notes.push(
                    "Canvas file listing is restricted for students; weekly modules and board materials are included"
                        .to_string(),
                );
                Paged {
                    items: Vec::new(),
                    complete: true,
                    next_cursor: None,
                }
            }
            Err(error) => {
                warnings.push(format!("Canvas file listing unavailable: {error}"));
                Paged {
                    items: Vec::new(),
                    complete: false,
                    next_cursor: None,
                }
            }
        };
        let mut catalog = material_catalog(course_id, &modules, &files.items);
        let board_items = match self.learningx_board_materials(course_id, week).await {
            Ok(items) => items,
            Err(error) => {
                warnings.push(format!("LearningX board materials unavailable: {error}"));
                Vec::new()
            }
        };
        let mut seen: HashSet<String> = catalog
            .iter()
            .map(|material| material.resource_id.clone())
            .collect();
        for material in board_items {
            if seen.insert(material.resource_id.clone()) {
                catalog.push(material);
            }
        }
        Ok(MaterialView {
            catalog,
            modules,
            complete: files.complete && warnings.is_empty(),
            warnings,
            notes,
        })
    }

    /// Unified Commons/Canvas material list with filters and pagination.
    pub async fn materials(
        &self,
        course_id: u32,
        week: Option<u32>,
        query: Option<&str>,
        content_type: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<Value, EclassError> {
        if !(1..=100).contains(&limit) {
            return Err(EclassError::InvalidArgument(
                "limit must be between 1 and 100".to_string(),
            ));
        }
        let view = self.material_view(course_id, week).await?;
        let items = filter_materials(&view.catalog, week, query, content_type);
        let total = items.len();
        let next = (offset + limit < total).then(|| json!(offset + limit));
        let page = items
            .iter()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        let catalog_week = view
            .catalog
            .iter()
            .flat_map(|material| {
                material
                    .weeks
                    .clone()
                    .unwrap_or_else(|| material.week.into_iter().collect())
            })
            .max();
        // Modules and board titles are already in the catalog; no extra pass.
        let current_week = module_weeks(&view.modules)
            .last()
            .copied()
            .max(catalog_week);
        Ok(json!({
            "items": page,
            "total": total,
            "complete": view.complete && next.is_none(),
            "has_more": next.is_some(),
            "next": next,
            "next_param": next.is_some().then_some("offset"),
            "current_week": current_week,
            "sources": ["commons", "canvas_file", "learningx_board"],
            "warnings": view.warnings,
            "notes": view.notes
        }))
    }

    async fn resolve_material(
        &self,
        resource_id: &str,
    ) -> Result<(Material, (Option<String>, Option<String>)), EclassError> {
        let resource = parse_resource_id(resource_id)?;
        if resource.source == "canvas" {
            let source_id = resource.source_id().ok_or(EclassError::MaterialNotFound)?;
            let file = self
                .get(
                    &format!("/api/v1/courses/{}/files/{source_id}", resource.course_id),
                    false,
                    false,
                )
                .await?;
            if file.get("id").and_then(positive_u64) != Some(u64::from(source_id)) {
                return Err(EclassError::MaterialNotFound);
            }
            let catalog = material_catalog(resource.course_id, &[], std::slice::from_ref(&file));
            let material = catalog
                .into_iter()
                .next()
                .ok_or(EclassError::MaterialNotFound)?;
            if !material.available {
                return Err(EclassError::MaterialUnavailable);
            }
            let revision = (
                file.get("updated_at")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                file.get("created_at")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            );
            return Ok((material, revision));
        }
        if resource.source == "board" {
            let board_id = resource.parts[0];
            let post_id = resource.parts[1];
            let attachment_id = resource.parts[2];
            let mut post = self
                .get(
                    &format!(
                        "/learningx/api/v1/learningx_board/courses/{}/boards/{board_id}/posts/{post_id}",
                        resource.course_id
                    ),
                    true,
                    false,
                )
                .await?;
            if let Some(object) = post.as_object_mut() {
                object.insert("board_id".to_string(), json!(board_id));
            }
            let material = board_materials(resource.course_id, &[post])
                .into_iter()
                .find(|item| item.attachment_id == Some(attachment_id))
                .ok_or(EclassError::MaterialNotFound)?;
            if !material.available {
                return Err(EclassError::MaterialUnavailable);
            }
            let revision = (material.updated_at.clone(), material.created_at.clone());
            return Ok((material, revision));
        }
        let source_id = resource.source_id().ok_or(EclassError::MaterialNotFound)?;
        let modules = self.modules(resource.course_id).await?;
        let catalog = material_catalog(resource.course_id, &modules, &[]);
        let material = catalog
            .into_iter()
            .find(|item| {
                item.source == "commons"
                    && item.resource_id == format!("commons:{}:{source_id}", resource.course_id)
            })
            .ok_or(EclassError::MaterialNotFound)?;
        if !material.available {
            return Err(EclassError::MaterialUnavailable);
        }
        let revision = (material.updated_at.clone(), material.created_at.clone());
        Ok((material, revision))
    }

    /// Download and verify a material, reusing a verified local file when possible.
    pub async fn material_download(&self, resource_id: &str) -> Result<Value, EclassError> {
        let _guard = self.material_lock.lock().await;
        let (material, revision) = self.resolve_material(resource_id).await?;
        if !material.download_supported {
            return Err(EclassError::UnsupportedMaterial);
        }
        if let Some(cached) = self.material_cache.lock().await.get(resource_id)
            && cached.revision == revision
            && let Some(path) = cached.result.get("path").and_then(Value::as_str)
            && let Some(sha256) = cached.result.get("sha256").and_then(Value::as_str)
            && Path::new(path).is_file()
            && file_sha256(Path::new(path)).is_ok_and(|value| value == sha256)
        {
            return Ok(cached.result.clone());
        }
        let resource = parse_resource_id(resource_id)?;
        let course_id = resource.course_id;
        let source_id = resource.source_id().ok_or(EclassError::MaterialNotFound)?;
        let (mut result, destination) = if material.source == "canvas" {
            let mut value = self.download_attachment(course_id, source_id).await?;
            let destination = value.get("path").and_then(Value::as_str).map(PathBuf::from);
            if let Some(object) = value.as_object_mut() {
                object.insert("source".to_string(), json!("canvas_file"));
                object.insert("original".to_string(), json!(true));
            }
            (value, destination)
        } else if material.source == "learningx_board" {
            let url = material
                .download_url
                .clone()
                .ok_or(EclassError::MaterialUnavailable)?;
            std::fs::create_dir_all(&self.output_dir)?;
            let extension = Path::new(&material.title)
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("bin");
            let stem = Path::new(&safe_filename(&material.title))
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("material")
                .to_string();
            let path = self.output_dir.join(format!(
                "{stem}-{}.{}",
                unique_suffix(),
                safe_filename(extension)
            ));
            let downloaded = self
                .transport
                .download_url(&url, &path, 64 * 1024 * 1024, None)
                .await;
            let downloaded = match downloaded {
                Ok(downloaded) => downloaded,
                Err(error) => {
                    let _ = std::fs::remove_file(&path);
                    return Err(error);
                }
            };
            let value = json!({
                "path": path,
                "bytes": downloaded.bytes,
                "sha256": file_sha256(&path)?,
                "content_type": downloaded.content_type,
                "source": "learningx_board",
                "original": true,
                "board_id": material.board_id,
                "post_id": material.post_id,
                "attachment_id": material.attachment_id
            });
            (value, Some(path))
        } else {
            std::fs::create_dir_all(&self.output_dir)?;
            let path = self.output_dir.join(format!(
                "material-{course_id}-{source_id}-{}.pdf",
                unique_suffix()
            ));
            let native = self
                .transport
                .commons_pdf(course_id, source_id, &path, 64 * 1024 * 1024)
                .await?;
            if native.get("available").and_then(Value::as_bool) != Some(true) {
                let _ = std::fs::remove_file(&path);
                return Err(EclassError::PdfUnavailable);
            }
            let mut value = native;
            if let Some(object) = value.as_object_mut() {
                object.insert("path".to_string(), json!(path));
                object.insert("source".to_string(), json!("commons_original"));
                object.insert("original".to_string(), json!(true));
            }
            (value, Some(path))
        };
        if material.content_type == "pdf" {
            let Some(path) = destination.as_deref() else {
                return Err(EclassError::InvalidPdf);
            };
            let info = match inspect_pdf(path) {
                Ok(info) => info,
                Err(error) => {
                    let _ = std::fs::remove_file(path);
                    return Err(error);
                }
            };
            merge_object(
                &mut result,
                &json!({
                    "bytes": info.bytes,
                    "sha256": info.sha256,
                    "page_count": info.page_count,
                    "content_type": info.content_type
                }),
            );
        }
        merge_object(
            &mut result,
            &json!({
                "resource_id": resource_id,
                "course_id": material.course_id,
                "title": material.title,
                "attendance_recorded": false
            }),
        );
        self.material_cache.lock().await.insert(
            resource_id.to_string(),
            CachedMaterial {
                revision,
                result: result.clone(),
            },
        );
        Ok(result)
    }

    /// Download a PDF and return resumable page text.
    pub async fn material_read(
        &self,
        resource_id: &str,
        start_page: u32,
        end_page: Option<u32>,
        start_offset: usize,
        max_chars: usize,
    ) -> Result<Value, EclassError> {
        let (material, _) = self.resolve_material(resource_id).await?;
        if !material.text_supported {
            return Err(EclassError::UnsupportedMaterial);
        }
        let downloaded = self.material_download(resource_id).await?;
        let path = downloaded
            .get("path")
            .and_then(Value::as_str)
            .ok_or(EclassError::InvalidPdf)?;
        let read = read_pdf(
            Path::new(path),
            start_page,
            end_page,
            start_offset,
            max_chars,
        )?;
        let mut result = downloaded;
        merge_object(&mut result, &serde_json::to_value(read)?);
        Ok(result)
    }

    /// Announcements in an explicit date range.
    pub async fn announcements(
        &self,
        course_id: u32,
        start: &str,
        end: &str,
    ) -> Result<Value, EclassError> {
        let result = announcements_on(&self.transport, u64::from(course_id), start, end).await?;
        let truncated = result.next_cursor.is_some();
        let mut payload = list_response(result.items, result.complete && !truncated, None, None);
        if truncated && let Some(object) = payload.as_object_mut() {
            object.insert(
                "warnings".to_string(),
                json!(["more announcements exist beyond the page limit; narrow the date range"]),
            );
        }
        Ok(payload)
    }

    /// Announcement/discussion detail.
    pub async fn announcement(&self, course_id: u32, topic_id: u32) -> Result<Value, EclassError> {
        self.get(
            &format!("/api/v1/courses/{course_id}/discussion_topics/{topic_id}"),
            false,
            false,
        )
        .await
    }

    /// Assignments with the current user's submission.
    ///
    /// The default view is compact so MCP clients do not receive full Canvas
    /// HTML and verifier-bearing URLs on every call.
    pub async fn assignments(&self, course_id: u32, detail: bool) -> Result<Paged, EclassError> {
        let paged = self
            .paged(
                &format!(
                    "/api/v1/courses/{course_id}/assignments?include[]=submission&per_page=100"
                ),
                false,
                20,
            )
            .await?;
        if detail {
            return Ok(paged);
        }
        let items = paged
            .items
            .iter()
            .map(compact_assignment)
            .collect::<Vec<_>>();
        Ok(Paged {
            items,
            complete: paged.complete,
            next_cursor: paged.next_cursor,
        })
    }

    /// Assignment detail.
    pub async fn assignment(
        &self,
        course_id: u32,
        assignment_id: u32,
    ) -> Result<Value, EclassError> {
        let mut value = self
            .get(
                &format!(
                    "/api/v1/courses/{course_id}/assignments/{assignment_id}?include[]=submission"
                ),
                false,
                false,
            )
            .await?;
        // Detail responses keep the raw shape but must not lose the KST view
        // that compact list responses already provide.
        if let Some(object) = value.as_object_mut() {
            for (key, value) in date_fields(&Value::Object(object.clone())) {
                object.insert(key, value);
            }
        }
        Ok(value)
    }

    /// Current-user submission.
    pub async fn submission(
        &self,
        course_id: u32,
        assignment_id: u32,
    ) -> Result<Value, EclassError> {
        let mut value = self
            .get(
                &format!(
                    "/api/v1/courses/{course_id}/assignments/{assignment_id}/submissions/self"
                ),
                false,
                false,
            )
            .await?;
        if let Some(object) = value.as_object_mut()
            && let Some(raw) = object.get("submitted_at").and_then(Value::as_str)
            && let Ok(date) = DateTime::parse_from_rfc3339(raw)
        {
            object.insert(
                "submitted_at_seoul".to_string(),
                json!(
                    date.with_timezone(&Seoul)
                        .to_rfc3339_opts(SecondsFormat::AutoSi, false)
                ),
            );
        }
        Ok(value)
    }

    /// Assignment and LearningX deadlines.
    pub async fn deadlines(&self, course_id: u32) -> Result<Value, EclassError> {
        let assignments = self.assignments(course_id, true).await?;
        let mut items = Vec::new();
        for assignment in &assignments.items {
            let mut item = Map::new();
            item.insert("kind".to_string(), json!("assignment"));
            item.insert(
                "id".to_string(),
                assignment.get("id").cloned().unwrap_or(Value::Null),
            );
            item.insert(
                "title".to_string(),
                assignment.get("name").cloned().unwrap_or(Value::Null),
            );
            item.extend(date_fields(assignment));
            item.insert(
                "locked_for_user".to_string(),
                assignment
                    .get("locked_for_user")
                    .cloned()
                    .unwrap_or(Value::Null),
            );
            items.push(Value::Object(item));
        }
        let mut warnings = Vec::new();
        match self.modules(course_id).await {
            Ok(modules) => {
                for module in modules {
                    for entry in module
                        .get("module_items")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        let Some(data) = entry.get("content_data").and_then(Value::as_object)
                        else {
                            continue;
                        };
                        if entry.get("content_type").and_then(Value::as_str)
                            != Some("attendance_item")
                        {
                            continue;
                        }
                        if !["due_at", "lock_at", "late_at"]
                            .iter()
                            .any(|key| data.get(*key).is_some_and(|value| !value.is_null()))
                        {
                            continue;
                        }
                        let mut item = Map::new();
                        item.insert("kind".to_string(), json!("learning_item"));
                        item.insert(
                            "id".to_string(),
                            data.get("item_id").cloned().unwrap_or(Value::Null),
                        );
                        item.insert(
                            "title".to_string(),
                            entry.get("title").cloned().unwrap_or(Value::Null),
                        );
                        item.insert(
                            "use_attendance".to_string(),
                            data.get("use_attendance").cloned().unwrap_or(Value::Null),
                        );
                        item.extend(date_fields(&Value::Object(data.clone())));
                        items.push(Value::Object(item));
                    }
                }
            }
            Err(error) => warnings.push(error.to_string()),
        }
        Ok(json!({
            "items": items,
            "complete": assignments.complete && warnings.is_empty(),
            "warnings": warnings,
            "next_cursor": assignments.next_cursor
        }))
    }

    /// Syllabus text and external viewer links.
    pub async fn syllabus(&self, course_id: u32) -> Result<Value, EclassError> {
        let course = self
            .get(
                &format!("/api/v1/courses/{course_id}?include[]=syllabus_body"),
                false,
                false,
            )
            .await?;
        let html = course
            .get("syllabus_body")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let text = html_text(html);
        let mut viewers = iframe_sources(html);
        if viewers.is_empty()
            && let Ok(page) = self
                .get(
                    &format!("/courses/{course_id}/assignments/syllabus"),
                    false,
                    true,
                )
                .await
        {
            let page = page.as_str().unwrap_or_default();
            viewers = iframe_sources(page)
                .into_iter()
                .filter(|url| url.contains("rpt80.cau.ac.kr"))
                .collect();
        }
        let available = !text.is_empty();
        let has_viewers = !viewers.is_empty();
        let mut notes = Vec::new();
        let mut warnings = Vec::new();
        if !available && has_viewers {
            notes.push(
                "The syllabus is an OZ viewer document; its text is not embedded in the Canvas page."
                    .to_string(),
            );
            warnings.push("External viewer document content is not extracted".to_string());
        }
        if !available && !has_viewers {
            notes.push("No syllabus was published for this course.".to_string());
        }
        let suggested_tool = (!available && has_viewers).then(|| {
            json!({
                "tool": "download_syllabus_pdf",
                "arguments": {"course_id": course_id},
                "requires": "optional eclass-oz-export helper (Python `oz` extra with Playwright)"
            })
        });
        Ok(json!({
            "course_id": course_id,
            "text": text,
            "available": available,
            "viewer_urls": viewers,
            "suggested_tool": suggested_tool,
            "complete": available,
            "warnings": warnings,
            "notes": notes
        }))
    }

    /// Export an OZ syllabus through the optional Python/Playwright helper.
    ///
    /// The helper receives only the course ID, output directory and session
    /// file. It returns the same PDF/text shape as the Python reference.
    pub async fn syllabus_document(
        &self,
        course_id: u32,
        extract_text: bool,
    ) -> Result<Value, EclassError> {
        std::fs::create_dir_all(&self.output_dir)?;
        let mut command = tokio::process::Command::new(&self.oz_helper);
        command
            .arg("--course-id")
            .arg(course_id.to_string())
            .arg("--output-dir")
            .arg(&self.output_dir)
            .arg("--session-file")
            .arg(self.transport.session_file())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if !extract_text {
            command.arg("--no-extract-text");
        }
        let output = match tokio::time::timeout(Duration::from_secs(180), command.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(EclassError::OzHelper(
                    "install the optional `eclass-oz-export` helper (Python package with the `oz` extra) or set ECLASS_OZ_HELPER"
                        .to_string(),
                ));
            }
            Ok(Err(_)) => {
                return Err(EclassError::OzHelper(
                    "could not start the OZ PDF helper".to_string(),
                ));
            }
            Err(_) => {
                return Err(EclassError::OzHelper(
                    "OZ PDF export timed out after 180 seconds".to_string(),
                ));
            }
        };
        if !output.status.success() {
            let message = String::from_utf8_lossy(&output.stderr)
                .replace('\n', " ")
                .trim()
                .chars()
                .take(300)
                .collect::<String>();
            return Err(EclassError::OzHelper(if message.is_empty() {
                "OZ PDF helper exited without a message".to_string()
            } else {
                message
            }));
        }
        if output.stdout.len() > 16 * 1024 * 1024 {
            return Err(EclassError::OzHelper(
                "OZ PDF helper response exceeded the read limit".to_string(),
            ));
        }
        let mut value: Value = serde_json::from_slice(&output.stdout).map_err(|_| {
            EclassError::OzHelper("OZ PDF helper returned invalid JSON".to_string())
        })?;
        let path = value
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| EclassError::OzHelper("OZ PDF helper returned no path".to_string()))?;
        let output_root = self
            .output_dir
            .canonicalize()
            .map_err(|_| EclassError::OzHelper("output directory is unavailable".to_string()))?;
        let path = Path::new(path)
            .canonicalize()
            .map_err(|_| EclassError::OzHelper("OZ PDF output was not created".to_string()))?;
        if !path.starts_with(&output_root) {
            return Err(EclassError::OzHelper(
                "OZ PDF helper returned a path outside the download directory".to_string(),
            ));
        }
        let metadata = std::fs::metadata(&path)?;
        if metadata.len() == 0 || metadata.len() > 64 * 1024 * 1024 {
            return Err(EclassError::OzHelper(
                "OZ PDF is empty or exceeds 64 MiB".to_string(),
            ));
        }
        if value.get("bytes").and_then(Value::as_u64) != Some(metadata.len()) {
            return Err(EclassError::OzHelper(
                "OZ PDF helper size did not match the file".to_string(),
            ));
        }
        let header = std::fs::read(&path)?
            .into_iter()
            .take(5)
            .collect::<Vec<_>>();
        if header != b"%PDF-" {
            return Err(EclassError::OzHelper(
                "OZ export did not return a valid PDF".to_string(),
            ));
        }
        value["course_id"] = json!(course_id);
        Ok(value)
    }

    /// LearningX boards.
    pub async fn boards(&self, course_id: u32) -> Result<Value, EclassError> {
        let body = self
            .get(
                &format!("/learningx/api/v1/learningx_board/courses/{course_id}/boards"),
                true,
                false,
            )
            .await?;
        let complete = body.is_array();
        let items: Vec<Value> = body
            .as_array()
            .map(|boards| boards.iter().map(compact_board).collect())
            .unwrap_or_default();
        Ok(json!({
            "items": items,
            "total": items.len(),
            "complete": complete,
            "has_more": false,
            "next": null,
            "warnings": if complete { Vec::<String>::new() } else { vec!["provider returned an unexpected board payload".to_string()] }
        }))
    }

    /// One LearningX board page.
    pub async fn board_posts(
        &self,
        course_id: u32,
        board_id: u32,
        page: u32,
    ) -> Result<Value, EclassError> {
        let mut body = self
            .get(
                &format!("/learningx/api/v1/learningx_board/courses/{course_id}/boards/{board_id}/posts?page={page}"),
                true,
                false,
            )
            .await?;
        if let Some(posts) = body.get_mut("items").and_then(Value::as_array_mut) {
            for post in posts {
                add_seoul_timestamps(post);
            }
        }
        let pagination = body.get("pagination").and_then(Value::as_object);
        let current = pagination
            .and_then(|value| value.get("current_page"))
            .and_then(Value::as_u64);
        let last = pagination
            .and_then(|value| value.get("last_page"))
            .and_then(Value::as_u64);
        let known = current.is_some() && last.is_some();
        let next = current
            .zip(last)
            .filter(|(current, last)| current < last)
            .map(|(current, _)| json!(current + 1));
        let items = body
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut payload = list_response(items, known && next.is_none(), next, Some("page"));
        if let Some(object) = payload.as_object_mut() {
            object.insert("page".to_string(), json!(page));
            object.insert(
                "pagination".to_string(),
                pagination
                    .cloned()
                    .map(Value::Object)
                    .unwrap_or(Value::Null),
            );
            if !known {
                object.insert(
                    "warnings".to_string(),
                    json!(["provider pagination could not be established"]),
                );
            }
        }
        Ok(payload)
    }

    /// LearningX post detail.
    pub async fn board_post(
        &self,
        course_id: u32,
        board_id: u32,
        post_id: u32,
    ) -> Result<Value, EclassError> {
        let mut value = self
            .get(
                &format!("/learningx/api/v1/learningx_board/courses/{course_id}/boards/{board_id}/posts/{post_id}"),
                true,
                false,
            )
            .await?;
        add_seoul_timestamps(&mut value);
        Ok(value)
    }

    /// Current-user groups.
    pub async fn groups(&self) -> Result<Value, EclassError> {
        let result = self
            .paged("/api/v1/users/self/groups?per_page=100", false, 20)
            .await?;
        Ok(list_response(result.items, result.complete, None, None))
    }

    /// Accessible discussions/files for a verified current-user group.
    pub async fn group_activity(&self, group_id: u32) -> Result<Value, EclassError> {
        let groups = self.groups().await?;
        if !groups
            .get("items")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items.iter().any(|group| {
                    group.get("id").and_then(positive_u64) == Some(u64::from(group_id))
                })
            })
        {
            return Err(EclassError::MaterialNotFound);
        }
        let discussions = self
            .paged(
                &format!("/api/v1/groups/{group_id}/discussion_topics?per_page=100"),
                false,
                20,
            )
            .await?;
        let files = self
            .paged(
                &format!("/api/v1/groups/{group_id}/files?per_page=100"),
                false,
                20,
            )
            .await?;
        Ok(json!({
            "discussions": list_response(discussions.items, discussions.complete, None, None),
            "files": list_response(files.items, files.complete, None, None)
        }))
    }

    /// Video metadata without playback or progress.
    pub async fn video_info(&self, course_id: u32) -> Result<Value, EclassError> {
        let modules = self.modules(course_id).await?;
        let mut items = Vec::new();
        for module in modules {
            for item in module
                .get("module_items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(data) = item.get("content_data").and_then(Value::as_object) else {
                    continue;
                };
                let Some(media) = data.get("item_content_data").and_then(Value::as_object) else {
                    continue;
                };
                let content_type = media
                    .get("content_type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !["mp4", "everlec", "movie360", "oncast", "zoom"].contains(&content_type) {
                    continue;
                }
                let mut value = Map::new();
                value.insert(
                    "title".to_string(),
                    item.get("title").cloned().unwrap_or(Value::Null),
                );
                value.insert(
                    "item_id".to_string(),
                    data.get("item_id").cloned().unwrap_or(Value::Null),
                );
                value.insert("content_type".to_string(), json!(content_type));
                value.insert(
                    "duration".to_string(),
                    media.get("duration").cloned().unwrap_or(Value::Null),
                );
                value.insert(
                    "has_view_url".to_string(),
                    json!(
                        media
                            .get("view_url")
                            .and_then(Value::as_str)
                            .is_some_and(|value| !value.trim().is_empty())
                    ),
                );
                value.insert(
                    "download_candidate".to_string(),
                    json!(
                        media
                            .get("content_id")
                            .is_some_and(|value| value != "not_open")
                            && data.get("opened").and_then(Value::as_bool) != Some(false)
                            && content_type == "mp4"
                    ),
                );
                value.insert(
                    "reason".to_string(),
                    json!("Original availability is checked by download_video_original; not_open items cannot be downloaded"),
                );
                value.extend(date_fields(&Value::Object(data.clone())));
                items.push(Value::Object(value));
            }
        }
        Ok(json!({"items": items, "complete": true}))
    }

    /// Download a Commons original MP4.
    pub async fn download_video(
        &self,
        course_id: u32,
        item_id: u32,
        allow_playback_copy: bool,
    ) -> Result<Value, EclassError> {
        std::fs::create_dir_all(&self.output_dir)?;
        let path = self.output_dir.join(format!(
            "video-{course_id}-{item_id}-{}.mp4",
            unique_suffix()
        ));
        let result = self
            .transport
            .video_original(course_id, item_id, Some(&path), allow_playback_copy)
            .await;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                let _ = std::fs::remove_file(&path);
                return Err(error);
            }
        };
        if result.get("available").and_then(Value::as_bool) != Some(true) {
            let _ = std::fs::remove_file(&path);
            return Err(EclassError::VideoUnavailable);
        }
        let size = std::fs::metadata(&path)?.len();
        let mut prefix = [0_u8; 12];
        use std::io::Read;
        let read = std::fs::File::open(&path)?.read(&mut prefix)?;
        let valid = read >= 12 && &prefix[4..8] == b"ftyp";
        if !valid {
            let _ = std::fs::remove_file(&path);
            return Err(EclassError::UnsupportedVideo);
        }
        let sha256 = file_sha256(&path)?;
        let source = result
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or("commons_original");
        Ok(json!({
            "path": path,
            "bytes": size,
            "sha256": sha256,
            "content_type": "video/mp4",
            "source": source,
            "original": source == "commons_original",
            "course_id": course_id,
            "item_id": item_id,
            "attendance_recorded": false
        }))
    }

    /// Download a Canvas file by file ID.
    pub async fn download_attachment(
        &self,
        course_id: u32,
        file_id: u32,
    ) -> Result<Value, EclassError> {
        let meta = self
            .get(
                &format!("/api/v1/courses/{course_id}/files/{file_id}"),
                false,
                false,
            )
            .await?;
        if meta.get("locked_for_user").and_then(Value::as_bool) == Some(true) {
            return Err(EclassError::MaterialUnavailable);
        }
        std::fs::create_dir_all(&self.output_dir)?;
        let name = meta
            .get("display_name")
            .or_else(|| meta.get("filename"))
            .and_then(Value::as_str)
            .map(safe_filename)
            .unwrap_or_else(|| format!("file-{file_id}"));
        let destination = self
            .output_dir
            .join(format!("{file_id}-{}-{}", unique_suffix(), name));
        let result = self
            .transport
            .download(
                &format!("/courses/{course_id}/files/{file_id}/download?wrap=1"),
                &destination,
                64 * 1024 * 1024,
            )
            .await;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                let _ = std::fs::remove_file(&destination);
                return Err(error);
            }
        };
        let size = std::fs::metadata(&destination)?.len();
        let mut prefix = vec![0_u8; 512];
        use std::io::Read;
        let read = std::fs::File::open(&destination)?.read(&mut prefix)?;
        prefix.truncate(read);
        let lower = prefix
            .iter()
            .map(u8::to_ascii_lowercase)
            .collect::<Vec<_>>();
        if size == 0
            || lower.starts_with(b"<!doctype html")
            || lower.starts_with(b"<html")
            || meta
                .get("size")
                .and_then(Value::as_u64)
                .is_some_and(|expected| expected != size)
        {
            let _ = std::fs::remove_file(&destination);
            return Err(EclassError::NotAFile);
        }
        Ok(json!({
            "path": destination,
            "bytes": size,
            "sha256": file_sha256(&destination)?,
            "content_type": result.content_type,
            "file_id": file_id
        }))
    }

    /// Personal file folders. Submission uploads live under `제출물`.
    pub async fn my_folders(&self) -> Result<Value, EclassError> {
        let result = self
            .paged("/api/v1/users/self/folders?per_page=100", false, 5)
            .await?;
        let items = result
            .items
            .iter()
            .map(|folder| {
                json!({
                    "folder_id": folder.get("id").and_then(positive_u64)
                        .map_or(Value::Null, |id| json!(id)),
                    "name": folder.get("name").cloned().unwrap_or(Value::Null),
                    "files_count": folder.get("files_count").cloned().unwrap_or(Value::Null),
                    "parent_folder_id": folder.get("parent_folder_id").and_then(positive_u64)
                        .map_or(Value::Null, |id| json!(id)),
                    "created_at": folder.get("created_at").cloned().unwrap_or(Value::Null),
                })
            })
            .collect::<Vec<_>>();
        let mut payload = list_response(items, result.complete, None, None);
        if let Some(object) = payload.as_object_mut() {
            object.insert(
                "total".to_string(),
                json!(
                    object
                        .get("items")
                        .and_then(Value::as_array)
                        .map_or(0, Vec::len)
                ),
            );
        }
        Ok(payload)
    }

    /// Files in the user's own storage, newest first.
    pub async fn my_files(
        &self,
        folder_id: Option<u32>,
        query: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<Value, EclassError> {
        if !(1..=100).contains(&limit) {
            return Err(EclassError::InvalidArgument(
                "limit must be between 1 and 100".to_string(),
            ));
        }
        let path = folder_id.map_or_else(
            || "/api/v1/users/self/files?per_page=100".to_string(),
            |folder_id| format!("/api/v1/folders/{folder_id}/files?per_page=100"),
        );
        let result = self.paged(&path, false, 10).await?;
        let search = normalized(query);
        let mut items = result
            .items
            .iter()
            .filter(|file| {
                search.is_empty()
                    || normalized(file.get("display_name").and_then(Value::as_str))
                        .contains(&search)
            })
            .map(compact_file)
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            right
                .get("created_at")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .cmp(
                    left.get("created_at")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                )
        });
        let total = items.len();
        let next = (offset + limit < total).then(|| json!(offset + limit));
        let page = items
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect::<Vec<_>>();
        Ok(json!({
            "items": page,
            "total": total,
            "folder_id": folder_id,
            "complete": result.complete && next.is_none(),
            "has_more": next.is_some(),
            "next": next,
            "next_param": next.is_some().then_some("offset")
        }))
    }

    /// True when the folder or one of its ancestors is the `제출물` folder.
    async fn folder_is_submission(&self, folder_id: u32) -> Result<bool, EclassError> {
        let mut current = Some(folder_id);
        for _ in 0..5 {
            let Some(id) = current else {
                return Ok(false);
            };
            let folder = self
                .get(&format!("/api/v1/folders/{id}"), false, false)
                .await?;
            if folder.get("name").and_then(Value::as_str) == Some("제출물") {
                return Ok(true);
            }
            current = folder
                .get("parent_folder_id")
                .and_then(positive_u64)
                .and_then(|value| u32::try_from(value).ok());
        }
        Ok(false)
    }

    /// Preview or delete one file owned by the current user.
    ///
    /// `dry_run` is always allowed and never touches the server. A real delete
    /// needs `ECLASS_ENABLE_DELETE=1`, the exact confirmation string, and an
    /// extra opt-in when the file sits in a submission folder.
    pub async fn delete_file(
        &self,
        file_id: u32,
        confirmation: &str,
        dry_run: bool,
        allow_submission_folder: bool,
    ) -> Result<Value, EclassError> {
        let file = self
            .get(&format!("/api/v1/files/{file_id}"), false, false)
            .await?;
        let folder_id = file
            .get("folder_id")
            .and_then(positive_u64)
            .and_then(|value| u32::try_from(value).ok());
        let submission_folder = match folder_id {
            Some(folder_id) => self.folder_is_submission(folder_id).await?,
            None => false,
        };
        let preview = json!({
            "file_id": file_id,
            "name": file.get("display_name").or_else(|| file.get("filename")).cloned().unwrap_or(Value::Null),
            "bytes": file.get("size").cloned().unwrap_or(Value::Null),
            "folder_id": folder_id,
            "created_at": file.get("created_at").cloned().unwrap_or(Value::Null),
            "submission_folder": submission_folder
        });
        if dry_run {
            return Ok(json!({
                "dry_run": true,
                "deleted": false,
                "file": preview,
                "next": {
                    "confirm": format!("DELETE {file_id}"),
                    "dry_run": false,
                    "allow_submission_folder": submission_folder
                },
                "warnings": if submission_folder {
                    vec!["this file is in a submission folder; deleting it can remove it from a submitted assignment"]
                } else {
                    Vec::<&str>::new()
                }
            }));
        }
        if !self.enable_delete {
            return Err(EclassError::DeleteDisabled);
        }
        if confirmation != format!("DELETE {file_id}") {
            return Err(EclassError::DeleteConfirmation(u64::from(file_id)));
        }
        if submission_folder && !allow_submission_folder {
            return Err(EclassError::SubmissionFolder);
        }
        let deleted = self.transport.delete_file(file_id).await?;
        self.invalidate().await;
        Ok(json!({
            "dry_run": false,
            "deleted": true,
            "file": preview,
            "workflow_state": deleted.get("workflow_state").cloned().unwrap_or(Value::Null)
        }))
    }

    fn fingerprint(path: &str) -> Result<FileFingerprint, EclassError> {
        let path = expand_home(path);
        if !path.is_file() || std::fs::metadata(&path)?.len() > 10 * 1024 * 1024 {
            return Err(EclassError::BrowserOnly(
                "each submission file must exist and be at most 10 MiB",
            ));
        }
        let bytes = std::fs::read(&path)?;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        Ok(FileFingerprint {
            path: path.to_string_lossy().to_string(),
            name: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("file")
                .to_string(),
            size: u64::try_from(bytes.len()).map_err(|_| EclassError::InvalidPageRange)?,
            sha256: hex::encode(hasher.finalize()),
        })
    }

    /// Validate local files and the assignment, returning a short-lived plan.
    pub async fn prepare_submission(
        &self,
        course_id: u32,
        assignment_id: u32,
        files: &[String],
    ) -> Result<Value, EclassError> {
        let assignment = self.assignment(course_id, assignment_id).await?;
        let status = self.session_status().await?;
        let user_id = status
            .get("user")
            .and_then(|user| user.get("id"))
            .and_then(positive_u64)
            .ok_or(EclassError::NoProfile)?;
        let existing = self.submission(course_id, assignment_id).await?;
        let manifest = files
            .iter()
            .map(|path| Self::fingerprint(path))
            .collect::<Result<Vec<_>, _>>()?;
        let mut problems = Vec::new();
        if manifest.is_empty() {
            problems.push("At least one file is required".to_string());
        }
        if !assignment
            .get("submission_types")
            .and_then(Value::as_array)
            .is_some_and(|types| {
                types
                    .iter()
                    .any(|value| value.as_str() == Some("online_upload"))
            })
        {
            problems.push("Assignment does not allow file upload".to_string());
        }
        if assignment.get("locked_for_user").and_then(Value::as_bool) == Some(true) {
            problems.push("Assignment is locked".to_string());
        }
        if let Some(allowed) = assignment
            .get("allowed_extensions")
            .and_then(Value::as_array)
        {
            let allowed: Vec<String> = allowed
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_ascii_lowercase)
                .collect();
            if !allowed.is_empty()
                && manifest.iter().any(|file| {
                    !allowed.contains(
                        &Path::new(&file.name)
                            .extension()
                            .and_then(|value| value.to_str())
                            .unwrap_or_default()
                            .to_ascii_lowercase(),
                    )
                })
            {
                problems.push("File extension is not allowed".to_string());
            }
        }
        if assignment
            .get("group_category_id")
            .is_some_and(|value| !value.is_null())
        {
            problems.push("Group submissions require a separately verified workflow".to_string());
        }
        if assignment.get("turnitin_enabled").and_then(Value::as_bool) == Some(true)
            || assignment.get("vericite_enabled").and_then(Value::as_bool) == Some(true)
        {
            problems.push(
                "External originality agreement requires a separately verified workflow"
                    .to_string(),
            );
        }
        let attempts = assignment
            .get("allowed_attempts")
            .and_then(Value::as_i64)
            .unwrap_or(-1);
        let previous_attempt = existing.get("attempt").and_then(Value::as_i64);
        if attempts >= 0 && previous_attempt.unwrap_or(0) >= attempts {
            problems.push("No submission attempts remain".to_string());
        }
        if manifest.len() > 10
            || manifest.iter().map(|file| file.size).sum::<u64>() > 20 * 1024 * 1024
        {
            problems.push("Submission limit: 10 files and 20 MiB total".to_string());
        }
        let now = Utc::now();
        let mut plans = self.plans.lock().await;
        plans.retain(|_, plan| plan.expires_at > now);
        if plans.len() >= 100 {
            return Err(EclassError::TooManyPlans);
        }
        let plan_id = unique_suffix();
        let mut plan = SubmissionPlan {
            plan_id: plan_id.clone(),
            course_id,
            assignment_id,
            assignment_name: assignment
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Assignment")
                .to_string(),
            user_id,
            files: manifest,
            previous_attempt,
            previous_submitted_at: existing
                .get("submitted_at")
                .and_then(Value::as_str)
                .map(str::to_string),
            expires_at: now + chrono::Duration::minutes(10),
            problems,
            ready: false,
            state: "prepared".to_string(),
        };
        plan.ready = plan.problems.is_empty();
        let result = serde_json::to_value(&plan)?;
        plans.insert(plan_id, plan);
        Ok(result)
    }

    /// Upload prepared bytes and verify the resulting submission.
    pub async fn submit_assignment(
        &self,
        plan_id: &str,
        confirmation: &str,
    ) -> Result<Value, EclassError> {
        let _guard = self.submission_lock.lock().await;
        if !self.enable_submission {
            return Err(EclassError::BrowserOnly(
                "actual submission is disabled; no upload or POST was sent",
            ));
        }
        let plan = {
            let plans = self.plans.lock().await;
            plans
                .get(plan_id)
                .cloned()
                .ok_or(EclassError::MaterialNotFound)?
        };
        if plan.state != "prepared" || !plan.ready {
            return Err(EclassError::BrowserOnly(
                "a ready, unused preparation plan is required",
            ));
        }
        if confirmation != format!("SUBMIT {}", plan.assignment_id) {
            return Err(EclassError::BrowserOnly(
                "confirmation must exactly match SUBMIT followed by the assignment ID",
            ));
        }
        if plan.expires_at < Utc::now() {
            return Err(EclassError::BrowserOnly("preparation plan expired"));
        }
        let current = plan
            .files
            .iter()
            .map(|file| Self::fingerprint(&file.path))
            .collect::<Result<Vec<_>, _>>()?;
        if current != plan.files {
            return Err(EclassError::BrowserOnly("files changed after preparation"));
        }
        let paths = plan
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>();
        let fresh = self
            .prepare_submission(plan.course_id, plan.assignment_id, &paths)
            .await?;
        let fresh_files = fresh.get("files").cloned().unwrap_or(Value::Null);
        let fresh_ready = fresh.get("ready").and_then(Value::as_bool) == Some(true);
        let fresh_user = fresh.get("user_id").and_then(Value::as_u64);
        let fresh_attempt = fresh.get("previous_attempt").and_then(Value::as_i64);
        let fresh_submitted = fresh.get("previous_submitted_at").and_then(Value::as_str);
        if !fresh_ready
            || fresh_files != serde_json::to_value(&plan.files)?
            || fresh_user != Some(plan.user_id)
            || fresh_attempt != plan.previous_attempt
            || fresh_submitted != plan.previous_submitted_at.as_deref()
        {
            return Err(EclassError::BrowserOnly(
                "assignment, session, or submission changed; prepare again",
            ));
        }
        let mut payload = Vec::new();
        for file in &plan.files {
            let bytes = std::fs::read(&file.path)?;
            if u64::try_from(bytes.len()).ok() != Some(file.size) {
                return Err(EclassError::BrowserOnly("files changed after preparation"));
            }
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            if hex::encode(hasher.finalize()) != file.sha256 {
                return Err(EclassError::BrowserOnly("files changed after preparation"));
            }
            let content_type = mime_guess::from_path(&file.name)
                .first_or_octet_stream()
                .to_string();
            payload.push((file.name.clone(), bytes, content_type));
        }
        if let Some(stored) = self.plans.lock().await.get_mut(plan_id) {
            stored.state = "in_flight".to_string();
        }
        let result = self
            .transport
            .submit_files(plan.course_id, plan.assignment_id, &payload)
            .await;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                if let Some(stored) = self.plans.lock().await.get_mut(plan_id) {
                    stored.state = "uncertain".to_string();
                }
                return Err(EclassError::SubmissionStep {
                    step: "submit files",
                    message: format!("{error}; inspect get_submission; do not retry this plan"),
                });
            }
        };
        self.invalidate().await;
        let stored = self.submission(plan.course_id, plan.assignment_id).await?;
        let expected: Vec<u64> = result
            .get("file_ids")
            .and_then(Value::as_array)
            .map(|values| values.iter().filter_map(positive_u64).collect())
            .unwrap_or_default();
        let actual: Vec<u64> = stored
            .get("attachments")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.get("id").and_then(positive_u64))
                    .collect()
            })
            .unwrap_or_default();
        if !expected.iter().all(|id| actual.contains(id))
            || stored.get("submitted_at").is_none_or(Value::is_null)
        {
            if let Some(stored_plan) = self.plans.lock().await.get_mut(plan_id) {
                stored_plan.state = "uncertain".to_string();
            }
            return Err(EclassError::BrowserOnly(
                "submission response could not be verified; check server state before retrying",
            ));
        }
        if let Some(stored_plan) = self.plans.lock().await.get_mut(plan_id) {
            stored_plan.state = "verified".to_string();
        }
        Ok(json!({"state": "verified", "submission": stored}))
    }
}

fn expand_home(value: &str) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/") {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join(rest)
    } else {
        PathBuf::from(value)
    }
}

fn action(code: &str, message: &str, command: Option<&str>) -> Value {
    json!({"code": code, "message": message, "command": command})
}

fn is_session_expired(error: &EclassError) -> bool {
    matches!(
        error,
        EclassError::SessionExpired
            | EclassError::CanvasSessionExpired
            | EclassError::LoginRedirect
            | EclassError::Http(401)
    )
}

async fn paged_on(
    transport: &HttpTransport,
    path: &str,
    learning_x: bool,
    max_pages: usize,
) -> Result<Paged, EclassError> {
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    let mut current = path.to_string();
    let pattern = Regex::new(r#"<([^>]+)>;\s*rel="next""#).map_err(|_| EclassError::InvalidPath)?;
    for _ in 0..max_pages {
        if !seen.insert(current.clone()) {
            return Err(EclassError::InvalidPath);
        }
        let response = transport.get(&current, learning_x, false).await?;
        let Some(mut page) = response.body.as_array().cloned() else {
            return Err(EclassError::UnexpectedList);
        };
        items.append(&mut page);
        let Some(link) = response.link else {
            return Ok(Paged {
                items,
                complete: true,
                next_cursor: None,
            });
        };
        let Some(captures) = pattern.captures(&link) else {
            return Ok(Paged {
                items,
                complete: true,
                next_cursor: None,
            });
        };
        let next = captures
            .get(1)
            .map(|value| value.as_str())
            .ok_or(EclassError::InvalidPath)?;
        let url = url::Url::parse(next)?;
        let base = transport.base();
        if url.scheme() != base.scheme()
            || url.host_str() != base.host_str()
            || url.port_or_known_default() != base.port_or_known_default()
        {
            return Err(EclassError::CrossOrigin);
        }
        current = format!(
            "{}{}",
            url.path(),
            url.query()
                .map(|query| format!("?{query}"))
                .unwrap_or_default()
        );
        crate::http::local_path(&current)?;
    }
    Ok(Paged {
        items,
        complete: false,
        next_cursor: Some(current),
    })
}

async fn announcements_on(
    transport: &HttpTransport,
    course_id: u64,
    start: &str,
    end: &str,
) -> Result<Paged, EclassError> {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer
        .append_pair("context_codes[]", &format!("course_{course_id}"))
        .append_pair("start_date", start)
        .append_pair("end_date", end)
        .append_pair("per_page", "100");
    let path = format!("/api/v1/announcements?{}", serializer.finish());
    paged_on(transport, &path, false, 20).await
}

pub(crate) fn default_output_dir() -> PathBuf {
    if let Some(value) = std::env::var_os("ECLASS_DOWNLOAD_DIR") {
        return expand_home(&value.to_string_lossy());
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Downloads/cau-eclass")
}

fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{nanos:x}")
}

fn safe_filename(value: &str) -> String {
    let mut result = String::new();
    for character in value.chars().take(150) {
        if character.is_alphanumeric() || matches!(character, '.' | '(' | ')' | ' ' | '-' | '_') {
            result.push(character);
        } else {
            result.push('_');
        }
    }
    if result.is_empty() {
        "file".to_string()
    } else {
        result
    }
}

fn file_sha256(path: &Path) -> Result<String, EclassError> {
    let mut hasher = Sha256::new();
    let mut file = std::fs::File::open(path)?;
    let mut buffer = [0_u8; 64 * 1024];
    use std::io::Read;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn positive_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.parse::<u64>().ok(),
        _ => None,
    }
}

fn select(value: &Value, keys: &[&str]) -> Value {
    let Some(object) = value.as_object() else {
        return Value::Null;
    };
    let mut selected = Map::new();
    for key in keys {
        if let Some(value) = object.get(*key) {
            selected.insert((*key).to_string(), value.clone());
        }
    }
    Value::Object(selected)
}

/// The one shape every list tool returns.
pub(crate) fn list_response(
    items: Vec<Value>,
    complete: bool,
    next: Option<Value>,
    next_param: Option<&str>,
) -> Value {
    json!({
        "items": items,
        "complete": complete,
        "has_more": next.is_some(),
        "next": next,
        "next_param": next_param.filter(|_| next.is_some())
    })
}

fn compact_course(course: &Value) -> Value {
    let term = course_term_name(course);
    let enrollment_state = course
        .get("enrollments")
        .and_then(Value::as_array)
        .and_then(|enrollments| enrollments.first())
        .and_then(|enrollment| enrollment.get("enrollment_state"));
    json!({
        "course_id": course.get("id").and_then(positive_u64).map_or(Value::Null, |id| json!(id)),
        "name": course.get("name").cloned().unwrap_or(Value::Null),
        "course_code": course.get("course_code").cloned().unwrap_or(Value::Null),
        "term": term.map_or(Value::Null, |term| json!(term)),
        "workflow_state": course.get("workflow_state").cloned().unwrap_or(Value::Null),
        "enrollment_state": enrollment_state.cloned().unwrap_or(Value::Null)
    })
}

fn compact_file(file: &Value) -> Value {
    let mut item = json!({
        "file_id": file.get("id").and_then(positive_u64).map_or(Value::Null, |id| json!(id)),
        "name": file.get("display_name").or_else(|| file.get("filename")).cloned().unwrap_or(Value::Null),
        "bytes": file.get("size").cloned().unwrap_or(Value::Null),
        "content_type": file.get("content-type").or_else(|| file.get("content_type")).cloned().unwrap_or(Value::Null),
        "folder_id": file.get("folder_id").and_then(positive_u64).map_or(Value::Null, |id| json!(id)),
        "created_at": file.get("created_at").cloned().unwrap_or(Value::Null),
        "hidden": file.get("hidden").cloned().unwrap_or(Value::Null)
    });
    add_seoul_timestamps(&mut item);
    item
}

fn compact_board(board: &Value) -> Value {
    json!({
        "board_id": board.get("id").cloned().unwrap_or(Value::Null),
        "title": board.get("title").cloned().unwrap_or(Value::Null),
        "slug": board.get("slug").cloned().unwrap_or(Value::Null),
        "type": board.get("type").cloned().unwrap_or(Value::Null),
        "description": board.get("description").cloned().unwrap_or(Value::Null),
        "total_post_count": board.get("total_post_count").cloned().unwrap_or(Value::Null),
        "latest_post_created_at": board.get("latest_post_created_at").cloned().unwrap_or(Value::Null)
    })
}

fn course_term_name(course: &Value) -> Option<&str> {
    let term = course.get("term")?;
    term.get("name")
        .and_then(Value::as_str)
        .or_else(|| term.as_str())
}

fn course_term_key(course: &Value) -> Option<(i32, u32)> {
    course_term_name(course).and_then(term_key)
}

/// The academic term that contains today, as observed in the course list.
fn current_term(courses: &[Value]) -> Option<((i32, u32), String)> {
    let now = Utc::now().with_timezone(&Seoul);
    let target = term_key_for_month(now.year(), now.month());
    let mut best: Option<((i32, u32), String)> = None;
    for course in courses {
        let Some(name) = course_term_name(course) else {
            continue;
        };
        let Some(key) = term_key(name) else {
            continue;
        };
        if key > target {
            continue;
        }
        if best.as_ref().is_none_or(|(current, _)| key > *current) {
            best = Some((key, name.to_string()));
        }
    }
    best
}

fn module_weeks(modules: &[Value]) -> Vec<u32> {
    let mut weeks: Vec<u32> = modules
        .iter()
        .filter_map(|module| {
            ["week_position", "week", "position"]
                .iter()
                .find_map(|key| module.get(*key).and_then(positive_u64))
                .and_then(|value| u32::try_from(value).ok())
                .or_else(|| {
                    module
                        .get("title")
                        .and_then(Value::as_str)
                        .and_then(week_from_title)
                })
        })
        .collect();
    weeks.sort_unstable();
    weeks.dedup();
    weeks
}

fn format_weeks(weeks: &[u32]) -> String {
    if weeks.is_empty() {
        return "none".to_string();
    }
    let shown = weeks
        .iter()
        .take(12)
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    if weeks.len() > 12 {
        format!("{shown}, …")
    } else {
        shown
    }
}

fn normalized(value: Option<&str>) -> String {
    value
        .unwrap_or_default()
        .to_lowercase()
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn sort_courses_for_briefing(courses: &mut [Value]) {
    let now = Utc::now().with_timezone(&Seoul);
    let target = term_key_for_month(now.year(), now.month());
    courses.sort_by(|left, right| {
        let left_term = course_term_key(left);
        let right_term = course_term_key(right);
        let left_current = left_term == Some(target);
        let right_current = right_term == Some(target);
        right_current
            .cmp(&left_current)
            .then_with(|| right_term.cmp(&left_term))
            .then_with(|| {
                let left_name = left.get("name").and_then(Value::as_str).unwrap_or_default();
                let right_name = right
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                left_name.cmp(right_name)
            })
    });
}

fn add_seoul_timestamps(value: &mut Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    for key in ["created_at", "updated_at", "latest_comment_created_at"] {
        let Some(raw) = object.get(key).and_then(Value::as_str).map(str::to_string) else {
            continue;
        };
        if let Ok(date) = DateTime::parse_from_rfc3339(&raw) {
            object.insert(
                format!("{key}_seoul"),
                json!(
                    date.with_timezone(&Seoul)
                        .to_rfc3339_opts(SecondsFormat::AutoSi, false)
                ),
            );
        }
    }
}

fn compact_assignment(assignment: &Value) -> Value {
    let mut item = Map::new();
    for key in [
        "id",
        "name",
        "points_possible",
        "html_url",
        "locked_for_user",
    ] {
        if let Some(value) = assignment.get(key) {
            item.insert(key.to_string(), value.clone());
        }
    }
    item.extend(date_fields(assignment));
    if let Some(submission) = assignment.get("submission") {
        item.insert(
            "submission".to_string(),
            select(
                submission,
                &[
                    "id",
                    "workflow_state",
                    "submitted_at",
                    "score",
                    "grade",
                    "attempt",
                    "late",
                    "missing",
                ],
            ),
        );
    }
    Value::Object(item)
}

fn date_fields(value: &Value) -> Map<String, Value> {
    let mut result = Map::new();
    for key in ["unlock_at", "late_at", "due_at", "lock_at"] {
        let raw = value.get(key).cloned().unwrap_or(Value::Null);
        let seoul = raw
            .as_str()
            .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
            .map(|date| {
                date.with_timezone(&Seoul)
                    .to_rfc3339_opts(SecondsFormat::AutoSi, false)
            })
            .map(Value::String)
            .unwrap_or(Value::Null);
        result.insert(key.to_string(), raw);
        result.insert(format!("{key}_seoul"), seoul);
    }
    result
}

fn merge_object(target: &mut Value, extra: &Value) {
    if let (Some(target), Some(extra)) = (target.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
}

fn iframe_sources(html: &str) -> Vec<String> {
    let Ok(pattern) = Regex::new(r#"(?i)<iframe[^>]+src=["']([^"']+)["']"#) else {
        return Vec::new();
    };
    pattern
        .captures_iter(html)
        .filter_map(|captures| captures.get(1))
        .map(|value| sanitize_text(value.as_str()))
        .collect()
}

fn html_text(html: &str) -> String {
    let stripped = Regex::new(r"(?is)<script[^>]*>.*?</script>|<style[^>]*>.*?</style>")
        .ok()
        .map_or_else(
            || html.to_string(),
            |pattern| pattern.replace_all(html, " ").into_owned(),
        );
    let document = scraper::Html::parse_document(&stripped);
    document
        .root_element()
        .text()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Remove secret-looking keys and redact URLs in tool responses.
#[must_use]
pub fn clean(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let secret = Regex::new(r"(?i)token|secret|signature|verifier|authorization|password|nonce|secure_params|credential|csrf|oauth|access_key").ok();
            Value::Object(
                object
                    .into_iter()
                    .filter(|(key, _)| {
                        let public_status = matches!(
                            key.as_str(),
                            "learningx_token"
                                | "learningx_token_age_secs"
                                | "learningx_ok"
                                | "token_present"
                        );
                        public_status
                            || !secret.as_ref().is_some_and(|pattern| pattern.is_match(key))
                    })
                    .map(|(key, value)| (key, clean(value)))
                    .collect(),
            )
        }
        Value::Array(values) => Value::Array(values.into_iter().map(clean).collect()),
        Value::String(value) => Value::String(sanitize_text(&value)),
        other => other,
    }
}

/// ISO timestamp with microseconds.
#[must_use]
pub fn iso_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_fields_convert_to_seoul() {
        let fields = date_fields(&json!({"due_at": "2026-10-25T14:59:00Z"}));
        assert_eq!(
            fields.get("due_at_seoul").and_then(Value::as_str),
            Some("2026-10-25T23:59:00+09:00")
        );
    }

    #[test]
    fn briefing_puts_current_term_first() {
        let mut courses = vec![
            json!({"name": "Old", "term": {"name": "2021년 1학기"}}),
            json!({"name": "New", "term": {"name": "2026년 2학기"}}),
        ];
        sort_courses_for_briefing(&mut courses);
        assert_eq!(courses[0]["name"], "New");
    }

    #[test]
    fn compact_assignment_removes_description_and_keeps_dates() {
        let value = compact_assignment(&json!({
            "id": "7",
            "name": "Problem Set",
            "description": "<p>long html</p>",
            "due_at": "2026-11-20T14:59:00Z",
            "submission": {"workflow_state": "submitted", "score": 9}
        }));
        assert!(value.get("description").is_none());
        assert_eq!(
            value.get("due_at_seoul").and_then(Value::as_str),
            Some("2026-11-20T23:59:00+09:00")
        );
        assert_eq!(value["submission"]["score"], 9);
    }

    #[test]
    fn clean_removes_secret_keys_and_redacts_urls() {
        let value = clean(json!({
            "access_token": "secret",
            "url": "https://eclass3.cau.ac.kr/x?verifier=secret&wrap=1"
        }));
        let text = value.to_string();
        assert!(!text.contains("secret"));
        assert!(text.contains("wrap=1"));
    }

    #[test]
    fn clean_keeps_public_token_status_fields() {
        let value = clean(json!({
            "learningx_token": true,
            "learningx_token_age_secs": 12,
            "access_token": "secret"
        }));
        assert_eq!(value["learningx_token"], true);
        assert_eq!(value["learningx_token_age_secs"], 12);
        assert!(value.get("access_token").is_none());
    }
}
