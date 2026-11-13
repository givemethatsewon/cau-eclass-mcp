//! End-to-end submission planning and upload against a mock Canvas server.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use eclass_core::{
    error::EclassError,
    http::HttpTransport,
    service::EClassService,
    session::{Cookie, Session},
};
use serde_json::json;
use wiremock::{
    Mock, MockServer, Request, ResponseTemplate,
    matchers::{method, path},
};

fn session() -> Session {
    let cookie = |name: &str, value: &str| Cookie {
        name: name.to_string(),
        value: value.to_string(),
        domain: ".cau.ac.kr".to_string(),
        path: "/".to_string(),
        expires: None,
        secure: false,
        http_only: true,
        same_site: None,
        host_only: None,
    };
    Session::from_cdp(
        vec![
            cookie("xn_api_token", "synthetic"),
            cookie("_csrf_token", "csrf-token"),
        ],
        None,
    )
}

async fn service(server: &MockServer, output: PathBuf, enable_submission: bool) -> EClassService {
    let transport = HttpTransport::new(
        session(),
        PathBuf::from("/tmp/rust-submission-session.json"),
        url::Url::parse(&server.uri()).unwrap(),
        Duration::from_secs(20),
    )
    .unwrap();
    EClassService::new(Arc::new(transport), Some(output), enable_submission)
}

async fn mount_assignment(server: &MockServer, submitted: Arc<AtomicBool>) {
    Mock::given(method("GET"))
        .and(path("/api/v1/users/self/profile"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 7, "name": "Fixture"})))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/courses/1/assignments/10"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 10,
            "name": "Demo assignment",
            "submission_types": ["online_upload"],
            "allowed_extensions": ["txt"],
            "allowed_attempts": -1,
            "locked_for_user": false
        })))
        .mount(server)
        .await;
    // Before the upload POST the submission is empty; afterwards it carries
    // the uploaded attachment so the tool can verify its own work.
    Mock::given(method("GET"))
        .and(path("/api/v1/courses/1/assignments/10/submissions/self"))
        .respond_with(move |_request: &Request| {
            if submitted.load(Ordering::SeqCst) {
                ResponseTemplate::new(200).set_body_json(json!({
                    "id": 900,
                    "workflow_state": "submitted",
                    "attempt": 1,
                    "submitted_at": "2026-11-13T05:00:00Z",
                    "attachments": [{"id": 77, "filename": "answer.txt"}]
                }))
            } else {
                ResponseTemplate::new(200).set_body_json(json!({
                    "id": 5,
                    "workflow_state": "unsubmitted",
                    "attempt": null,
                    "submitted_at": null,
                    "attachments": []
                }))
            }
        })
        .mount(server)
        .await;
}

async fn mount_upload(server: &MockServer, submitted: Arc<AtomicBool>) {
    let upload_url = format!("{}/upload", server.uri());
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/courses/1/assignments/10/submissions/self/files",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "upload_url": upload_url,
            "upload_params": {"key": "vault/key", "policy": "synthetic"}
        })))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/upload"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 77})))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/courses/1/assignments/10/submissions"))
        .respond_with(move |_request: &Request| {
            submitted.store(true, Ordering::SeqCst);
            ResponseTemplate::new(200).set_body_json(json!({"id": 900, "attempt": 1}))
        })
        .mount(server)
        .await;
}

fn write_answer(dir: &std::path::Path, body: &str) -> String {
    let path = dir.join("answer.txt");
    std::fs::write(&path, body).unwrap();
    path.to_string_lossy().into_owned()
}

