//! Minimal MCP stdio server with 28 tools; the Python OZ helper is separate.

use std::io::Write;

use anyhow::Context;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::{
    error::EclassError,
    service::{EClassService, clean, iso_now, list_response},
};

const PROTOCOL_VERSION: &str = "2024-11-05";

#[derive(Debug, Serialize)]
struct Tool {
    name: &'static str,
    description: &'static str,
    #[serde(rename = "inputSchema")]
    input_schema: Value,
    annotations: Value,
}

/// Run the MCP server on stdin/stdout until EOF.
///
/// # Errors
///
/// Returns an error when stdin cannot be read or stdout cannot be written.
pub async fn serve_stdio(service: EClassService) -> anyhow::Result<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::BufWriter::new(tokio::io::stdout());
    while let Some(line) = lines.next_line().await.context("reading stdio")? {
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(_) => {
                let response = error_response(Value::Null, -32700, "parse error");
                write_response(&mut stdout, &response).await?;
                continue;
            }
        };
        if let Some(response) = handle_request(&service, request).await {
            write_response(&mut stdout, &response).await?;
        }
    }
    Ok(())
}

async fn write_response(
    stdout: &mut tokio::io::BufWriter<tokio::io::Stdout>,
    response: &Value,
) -> anyhow::Result<()> {
    let encoded = serde_json::to_string(response)?;
    stdout.write_all(encoded.as_bytes()).await?;
    stdout.write_all(b"\n").await?;
    stdout.flush().await?;
    Ok(())
}

async fn handle_request(service: &EClassService, request: Value) -> Option<Value> {
    let id = request.get("id").cloned();
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(id) = id else {
        // Notifications (including notifications/initialized) have no response.
        return None;
    };
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "CAU e-Class Rust", "version": env!("CARGO_PKG_VERSION")}
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools()})),
        "tools/call" => {
            let params = request.get("params").cloned().unwrap_or(Value::Null);
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match call_tool(service, name, &arguments).await {
                Ok(value) => Ok(json!({
                    "content": [{"type": "text", "text": serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string())}],
                    "structuredContent": value,
                    "isError": false
                })),
                Err(error) => {
                    let mut payload = json!({
                        "error": {
                            "code": error.code(),
                            "message": error.to_string(),
                            "next_action": error.next_action()
                        }
                    });
                    if let Some(details) = error.details()
                        && let Some(object) =
                            payload.get_mut("error").and_then(Value::as_object_mut)
                    {
                        object.insert("details".to_string(), details);
                    }
                    Ok(json!({
                        "content": [{"type": "text", "text": serde_json::to_string(&payload).unwrap_or_else(|_| error.to_string())}],
                        "structuredContent": payload,
                        "isError": true
                    }))
                }
            }
        }
        _ => Err((-32601, format!("method not found: {method}"))),
    };
    Some(match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err((code, message)) => error_response(id, code, &message),
    })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

