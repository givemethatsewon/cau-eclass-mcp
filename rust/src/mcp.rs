//! Minimal MCP stdio server with the same tool names as the Python build.

use std::io::Write;

use anyhow::Context;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::{
    error::EclassError,
    service::{EClassService, clean, iso_now},
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
                    "isError": false
                })),
                Err(error) => Ok(json!({
                    "content": [{"type": "text", "text": error.to_string()}],
                    "isError": true
                })),
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
    let value = match name {
        "get_session_status" => service.session_status().await?,
        "get_onboarding_status" => service.onboarding_status().await,
        "list_courses" => {
            service
                .courses(
                    args.get("query").and_then(Value::as_str),
                    args.get("term").and_then(Value::as_str),
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
                    arg_u32(args, "course_id").ok_or(EclassError::InvalidPath)?,
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
                    arg_u32(args, "course_id").ok_or(EclassError::InvalidPath)?,
                    arg_u32(args, "week"),
                    args.get("query").and_then(Value::as_str),
                    args.get("content_type").and_then(Value::as_str),
                    arg_usize(args, "offset").unwrap_or(0),
                    arg_usize(args, "limit").unwrap_or(20),
                )
                .await?
        }
        "download_material" => {
            let resource_id = arg_str(args, "resource_id")?;
            service.material_download(&resource_id).await?
        }
        "read_material" => {
            let resource_id = arg_str(args, "resource_id")?;
            let value = service
                .material_read(
                    &resource_id,
                    arg_u32(args, "start_page").unwrap_or(1),
                    arg_u32(args, "end_page"),
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
                .assignments(arg_u32(args, "course_id").ok_or(EclassError::InvalidPath)?)
                .await?;
            json!({"items": paged.items, "complete": paged.complete, "next_cursor": paged.next_cursor})
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
                .deadlines(arg_u32(args, "course_id").ok_or(EclassError::InvalidPath)?)
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
        "list_boards" => {
            service
                .boards(arg_u32(args, "course_id").ok_or(EclassError::InvalidPath)?)
                .await?
        }
        "list_board_posts" => {
            service
                .board_posts(
                    arg_u32(args, "course_id").ok_or(EclassError::InvalidPath)?,
                    arg_u32(args, "board_id").ok_or(EclassError::InvalidPath)?,
                    arg_u32(args, "page").unwrap_or(1),
                )
                .await?
        }
        "get_board_post" => {
            service
                .board_post(
                    arg_u32(args, "course_id").ok_or(EclassError::InvalidPath)?,
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
    args.get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

fn arg_usize(args: &Value, key: &str) -> Option<usize> {
    args.get(key)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
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
            description: "List courses filtered by title/code and term.",
            input_schema: schema(
                json!({"query": {"type": "string"}, "term": {"type": "string"}}),
                &[],
            ),
            annotations: read_only(),
        },
        Tool {
            name: "get_briefing",
            description: "Courses, upcoming todo deadlines and recent announcements in one call.",
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
            description: "Compact weekly modules; optional week/title/type filters.",
            input_schema: schema(
                json!({"course_id": {"type": "integer"}, "week": {"type": "integer"}, "query": {"type": "string"}, "content_type": {"type": "string"}, "detail": {"type": "boolean"}}),
                &["course_id"],
            ),
            annotations: read_only(),
        },
        Tool {
            name: "list_materials",
            description: "List Commons and Canvas materials with stable resource IDs.",
            input_schema: schema(
                json!({"course_id": {"type": "integer"}, "week": {"type": "integer"}, "query": {"type": "string"}, "content_type": {"type": "string"}, "offset": {"type": "integer", "minimum": 0}, "limit": {"type": "integer", "minimum": 1, "maximum": 100}}),
                &["course_id"],
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
            description: "Read PDF page text with resumable offsets.",
            input_schema: schema(
                json!({"resource_id": {"type": "string"}, "start_page": {"type": "integer", "minimum": 1}, "end_page": {"type": "integer", "minimum": 1}, "start_offset": {"type": "integer", "minimum": 0}, "max_chars": {"type": "integer", "minimum": 1, "maximum": 100000}}),
                &["resource_id"],
            ),
            annotations: write_read(),
        },
        Tool {
            name: "list_announcements",
            description: "List announcements in an explicit date range.",
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
            description: "List assignments and current-user submissions.",
            input_schema: schema(json!({"course_id": {"type": "integer"}}), &["course_id"]),
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
            input_schema: schema(json!({"course_id": {"type": "integer"}}), &["course_id"]),
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
            description: "OZ native PDF export; requires the Python/browser build.",
            input_schema: schema(
                json!({"course_id": {"type": "integer"}, "extract_text": {"type": "boolean"}}),
                &["course_id"],
            ),
            annotations: write_read(),
        },
        Tool {
            name: "list_boards",
            description: "List LearningX boards.",
            input_schema: schema(json!({"course_id": {"type": "integer"}}), &["course_id"]),
            annotations: read_only(),
        },
        Tool {
            name: "list_board_posts",
            description: "Read one LearningX board page.",
            input_schema: schema(
                json!({"course_id": {"type": "integer"}, "board_id": {"type": "integer"}, "page": {"type": "integer", "minimum": 1}}),
                &["course_id", "board_id"],
            ),
            annotations: read_only(),
        },
        Tool {
            name: "get_board_post",
            description: "Read a LearningX post detail.",
            input_schema: schema(
                json!({"course_id": {"type": "integer"}, "board_id": {"type": "integer"}, "post_id": {"type": "integer"}}),
                &["course_id", "board_id", "post_id"],
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
            description: "Submission validation; requires the Python/browser build.",
            input_schema: schema(
                json!({"course_id": {"type": "integer"}, "assignment_id": {"type": "integer"}, "files": {"type": "array", "items": {"type": "string"}}}),
                &["course_id", "assignment_id", "files"],
            ),
            annotations: write_read(),
        },
        Tool {
            name: "submit_assignment",
            description: "Actual submission; requires the Python/browser build.",
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
        assert_eq!(listed["result"]["tools"].as_array().unwrap().len(), 27);
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
