use std::{path::PathBuf, sync::Arc, time::Duration};

use eclass_core::{
    error::EclassError,
    http::HttpTransport,
    service::EClassService,
    session::{Cookie, Session},
};
use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, query_param},
};

fn session() -> Session {
    Session::from_cdp(
        vec![
            Cookie {
                name: "xn_api_token".to_string(),
                value: "synthetic".to_string(),
                domain: ".cau.ac.kr".to_string(),
                path: "/".to_string(),
                expires: None,
                secure: false,
                http_only: true,
                same_site: None,
                host_only: None,
            },
            Cookie {
                name: "_csrf_token".to_string(),
                value: "csrf-token".to_string(),
                domain: ".cau.ac.kr".to_string(),
                path: "/".to_string(),
                expires: None,
                secure: false,
                http_only: true,
                same_site: None,
                host_only: None,
            },
        ],
        None,
    )
}

async fn service(server: &MockServer, output: PathBuf) -> EClassService {
    let transport = HttpTransport::new(
        session(),
        PathBuf::from("/tmp/rust-session.json"),
        url::Url::parse(&server.uri()).unwrap(),
        Duration::from_secs(20),
    )
    .unwrap();
    EClassService::new(Arc::new(transport), Some(output), false)
}

#[tokio::test]
async fn session_status_and_courses() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/users/self/profile"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": "7", "name": "Fixture"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/courses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": "1", "name": "Algorithms", "term": {"id": "96", "name": "2026 Fall"}},
            {"id": "2", "name": "Networks", "term": {"id": "96", "name": "2026 Fall"}}
        ])))
        .mount(&server)
        .await;
    let service = service(&server, tempfile::tempdir().unwrap().keep()).await;
    let status = service.session_status().await.unwrap();
    assert_eq!(status["authenticated"], true);
    let courses = service.courses(Some("algo"), None).await.unwrap();
    assert_eq!(courses["items"].as_array().unwrap().len(), 1);
    assert_eq!(courses["items"][0]["name"], "Algorithms");
}

#[tokio::test]
async fn briefing_combines_todo_and_announcements() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/courses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": "1", "name": "Algorithms", "term": {"id": "96", "name": "2026 Fall"}},
            {"id": "2", "name": "Networks", "term": {"id": "96", "name": "2026 Fall"}}
        ])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/users/self/todo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"type": "submitting", "course_id": "1", "context_name": "Algorithms",
             "assignment": {"id": "10", "name": "Problem Set 4", "due_at": "2026-11-20T14:59:00Z"}}
        ])))
        .mount(&server)
        .await;
    for course in ["1", "2"] {
        Mock::given(method("GET"))
            .and(path("/api/v1/announcements"))
            .and(query_param("context_codes[]", format!("course_{course}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"id": 100, "title": format!("Notice {course}"), "posted_at": "2026-11-11T09:00:00Z"}
            ])))
            .expect(1)
            .mount(&server)
            .await;
    }
    let service = service(&server, tempfile::tempdir().unwrap().keep()).await;
    let briefing = service.briefing(14, 2).await.unwrap();
    assert_eq!(briefing["counts"]["courses"], 2);
    assert_eq!(briefing["counts"]["deadlines"], 1);
    assert_eq!(briefing["counts"]["announcements"], 2);
    assert_eq!(briefing["complete"], true);
}

#[tokio::test]
async fn onboarding_status_reports_ready() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/users/self/profile"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 7, "name": "Fixture"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/courses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": 1, "name": "Algorithms"}
        ])))
        .mount(&server)
        .await;
    let service = service(&server, tempfile::tempdir().unwrap().keep()).await;
    let status = service.onboarding_status().await;
    assert_eq!(status["schema_version"], 1);
    assert_eq!(status["session_state"], "authenticated");
    assert_eq!(status["authenticated"], true);
    assert_eq!(status["learningx_token"], true);
    assert_eq!(status["courses_visible"], 1);
    assert_eq!(status["ready"], true);
    let actions = status["next_actions"].as_array().unwrap();
    assert!(
        actions
            .iter()
            .any(|action| action["code"] == "CONFIGURE_CLIENT")
    );
    let serialized = status.to_string();
    assert!(
        !serialized.contains("synthetic"),
        "secret leaked: {serialized}"
    );
    assert!(
        !serialized.contains("xn_api_token"),
        "cookie name leaked: {serialized}"
    );
}

