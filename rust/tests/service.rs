use std::{path::PathBuf, sync::Arc, time::Duration};

use eclass_core::{
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
        vec![Cookie {
            name: "xn_api_token".to_string(),
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
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 7, "name": "Fixture"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/courses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": 1, "name": "Algorithms", "term": {"id": 96, "name": "2026 Fall"}},
            {"id": 2, "name": "Networks", "term": {"id": 96, "name": "2026 Fall"}}
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
            {"id": 1, "name": "Algorithms", "term": {"id": 96, "name": "2026 Fall"}},
            {"id": 2, "name": "Networks", "term": {"id": 96, "name": "2026 Fall"}}
        ])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/users/self/todo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"type": "submitting", "course_id": 1, "context_name": "Algorithms",
             "assignment": {"id": 10, "name": "Problem Set 4", "due_at": "2026-11-20T14:59:00Z"}}
        ])))
        .mount(&server)
        .await;
    for course in [1, 2] {
        Mock::given(method("GET"))
            .and(path("/api/v1/announcements"))
            .and(query_param("context_codes[]", format!("course_{course}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"id": 100 + course, "title": format!("Notice {course}"), "posted_at": "2026-11-11T09:00:00Z"}
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
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "result": {
                "extension": "pdf",
                "is_original_file_deleted": false,
                "original_file_length": pdf.len(),
                "download_url": download_url
            }
        })))
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
