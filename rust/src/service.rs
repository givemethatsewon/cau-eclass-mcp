//! Read/download behavior ported from the Python service.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, SecondsFormat, Utc};
use chrono_tz::Asia::Seoul;
use futures_util::{StreamExt, stream};
use regex::Regex;
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::{
    catalog::{
        Material, compact_modules, course_matches, filter_materials, material_catalog,
        parse_resource_id,
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

/// e-Class reads, materials and downloads.
#[derive(Debug)]
pub struct EClassService {
    transport: Arc<HttpTransport>,
    output_dir: PathBuf,
    oz_helper: PathBuf,
    enable_submission: bool,
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
        Ok(json!({
            "authenticated": true,
            "session_state": "authenticated",
            "user": select(&user, &["id", "name"]),
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
        let session = self.transport.session();
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
        let learningx_token = session
            .as_ref()
            .and_then(|session| session.token("xn_api_token"))
            .is_some();
        let courses = if authenticated {
            self.courses(None, None).await
        } else {
            Err(EclassError::SessionExpired)
        };
        let courses_visible = courses
            .as_ref()
            .ok()
            .and_then(|value| value.get("items"))
            .and_then(Value::as_array)
            .map(Vec::len);

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
            if !learningx_token {
                actions.push(action(
                    "OPEN_WEEKLY_LEARNING",
                    "Open the weekly modules (주차학습) page once so LearningX can issue its API token.",
                    Some("eclass-mcp login --no-launch"),
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
        let ready =
            authenticated && learningx_token && courses_visible.is_some_and(|count| count > 0);
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
            "learningx_token": learningx_token,
            "courses_visible": courses_visible,
            "ready": ready,
            "next_actions": actions,
            "session_file": self.transport.session_file(),
            "error": auth_error.map(|error| error.to_string())
        })
    }

    /// List courses with optional title/code and term filters.
    pub async fn courses(
        &self,
        query: Option<&str>,
        term: Option<&str>,
    ) -> Result<Value, EclassError> {
        let mut result = self
            .paged("/api/v1/courses?include[]=term&per_page=100", false, 20)
            .await?;
        result
            .items
            .retain(|course| course_matches(course, query, term));
        for course in &mut result.items {
            if let Some(term) = course.get("term").and_then(Value::as_object) {
                let compact = select(&Value::Object(term.clone()), &["id", "name"]);
                if let Some(object) = course.as_object_mut() {
                    object.insert("term".to_string(), compact);
                }
            }
        }
        Ok(json!({
            "items": result.items,
            "complete": result.complete,
            "next_cursor": result.next_cursor
        }))
    }

    /// Canvas todo items.
    pub async fn todo(&self) -> Result<Value, EclassError> {
        let result = self
            .paged("/api/v1/users/self/todo?per_page=100", false, 20)
            .await?;
        Ok(json!({
            "items": result.items,
            "complete": result.complete,
            "next_cursor": result.next_cursor
        }))
    }

    /// One bounded pass over courses, todo items and recent announcements.
    pub async fn briefing(&self, days: u32, course_limit: usize) -> Result<Value, EclassError> {
        if !(1..=60).contains(&days) {
            return Err(EclassError::InvalidPageRange);
        }
        if !(1..=30).contains(&course_limit) {
            return Err(EclassError::InvalidPageRange);
        }
        let today = Utc::now().with_timezone(&Seoul).date_naive();
        let start = today - chrono::Duration::days(i64::from(days) - 1);
        let mut warnings = Vec::new();

        let courses = self.courses(None, None).await?;
        let all_courses = courses
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let selected: Vec<Value> = all_courses.iter().take(course_limit).cloned().collect();
        let mut complete = courses
            .get("complete")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if all_courses.len() > selected.len() {
            warnings.push(format!(
                "course_limit={course_limit} truncated {} courses",
                all_courses.len()
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
            let start = start.to_string();
            let today = today.to_string();
            async move {
                let course_id = course.get("id").and_then(positive_u64).unwrap_or_default();
                let result = announcements_on(&transport, course_id, &start, &today).await;
                (course, result)
            }
        });
        let gathered: Vec<_> = stream::iter(tasks).buffer_unordered(4).collect().await;

        let mut announcements = Vec::new();
        for (course, result) in gathered {
            let course_id = course.get("id").and_then(positive_u64).unwrap_or_default();
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

        let mut deadlines = Vec::new();
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
                if due_at.is_empty() {
                    continue;
                }
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
                item.insert(
                    "course_name".to_string(),
                    entry.get("context_name").cloned().unwrap_or(Value::Null),
                );
                item.insert(
                    "points_possible".to_string(),
                    value.get("points_possible").cloned().unwrap_or(Value::Null),
                );
                item.extend(date_fields(&Value::Object(value.clone())));
                deadlines.push(Value::Object(item));
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

        Ok(json!({
            "range": {"start_date": start.to_string(), "end_date": today.to_string()},
            "courses": selected,
            "deadlines": deadlines,
            "announcements": announcements,
            "counts": {
                "courses": selected.len(),
                "deadlines": deadlines.len(),
                "announcements": announcements.len()
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
        if detail {
            return Ok(json!({
                "items": modules,
                "complete": true,
                "next_cursor": null,
                "detail": true
            }));
        }
        let items = compact_modules(course_id, &modules, week, query, content_type);
        Ok(json!({
            "items": items,
            "complete": true,
            "next_cursor": null,
            "detail": false
        }))
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
            return Err(EclassError::InvalidPageRange);
        }
        let modules = self.modules(course_id).await?;
        let mut warnings = Vec::new();
        let files = match self
            .paged(
                &format!("/api/v1/courses/{course_id}/files?per_page=100"),
                false,
                20,
            )
            .await
        {
            Ok(files) => files,
            Err(_) => {
                warnings
                    .push("Canvas file listing unavailable; weekly materials are still included");
                Paged {
                    items: Vec::new(),
                    complete: false,
                    next_cursor: None,
                }
            }
        };
        let items = filter_materials(
            &material_catalog(course_id, &modules, &files.items),
            week,
            query,
            content_type,
        );
        let total = items.len();
        let next_offset = (offset + limit < total).then_some(offset + limit);
        let page = items
            .iter()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        Ok(json!({
            "items": page,
            "total": total,
            "complete": files.complete && next_offset.is_none(),
            "has_more": next_offset.is_some(),
            "next_offset": next_offset,
            "warnings": warnings
        }))
    }

    async fn resolve_material(
        &self,
        resource_id: &str,
    ) -> Result<(Material, (Option<String>, Option<String>)), EclassError> {
        let (source, course_id, source_id) = parse_resource_id(resource_id)?;
        if source == "canvas" {
            let file = self
                .get(
                    &format!("/api/v1/courses/{course_id}/files/{source_id}"),
                    false,
                    false,
                )
                .await?;
            if file.get("id").and_then(positive_u64) != Some(u64::from(source_id)) {
                return Err(EclassError::MaterialNotFound);
            }
            let catalog = material_catalog(course_id, &[], std::slice::from_ref(&file));
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
        let modules = self.modules(course_id).await?;
        let catalog = material_catalog(course_id, &modules, &[]);
        let material = catalog
            .into_iter()
            .find(|item| item.resource_id == resource_id)
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
        let (mut result, destination) = if material.source == "canvas" {
            let (_, course_id, source_id) = parse_resource_id(resource_id)?;
            let mut value = self.download_attachment(course_id, source_id).await?;
            let destination = value.get("path").and_then(Value::as_str).map(PathBuf::from);
            if let Some(object) = value.as_object_mut() {
                object.insert("source".to_string(), json!("canvas_file"));
                object.insert("original".to_string(), json!(true));
            }
            (value, destination)
        } else {
            let (_, course_id, source_id) = parse_resource_id(resource_id)?;
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
        Ok(json!({
            "items": result.items,
            "complete": result.complete,
            "next_cursor": result.next_cursor
        }))
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
    pub async fn assignments(&self, course_id: u32) -> Result<Paged, EclassError> {
        self.paged(
            &format!("/api/v1/courses/{course_id}/assignments?include[]=submission&per_page=100"),
            false,
            20,
        )
        .await
    }

    /// Assignment detail.
    pub async fn assignment(
        &self,
        course_id: u32,
        assignment_id: u32,
    ) -> Result<Value, EclassError> {
        self.get(
            &format!(
                "/api/v1/courses/{course_id}/assignments/{assignment_id}?include[]=submission"
            ),
            false,
            false,
        )
        .await
    }

    /// Current-user submission.
    pub async fn submission(
        &self,
        course_id: u32,
        assignment_id: u32,
    ) -> Result<Value, EclassError> {
        self.get(
            &format!("/api/v1/courses/{course_id}/assignments/{assignment_id}/submissions/self"),
            false,
            false,
        )
        .await
    }

    /// Assignment and LearningX deadlines.
    pub async fn deadlines(&self, course_id: u32) -> Result<Value, EclassError> {
        let assignments = self.assignments(course_id).await?;
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
        Ok(json!({
            "course_id": course_id,
            "text": text,
            "viewer_urls": viewers,
            "complete": viewers.is_empty() && !text.is_empty(),
            "warnings": if viewers.is_empty() { Vec::<String>::new() } else { vec!["External viewer document content is not extracted".to_string()] }
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
        Ok(json!({
            "data": body,
            "complete": complete,
            "warnings": if complete { Vec::<String>::new() } else { vec!["Provider pagination schema requires inspection".to_string()] }
        }))
    }

    /// One LearningX board page.
    pub async fn board_posts(
        &self,
        course_id: u32,
        board_id: u32,
        page: u32,
    ) -> Result<Value, EclassError> {
        let body = self
            .get(
                &format!("/learningx/api/v1/learningx_board/courses/{course_id}/boards/{board_id}/posts?page={page}"),
                true,
                false,
            )
            .await?;
        let pagination = body.get("pagination").and_then(Value::as_object);
        let current = pagination
            .and_then(|value| value.get("current_page"))
            .and_then(Value::as_u64);
        let last = pagination
            .and_then(|value| value.get("last_page"))
            .and_then(Value::as_u64);
        let known = current.is_some() && last.is_some();
        let has_more = current.zip(last).map(|(current, last)| current < last);
        Ok(json!({
            "data": body,
            "page": page,
            "complete": known && current == Some(1) && last == Some(1),
            "has_more": has_more,
            "next_page": current.zip(last).filter(|(current, last)| current < last).map(|(current, _)| current + 1),
            "warnings": if known { Vec::<String>::new() } else { vec!["Provider pagination could not be established".to_string()] }
        }))
    }

    /// LearningX post detail.
    pub async fn board_post(
        &self,
        course_id: u32,
        board_id: u32,
        post_id: u32,
    ) -> Result<Value, EclassError> {
        self.get(
            &format!("/learningx/api/v1/learningx_board/courses/{course_id}/boards/{board_id}/posts/{post_id}"),
            true,
            false,
        )
        .await
    }

    /// Current-user groups.
    pub async fn groups(&self) -> Result<Value, EclassError> {
        let result = self
            .paged("/api/v1/users/self/groups?per_page=100", false, 20)
            .await?;
        Ok(json!({
            "items": result.items,
            "complete": result.complete,
            "next_cursor": result.next_cursor
        }))
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
            "discussions": {"items": discussions.items, "complete": discussions.complete, "next_cursor": discussions.next_cursor},
            "files": {"items": files.items, "complete": files.complete, "next_cursor": files.next_cursor}
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
            Err(_) => {
                if let Some(stored) = self.plans.lock().await.get_mut(plan_id) {
                    stored.state = "uncertain".to_string();
                }
                return Err(EclassError::BrowserOnly(
                    "upload/submission outcome uncertain; inspect get_submission; do not retry this plan",
                ));
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
        EclassError::SessionExpired | EclassError::LoginRedirect | EclassError::Http(401)
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

fn default_output_dir() -> PathBuf {
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
                        !secret.as_ref().is_some_and(|pattern| pattern.is_match(key))
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
    fn clean_removes_secret_keys_and_redacts_urls() {
        let value = clean(json!({
            "access_token": "secret",
            "url": "https://eclass3.cau.ac.kr/x?verifier=secret&wrap=1"
        }));
        let text = value.to_string();
        assert!(!text.contains("secret"));
        assert!(text.contains("wrap=1"));
    }
}