#[tokio::test]
async fn missing_session_can_still_report_onboarding() {
    let path = tempfile::tempdir().unwrap().path().join("missing.json");
    let transport = HttpTransport::from_session(Some(path), None).unwrap();
    assert!(!transport.has_session());
    let service = EClassService::new(Arc::new(transport), None, false);
    let status = service.onboarding_status().await;
    assert_eq!(status["session_state"], "missing");
    let actions = status["next_actions"].as_array().unwrap();
    assert!(actions.iter().any(|action| action["code"] == "RUN_ONBOARD"));
    let session = service.session_status().await.unwrap();
    assert_eq!(session["authenticated"], false);
    assert_eq!(session["session_state"], "missing");
}

#[tokio::test]
async fn onboarding_reports_expired_session_without_secrets() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/users/self/profile"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let service = service(&server, tempfile::tempdir().unwrap().keep()).await;
    let status = service.onboarding_status().await;
    assert_eq!(status["session_state"], "expired");
    assert_eq!(status["authenticated"], false);
    let actions = status["next_actions"].as_array().unwrap();
    assert!(
        actions
            .iter()
            .any(|action| action["code"] == "SESSION_EXPIRED")
    );
    let serialized = status.to_string();
    assert!(
        !serialized.contains("synthetic"),
        "secret leaked: {serialized}"
    );
}

#[tokio::test]
async fn commons_pdf_download_and_read_reuse_file() {
    let server = MockServer::start().await;
    let pdf =
        std::fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.pdf"))
            .unwrap();
    let download_url = format!("{}/download.pdf", server.uri());
    Mock::given(method("GET"))
        .and(path("/learningx/api/v1/courses/1/modules"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"module_id": 7, "title": "9주차", "position": 9, "module_items": [
                {"module_item_id": 8, "title": "Scheduler", "content_type": "attendance_item", "content_id": 2,
                 "content_data": {
                    "item_id": 2, "course_id": 1, "item_content_type": "commons", "opened": true,
                    "item_content_data": {"content_id": "fixture", "content_type": "pdf", "file_name": "sample.pdf"}
                 }}
            ]}
        ])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/learningx/api/v1/commons/contents"))
        .and(query_param("content_id", "fixture"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(
                json!({
                    "result": {
                        "extension": "pdf",
                        "is_original_file_deleted": false,
                        "original_file_length": pdf.len(),
                        "download_url": download_url
                    }
                })
                .to_string(),
                "text/html; charset=UTF-8",
            ),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/download.pdf"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/pdf")
                .set_body_bytes(pdf),
        )
        .expect(1)
        .mount(&server)
        .await;
    let output = tempfile::tempdir().unwrap().keep();
    let service = service(&server, output).await;
    let downloaded = service.material_download("commons:1:2").await.unwrap();
    assert_eq!(downloaded["page_count"], 2);
    assert_eq!(downloaded["source"], "commons_original");
    assert_eq!(downloaded["attendance_recorded"], false);
    let read = service
        .material_read("commons:1:2", 1, None, 0, 24_000)
        .await
        .unwrap();
    assert!(read["pages"][0]["text"].as_str().unwrap().contains("Alpha"));
    assert!(read["document_complete"].as_bool().unwrap());
}