async fn call_tool(
    service: &EClassService,
    name: &str,
    args: &Value,
) -> Result<Value, EclassError> {
    validate_args(name, args)?;
    let value = match name {
        "get_session_status" => service.session_status().await?,
        "get_onboarding_status" => service.onboarding_status().await,
        "list_courses" => {
            service
                .courses(
                    args.get("query").and_then(Value::as_str),
                    args.get("term").and_then(Value::as_str),
                    args.get("all_terms")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                )
                .await?
        }
        "get_briefing" => {
            service
                .briefing(
                    arg_u32(args, "days").unwrap_or(14),
                    arg_usize(args, "course_limit").unwrap_or(12),
                )
                .await?
        }
        "list_todo" => service.todo().await?,
        "list_learning_items" => {
            service
                .learning_items(
                    arg_course_id(service, args).await?,
                    arg_u32(args, "week"),
                    args.get("query").and_then(Value::as_str),
                    args.get("content_type").and_then(Value::as_str),
                    args.get("detail").and_then(Value::as_bool).unwrap_or(false),
                )
                .await?
        }
        "list_materials" => {
            service
                .materials(
                    arg_course_id(service, args).await?,
                    arg_u32(args, "week"),
                    args.get("query").and_then(Value::as_str),
                    args.get("content_type").and_then(Value::as_str),
                    arg_usize(args, "offset").unwrap_or(0),
                    arg_usize(args, "limit").unwrap_or(20),
                )
                .await?
        }
        "get_week_materials" => {
            service
                .week_materials(arg_course_id(service, args).await?, arg_u32(args, "week"))
                .await?
        }
        "download_material" => {
            let resource_id = arg_str(args, "resource_id")?;
            service.material_download(&resource_id).await?
        }
        "read_material" => {
            let resource_id = arg_str(args, "resource_id")?;
            let start_page = arg_u32(args, "start_page").unwrap_or(1);
            let max_pages = arg_u32(args, "max_pages").unwrap_or(10).clamp(1, 20);
            // An explicit end_page wins; otherwise max_pages bounds the range.
            let end_page =
                arg_u32(args, "end_page").or_else(|| start_page.checked_add(max_pages - 1));
            let value = service
                .material_read(
                    &resource_id,
                    start_page,
                    end_page,
                    arg_usize(args, "start_offset").unwrap_or(0),
                    arg_usize(args, "max_chars").unwrap_or(24_000),
                )
                .await?;
            // Text offsets are computed before this point; do not re-scrub the body.
            return Ok(json!({"observed_at": iso_now(), "result": value}));
        }
        "list_announcements" => {
            service
                .announcements(
                    arg_u32(args, "course_id").ok_or(EclassError::InvalidPath)?,
                    &arg_str(args, "start_date")?,
                    &arg_str(args, "end_date")?,
                )
                .await?
        }
        "get_announcement" => {
            service
                .announcement(
                    arg_u32(args, "course_id").ok_or(EclassError::InvalidPath)?,
                    arg_u32(args, "topic_id").ok_or(EclassError::InvalidPath)?,
                )
                .await?
        }
        "list_assignments" => {
            let paged = service
                .assignments(
                    arg_course_id(service, args).await?,
                    args.get("detail").and_then(Value::as_bool).unwrap_or(false),
                )
                .await?;
            let truncated = paged.next_cursor.is_some();
            let mut payload = list_response(paged.items, paged.complete && !truncated, None, None);
            if truncated && let Some(object) = payload.as_object_mut() {
                object.insert(
                    "warnings".to_string(),
                    json!(["more assignments exist beyond the page limit"]),
                );
            }
            payload
        }
        "get_assignment" => {
            service
                .assignment(
                    arg_u32(args, "course_id").ok_or(EclassError::InvalidPath)?,
                    arg_u32(args, "assignment_id").ok_or(EclassError::InvalidPath)?,
                )
                .await?
        }
        "list_deadlines" => {
            service
                .deadlines(arg_course_id(service, args).await?)
                .await?
        }
        "get_syllabus" => {
            service
                .syllabus(arg_u32(args, "course_id").ok_or(EclassError::InvalidPath)?)
                .await?
        }
        "download_syllabus_pdf" => {
            service
                .syllabus_document(
                    arg_u32(args, "course_id").ok_or(EclassError::InvalidPath)?,
                    args.get("extract_text")
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                )
                .await?
        }
        "list_boards" => service.boards(arg_course_id(service, args).await?).await?,
        "list_board_posts" => {
            service
                .board_posts(
                    arg_course_id(service, args).await?,
                    arg_u32(args, "board_id").ok_or(EclassError::InvalidPath)?,
                    arg_u32(args, "page").unwrap_or(1),
                )
                .await?
        }
        "get_board_post" => {
            service
                .board_post(
                    arg_course_id(service, args).await?,
                    arg_u32(args, "board_id").ok_or(EclassError::InvalidPath)?,
                    arg_u32(args, "post_id").ok_or(EclassError::InvalidPath)?,
                )
                .await?
        }
        "list_my_groups" => service.groups().await?,
        "get_group_activity" => {
            service
                .group_activity(arg_u32(args, "group_id").ok_or(EclassError::InvalidPath)?)
                .await?
        }
        "get_video_download_info" => {
            service
                .video_info(arg_u32(args, "course_id").ok_or(EclassError::InvalidPath)?)
                .await?
        }
        "download_attachment" => {
            service
                .download_attachment(
                    arg_u32(args, "course_id").ok_or(EclassError::InvalidPath)?,
                    arg_u32(args, "file_id").ok_or(EclassError::InvalidPath)?,
                )
                .await?
        }
        "download_video_original" => {
            service
                .download_video(
                    arg_u32(args, "course_id").ok_or(EclassError::InvalidPath)?,
                    arg_u32(args, "item_id").ok_or(EclassError::InvalidPath)?,
                    args.get("allow_playback_copy")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                )
                .await?
        }
        "get_submission" => {
            service
                .submission(
                    arg_u32(args, "course_id").ok_or(EclassError::InvalidPath)?,
                    arg_u32(args, "assignment_id").ok_or(EclassError::InvalidPath)?,
                )
                .await?
        }
        "prepare_submission" => {
            let files = args
                .get("files")
                .and_then(Value::as_array)
                .map(|files| {
                    files
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            service
                .prepare_submission(
                    arg_u32(args, "course_id").ok_or(EclassError::InvalidPath)?,
                    arg_u32(args, "assignment_id").ok_or(EclassError::InvalidPath)?,
                    &files,
                )
                .await?
        }
        "submit_assignment" => {
            service
                .submit_assignment(&arg_str(args, "plan_id")?, &arg_str(args, "confirmation")?)
                .await?
        }
        other => return Err(EclassError::UnknownTool(other.to_string())),
    };
    Ok(json!({"observed_at": iso_now(), "result": clean(value)}))
}

fn arg_str(args: &Value, key: &str) -> Result<String, EclassError> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or(EclassError::InvalidPath)
}

fn arg_u32(args: &Value, key: &str) -> Option<u32> {
    args.get(key).and_then(|value| match value {
        Value::Number(number) => number.as_u64().and_then(|value| u32::try_from(value).ok()),
        Value::String(text) => text.parse::<u32>().ok(),
        _ => None,
    })
}

fn arg_usize(args: &Value, key: &str) -> Option<usize> {
    args.get(key)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

/// Resolve `course_id`, or a human course name/code through `list_courses`.
///
/// Ambiguous names fail with `AMBIGUOUS_COURSE` and candidate IDs instead of
/// silently returning data for an arbitrary course.
async fn arg_course_id(service: &EClassService, args: &Value) -> Result<u32, EclassError> {
    if let Some(course_id) = arg_u32(args, "course_id") {
        return Ok(course_id);
    }
    let query = args
        .get("course")
        .and_then(Value::as_str)
        .ok_or(EclassError::CourseNotFound)?;
    service.resolve_course(query).await
}

/// Reject unknown or missing arguments, and check the declared JSON types.
///
/// MCP clients vary in how strictly they validate `inputSchema`, so the server
/// enforces it: a typo such as `max_page` must fail loudly instead of being
/// dropped and returning unfiltered data.
fn validate_args(name: &str, args: &Value) -> Result<(), EclassError> {
    let schema = tools()
        .into_iter()
        .find(|tool| tool.name == name)
        .map(|tool| tool.input_schema)
        .ok_or_else(|| EclassError::UnknownTool(name.to_string()))?;
    let Some(object) = args.as_object() else {
        return Err(EclassError::InvalidArgument(format!(
            "{name} expects a JSON object"
        )));
    };
    let properties = schema.get("properties").and_then(Value::as_object);
    let allowed: Vec<&str> = properties
        .map(|properties| properties.keys().map(String::as_str).collect())
        .unwrap_or_default();
    let unknown: Vec<&str> = object
        .keys()
        .map(String::as_str)
        .filter(|key| !allowed.contains(key))
        .collect();
    if !unknown.is_empty() {
        return Err(EclassError::InvalidArgument(format!(
            "{name} does not accept {}; allowed arguments: {}",
            unknown
                .iter()
                .map(|key| format!("`{key}`"))
                .collect::<Vec<_>>()
                .join(", "),
            if allowed.is_empty() {
                "none".to_string()
            } else {
                allowed.join(", ")
            }
        )));
    }
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|required| required.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let missing: Vec<&str> = required
        .iter()
        .copied()
        .filter(|key| !object.get(*key).is_some_and(|value| !value.is_null()))
        .collect();
    if !missing.is_empty() {
        return Err(EclassError::InvalidArgument(format!(
            "{name} requires {}; allowed arguments: {}",
            missing
                .iter()
                .map(|key| format!("`{key}`"))
                .collect::<Vec<_>>()
                .join(", "),
            allowed.join(", ")
        )));
    }
    if let Some(properties) = properties {
        for (key, value) in object {
            let Some(property) = properties.get(key) else {
                continue;
            };
            let kind = property
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            // Treat an explicit null as "not provided" for optional arguments.
            if value.is_null() {
                continue;
            }
            if !type_matches(kind, value) {
                return Err(EclassError::InvalidArgument(format!(
                    "`{key}` for {name} must be {kind}, got {}",
                    json_type(value)
                )));
            }
        }
    }
    Ok(())
}

fn type_matches(kind: &str, value: &Value) -> bool {
    match kind {
        "integer" => match value {
            Value::Number(number) => number.as_i64().is_some() || number.as_u64().is_some(),
            Value::String(text) => text.parse::<i64>().is_ok(),
            _ => false,
        },
        "number" => value.is_number(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => true,
    }
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
}

fn read_only() -> Value {
    json!({"readOnlyHint": true, "destructiveHint": false, "openWorldHint": true})
}

fn write_read() -> Value {
    json!({"readOnlyHint": false, "destructiveHint": false, "openWorldHint": true})
}

fn tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "get_session_status",
            description: "Check the saved session, current user and submission flag.",
            input_schema: schema(json!({}), &[]),
            annotations: read_only(),
        },
        Tool {
            name: "get_onboarding_status",
            description: "Check session, LearningX token, course visibility and next onboarding actions.",
            input_schema: schema(json!({}), &[]),
            annotations: read_only(),
        },
        Tool {
            name: "list_courses",
            description: "List compact courses for the current academic term (id, name, term). Pass all_terms=true or term=\"2026년 1학기\" to widen.",
            input_schema: schema(
                json!({"query": {"type": "string"}, "term": {"type": "string"}, "all_terms": {"type": "boolean"}}),
                &[],
            ),
            annotations: read_only(),
        },
        Tool {
            name: "get_briefing",
            description: "One call: current-term courses, deadlines due in the next `days` (Asia/Seoul), recent announcements, and already-overdue items.",
            input_schema: schema(
                json!({"days": {"type": "integer", "minimum": 1, "maximum": 60}, "course_limit": {"type": "integer", "minimum": 1, "maximum": 30}}),
                &[],
            ),
            annotations: read_only(),
        },
        Tool {
            name: "list_todo",
            description: "List Canvas todo items.",
            input_schema: schema(json!({}), &[]),
            annotations: read_only(),
        },
        Tool {
            name: "list_learning_items",
            description: "Compact weekly modules with current week; optional week/title/type filters.",
            input_schema: schema(
                json!({"course_id": {"type": "integer"}, "course": {"type": "string"}, "week": {"type": "integer"}, "query": {"type": "string"}, "content_type": {"type": "string"}, "detail": {"type": "boolean"}}),
                &[],
            ),
            annotations: read_only(),
        },
        Tool {
            name: "list_materials",
            description: "List Commons videos, Canvas files and LearningX board attachments with stable resource IDs. Follow next with next_param=offset while has_more=true.",
            input_schema: schema(
                json!({"course_id": {"type": "integer"}, "course": {"type": "string"}, "week": {"type": "integer"}, "query": {"type": "string"}, "content_type": {"type": "string"}, "offset": {"type": "integer", "minimum": 0}, "limit": {"type": "integer", "minimum": 1, "maximum": 100}}),
                &[],
            ),
            annotations: read_only(),
        },
        Tool {
            name: "get_week_materials",
            description: "Materials for the latest week that has any; passes course_id or an exact course name. An explicit week is checked against the course's weeks.",
            input_schema: schema(
                json!({"course_id": {"type": "integer"}, "course": {"type": "string"}, "week": {"type": "integer", "minimum": 1, "maximum": 30}}),
                &[],
            ),
            annotations: read_only(),
        },
        Tool {
            name: "download_material",
            description: "Download a listed material and return path, size and SHA-256.",
            input_schema: schema(json!({"resource_id": {"type": "string"}}), &["resource_id"]),
            annotations: write_read(),
        },
        Tool {
            name: "read_material",
            description: "Read PDF page text. Use max_pages (1-20, default 10) or end_page to bound one call; continue with next_page + next_offset while has_more=true.",
            input_schema: schema(
                json!({"resource_id": {"type": "string"}, "start_page": {"type": "integer", "minimum": 1}, "end_page": {"type": "integer", "minimum": 1}, "max_pages": {"type": "integer", "minimum": 1, "maximum": 20}, "start_offset": {"type": "integer", "minimum": 0}, "max_chars": {"type": "integer", "minimum": 1, "maximum": 100000}}),
                &["resource_id"],
            ),
            annotations: write_read(),
        },
        Tool {
            name: "list_announcements",
            description: "List announcements between start_date and end_date (YYYY-MM-DD, Asia/Seoul). Both dates are required; get_briefing covers recent announcements across courses.",
            input_schema: schema(
                json!({"course_id": {"type": "integer"}, "start_date": {"type": "string"}, "end_date": {"type": "string"}}),
                &["course_id", "start_date", "end_date"],
            ),
            annotations: read_only(),
        },
        Tool {
            name: "get_announcement",
            description: "Get an announcement/discussion detail.",
            input_schema: schema(
                json!({"course_id": {"type": "integer"}, "topic_id": {"type": "integer"}}),
                &["course_id", "topic_id"],
            ),
            annotations: read_only(),
        },
        Tool {
            name: "list_assignments",
            description: "List assignments and current-user submissions; compact by default, detail=true for raw Canvas fields.",
            input_schema: schema(
                json!({"course_id": {"type": "integer"}, "course": {"type": "string"}, "detail": {"type": "boolean"}}),
                &[],
            ),
            annotations: read_only(),
        },
        Tool {
            name: "get_assignment",
            description: "Get assignment instructions and submission state.",
            input_schema: schema(
                json!({"course_id": {"type": "integer"}, "assignment_id": {"type": "integer"}}),
                &["course_id", "assignment_id"],
            ),
            annotations: read_only(),
        },
        Tool {
            name: "list_deadlines",
            description: "Combine assignment and LearningX deadlines in Asia/Seoul.",
            input_schema: schema(
                json!({"course_id": {"type": "integer"}, "course": {"type": "string"}}),
                &[],
            ),
            annotations: read_only(),
        },
        Tool {
            name: "get_syllabus",
            description: "Get syllabus text and external viewer links.",
            input_schema: schema(json!({"course_id": {"type": "integer"}}), &["course_id"]),
            annotations: read_only(),
        },
        Tool {
            name: "download_syllabus_pdf",
            description: "OZ native PDF export; requires the optional eclass-oz-export helper with Playwright.",
            input_schema: schema(
                json!({"course_id": {"type": "integer"}, "extract_text": {"type": "boolean"}}),
                &["course_id"],
            ),
            annotations: write_read(),
        },
        Tool {
            name: "list_boards",
            description: "List LearningX boards.",
            input_schema: schema(
                json!({"course_id": {"type": "integer"}, "course": {"type": "string"}}),
                &[],
            ),
            annotations: read_only(),
        },
        Tool {
            name: "list_board_posts",
            description: "Read one LearningX board page (KST timestamps). Get board_id from list_boards; continue with next/next_param.",
            input_schema: schema(
                json!({"course_id": {"type": "integer"}, "course": {"type": "string"}, "board_id": {"type": "integer"}, "page": {"type": "integer", "minimum": 1}}),
                &["board_id"],
            ),
            annotations: read_only(),
        },
        Tool {
            name: "get_board_post",
            description: "Read a LearningX post detail with attachments and KST timestamps.",
            input_schema: schema(
                json!({"course_id": {"type": "integer"}, "course": {"type": "string"}, "board_id": {"type": "integer"}, "post_id": {"type": "integer"}}),
                &["board_id", "post_id"],
            ),
            annotations: read_only(),
        },
        Tool {
            name: "list_my_groups",
            description: "List the current user's Canvas groups.",
            input_schema: schema(json!({}), &[]),
            annotations: read_only(),
        },
        Tool {
            name: "get_group_activity",
            description: "Get discussions and files for a verified group.",
            input_schema: schema(json!({"group_id": {"type": "integer"}}), &["group_id"]),
            annotations: read_only(),
        },
        Tool {
            name: "get_video_download_info",
            description: "Inspect video metadata without playing.",
            input_schema: schema(json!({"course_id": {"type": "integer"}}), &["course_id"]),
            annotations: read_only(),
        },
        Tool {
            name: "download_attachment",
            description: "Download a Canvas file by file ID.",
            input_schema: schema(
                json!({"course_id": {"type": "integer"}, "file_id": {"type": "integer"}}),
                &["course_id", "file_id"],
            ),
            annotations: write_read(),
        },
        Tool {
            name: "download_video_original",
            description: "Download an accessible Commons original MP4.",
            input_schema: schema(
                json!({"course_id": {"type": "integer"}, "item_id": {"type": "integer"}, "allow_playback_copy": {"type": "boolean"}}),
                &["course_id", "item_id"],
            ),
            annotations: write_read(),
        },
        Tool {
            name: "get_submission",
            description: "Read the current user's submission.",
            input_schema: schema(
                json!({"course_id": {"type": "integer"}, "assignment_id": {"type": "integer"}}),
                &["course_id", "assignment_id"],
            ),
            annotations: read_only(),
        },
        Tool {
            name: "prepare_submission",
            description: "Validate local files and assignment; create a short-lived plan. No upload.",
            input_schema: schema(
                json!({"course_id": {"type": "integer"}, "assignment_id": {"type": "integer"}, "files": {"type": "array", "items": {"type": "string"}}}),
                &["course_id", "assignment_id", "files"],
            ),
            annotations: write_read(),
        },
        Tool {
            name: "submit_assignment",
            description: "Actual submission. Disabled by default; requires opt-in, a ready plan and confirmation.",
            input_schema: schema(
                json!({"plan_id": {"type": "string"}, "confirmation": {"type": "string"}}),
                &["plan_id", "confirmation"],
            ),
            annotations: json!({"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": true}),
        },
    ]
}