#[tokio::test]
async fn prepares_uploads_and_verifies_a_submission() {
    let server = MockServer::start().await;
    let submitted = Arc::new(AtomicBool::new(false));
    mount_assignment(&server, Arc::clone(&submitted)).await;
    mount_upload(&server, Arc::clone(&submitted)).await;
    let dir = tempfile::tempdir().unwrap();
    let file = write_answer(dir.path(), "my answer\n");
    let service = service(&server, dir.path().to_path_buf(), true).await;

    let plan = service
        .prepare_submission(1, 10, std::slice::from_ref(&file))
        .await
        .unwrap();
    assert_eq!(plan["ready"], true, "{plan}");
    assert_eq!(plan["problems"], json!([]));
    assert_eq!(plan["files"][0]["name"], "answer.txt");
    assert_eq!(plan["state"], "prepared");
    let plan_id = plan["plan_id"].as_str().unwrap().to_string();

    // A wrong confirmation must not upload anything.
    let error = service
        .submit_assignment(&plan_id, "SUBMIT 999")
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("confirmation must exactly match"),
        "{error}"
    );

    let result = service
        .submit_assignment(&plan_id, "SUBMIT 10")
        .await
        .unwrap();
    assert_eq!(result["state"], "verified");
    assert_eq!(result["submission"]["id"], 900);

    // The plan is single-use.
    let reuse = service
        .submit_assignment(&plan_id, "SUBMIT 10")
        .await
        .unwrap_err();
    assert!(
        reuse.to_string().contains("ready, unused"),
        "unexpected reuse error: {reuse}"
    );

    let requests = server.received_requests().await.unwrap();
    let submissions: Vec<&Request> = requests
        .iter()
        .filter(|request| {
            request.method == wiremock::http::Method::POST
                && request.url.path() == "/api/v1/courses/1/assignments/10/submissions"
        })
        .collect();
    assert_eq!(submissions.len(), 1, "exactly one submission POST");
    let body: serde_json::Value = serde_json::from_slice(&submissions[0].body).unwrap();
    assert_eq!(body["submission"]["file_ids"], json!([77]));
    assert_eq!(body["submission"]["submission_type"], "online_upload");
}

#[tokio::test]
async fn refuses_to_upload_when_submission_is_disabled() {
    let server = MockServer::start().await;
    let submitted = Arc::new(AtomicBool::new(false));
    mount_assignment(&server, Arc::clone(&submitted)).await;
    mount_upload(&server, Arc::clone(&submitted)).await;
    let dir = tempfile::tempdir().unwrap();
    let file = write_answer(dir.path(), "my answer\n");
    let service = service(&server, dir.path().to_path_buf(), false).await;

    let plan = service.prepare_submission(1, 10, &[file]).await.unwrap();
    assert_eq!(plan["ready"], true);
    let error = service
        .submit_assignment(plan["plan_id"].as_str().unwrap(), "SUBMIT 10")
        .await
        .unwrap_err();
    assert!(matches!(error, EclassError::BrowserOnly(_)));
    assert!(error.to_string().contains("disabled"), "{error}");

    let requests = server.received_requests().await.unwrap();
    assert!(
        requests
            .iter()
            .all(|request| request.method != wiremock::http::Method::POST),
        "no POST may leave the process"
    );
}

#[tokio::test]
async fn refuses_submission_when_the_file_changed_after_preparation() {
    let server = MockServer::start().await;
    let submitted = Arc::new(AtomicBool::new(false));
    mount_assignment(&server, Arc::clone(&submitted)).await;
    mount_upload(&server, Arc::clone(&submitted)).await;
    let dir = tempfile::tempdir().unwrap();
    let file = write_answer(dir.path(), "my answer\n");
    let service = service(&server, dir.path().to_path_buf(), true).await;

    let plan = service
        .prepare_submission(1, 10, std::slice::from_ref(&file))
        .await
        .unwrap();
    std::fs::write(&file, "tampered\n").unwrap();
    let error = service
        .submit_assignment(plan["plan_id"].as_str().unwrap(), "SUBMIT 10")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("files changed"), "{error}");
}

#[tokio::test]
async fn reports_assignment_problems_instead_of_planning() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/users/self/profile"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 7})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/courses/1/assignments/10"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 10,
            "name": "Locked assignment",
            "submission_types": ["online_upload"],
            "allowed_extensions": ["pdf"],
            "allowed_attempts": 0,
            "locked_for_user": true
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/courses/1/assignments/10/submissions/self"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 5, "workflow_state": "unsubmitted", "attempt": 0
        })))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let file = write_answer(dir.path(), "x");
    let service = service(&server, dir.path().to_path_buf(), true).await;

    let plan = service.prepare_submission(1, 10, &[file]).await.unwrap();
    assert_eq!(plan["ready"], false);
    let problems = plan["problems"].as_array().unwrap();
    let rendered = serde_json::to_string(problems).unwrap();
    assert!(rendered.contains("locked"), "{rendered}");
    assert!(rendered.contains("extension"), "{rendered}");
    assert!(rendered.contains("attempts"), "{rendered}");
}