#[tokio::test]
async fn html_login_page_is_reported_as_expired() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/users/self/profile"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw("<!DOCTYPE html><html>login</html>", "text/html"),
        )
        .mount(&server)
        .await;
    let transport = HttpTransport::new(
        session(),
        PathBuf::from("/tmp/rust-session.json"),
        url::Url::parse(&server.uri()).unwrap(),
        Duration::from_secs(20),
    )
    .unwrap()
    .with_profile_dir(tempfile::tempdir().unwrap().keep());
    let result = transport
        .get("/api/v1/users/self/profile", false, false)
        .await;
    assert!(matches!(result, Err(EclassError::LoginRedirect)));
}

#[tokio::test]
async fn group_activity_accepts_string_ids() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/users/self/groups"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": "55788", "name": "Team 10"}
        ])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/groups/55788/discussion_topics"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/groups/55788/files"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    let service = service(&server, tempfile::tempdir().unwrap().keep()).await;
    let activity = service.group_activity(55_788).await.unwrap();
    assert!(
        activity["discussions"]["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(activity["files"]["items"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn submission_uses_http_upload_and_verifies_readback() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/users/self/profile"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": "7", "name": "Fixture"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/courses/1/assignments/2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "2",
            "name": "Project",
            "submission_types": ["online_upload"],
            "allowed_extensions": ["pdf"],
            "allowed_attempts": 2,
            "locked_for_user": false
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/courses/1/assignments/2/submissions/self"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "attempt": 0,
            "submitted_at": "2026-11-01T00:00:00Z",
            "attachments": [{"id": "99"}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/courses/1/assignments/2/submissions/self/files",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "upload_url": format!("{}/upload", server.uri()),
            "upload_params": {"key": "value"}
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/upload"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 99})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/courses/1/assignments/2/submissions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 123})))
        .expect(1)
        .mount(&server)
        .await;
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("work.pdf");
    std::fs::write(&file, b"%PDF-1.7 synthetic").unwrap();
    let transport = HttpTransport::new(
        session(),
        PathBuf::from("/tmp/rust-session.json"),
        url::Url::parse(&server.uri()).unwrap(),
        Duration::from_secs(20),
    )
    .unwrap();
    let service = EClassService::new(
        Arc::new(transport),
        Some(directory.path().to_path_buf()),
        true,
    );
    let plan = service
        .prepare_submission(1, 2, &[file.to_string_lossy().to_string()])
        .await
        .unwrap();
    assert_eq!(plan["ready"], true);
    let result = service
        .submit_assignment(plan["plan_id"].as_str().unwrap(), "SUBMIT 2")
        .await
        .unwrap();
    assert_eq!(result["state"], "verified");
    assert_eq!(result["submission"]["attachments"][0]["id"], "99");
}

#[cfg(unix)]
#[tokio::test]
async fn oz_helper_is_invoked_and_path_is_validated() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let helper = directory.path().join("fake-oz-export");
    std::fs::write(
        &helper,
        r#"#!/bin/sh
set -eu
out=""
id=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --course-id) id="$2"; shift 2 ;;
    --output-dir) out="$2"; shift 2 ;;
    *) shift ;;
  esac
done
mkdir -p "$out"
file="$out/syllabus-$id.pdf"
printf '%%PDF-1.4\n' > "$file"
printf '{"path":"%s","bytes":%s,"sha256":"`zeros`","page_count":1,"text":"fixture","pages":[{"page":1,"text":"fixture"}],"text_complete":true}' "$file" "$(wc -c < "$file" | tr -d ' ')"
"#
        .replace("`zeros`", &"0".repeat(64)),
    )
    .unwrap();
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let transport = HttpTransport::new(
        session(),
        PathBuf::from("/tmp/rust-session.json"),
        url::Url::parse("https://eclass3.cau.ac.kr").unwrap(),
        Duration::from_secs(20),
    )
    .unwrap();
    let service = EClassService::new(
        Arc::new(transport),
        Some(directory.path().to_path_buf()),
        false,
    )
    .with_oz_helper(helper);
    let result = service.syllabus_document(1, true).await.unwrap();
    assert_eq!(result["course_id"], 1);
    assert_eq!(result["page_count"], 1);
    assert!(result["path"].as_str().unwrap().ends_with("syllabus-1.pdf"));
}