/// Flush a human-readable line without touching MCP stdout.
pub fn eprintln_flush(message: &str) {
    eprintln!("{message}");
    let _ = std::io::stderr().flush();
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc, time::Duration};

    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    use super::*;
    use crate::{
        http::HttpTransport,
        service::EClassService,
        session::{Cookie, Session},
    };

    fn session() -> Session {
        Session::from_cdp(
            vec![Cookie {
                name: "sid".to_string(),
                value: "synthetic".to_string(),
                domain: ".cau.ac.kr".to_string(),
                path: "/".to_string(),
                expires: None,
                secure: false,
                http_only: true,
                same_site: None,
                host_only: None,
            }],
            None,
        )
    }

    async fn service(server: &MockServer) -> EClassService {
        let transport = HttpTransport::new(
            session(),
            PathBuf::from("/tmp/rust-mcp-session.json"),
            url::Url::parse(&server.uri()).unwrap(),
            Duration::from_secs(20),
        )
        .unwrap();
        EClassService::new(Arc::new(transport), None, false)
    }

    #[tokio::test]
    async fn initialize_and_list_tools() {
        let server = MockServer::start().await;
        let service = service(&server).await;
        let initialized = handle_request(
            &service,
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
        )
        .await
        .unwrap();
        assert_eq!(
            initialized["result"]["serverInfo"]["name"],
            "CAU e-Class Rust"
        );
        let listed = handle_request(
            &service,
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
        )
        .await
        .unwrap();
        assert_eq!(listed["result"]["tools"].as_array().unwrap().len(), 28);
    }

    #[test]
    fn rejects_unknown_arguments_instead_of_ignoring_them() {
        let error = validate_args(
            "get_week_materials",
            &json!({"course_id": 1, "max_pages": 1}),
        )
        .unwrap_err();
        assert_eq!(error.code(), "INVALID_ARGUMENT");
        let message = error.to_string();
        assert!(message.contains("max_pages"), "{message}");
        assert!(message.contains("allowed arguments"), "{message}");
        assert!(message.contains("week"), "{message}");
    }

    #[test]
    fn rejects_missing_required_arguments_with_a_useful_message() {
        let error = validate_args("list_board_posts", &json!({"course_id": 1})).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("requires `board_id`"), "{message}");
    }

    #[test]
    fn treats_explicit_nulls_as_absent() {
        validate_args(
            "list_materials",
            &json!({"course_id": 1, "week": null, "query": null}),
        )
        .unwrap();
        let error = validate_args(
            "get_assignment",
            &json!({"course_id": 1, "assignment_id": null}),
        )
        .unwrap_err();
        assert!(error.to_string().contains("requires `assignment_id`"));
    }

    #[test]
    fn rejects_wrong_argument_types() {
        let error =
            validate_args("list_materials", &json!({"course_id": 1, "limit": "many"})).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("`limit` for list_materials must be integer")
        );
        // Numeric strings are accepted for integer arguments.
        validate_args("list_materials", &json!({"course_id": "1", "limit": "5"})).unwrap();
    }

    #[test]
    fn accepts_every_declared_argument_for_every_tool() {
        for tool in tools() {
            let properties = tool.input_schema["properties"]
                .as_object()
                .cloned()
                .unwrap_or_default();
            assert!(
                properties.keys().all(|key| !key.is_empty()),
                "{} has an empty property name",
                tool.name
            );
            let required = tool.input_schema["required"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            for key in &required {
                assert!(
                    properties.contains_key(key.as_str().unwrap()),
                    "{} requires an undeclared argument",
                    tool.name
                );
            }
        }
    }

    #[tokio::test]
    async fn tool_call_returns_wrapped_result() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/users/self/profile"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"id": 7, "name": "Fixture"})),
            )
            .mount(&server)
            .await;
        let service = service(&server).await;
        let response = handle_request(
            &service,
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {"name": "get_session_status", "arguments": {}}
            }),
        )
        .await
        .unwrap();
        assert_eq!(response["result"]["isError"], false);
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("\"authenticated\":true"), "{text}");
        assert!(text.contains("observed_at"), "{text}");
    }
}
