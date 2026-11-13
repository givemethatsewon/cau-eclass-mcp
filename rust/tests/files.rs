//! Personal file listing and guarded deletion against a mock Canvas server.

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

async fn service(server: &MockServer, delete_enabled: bool) -> EClassService {
    let transport = HttpTransport::new(
        session(),
        PathBuf::from("/tmp/rust-files-session.json"),
        url::Url::parse(&server.uri()).unwrap(),
        Duration::from_secs(20),
    )
    .unwrap();
    EClassService::new(Arc::new(transport), Some(std::env::temp_dir()), false)
        .with_delete_enabled(delete_enabled)
}

async fn mount_file(server: &MockServer, folder_id: u64, folder_name: &str, parent: Option<u64>) {
    Mock::given(method("GET"))
        .and(path("/api/v1/files/10"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 10,
            "display_name": "old-report.pdf",
            "size": 1024,
            "folder_id": folder_id,
            "created_at": "2026-11-04T12:43:07Z"
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/folders/{folder_id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": folder_id,
            "name": folder_name,
            "parent_folder_id": parent
        })))
        .mount(server)
        .await;
    if let Some(parent) = parent {
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/folders/{parent}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": parent,
                "name": "제출물",
                "parent_folder_id": null
            })))
            .mount(server)
            .await;
    }
}

#[tokio::test]
async fn lists_personal_files_newest_first() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/users/self/files"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": 1, "display_name": "older.pdf", "size": 10, "created_at": "2024-03-01T00:00:00Z", "folder_id": 9},
            {"id": 2, "display_name": "newer.pdf", "size": 20, "created_at": "2026-11-01T00:00:00Z", "folder_id": 9}
        ])))
        .mount(&server)
        .await;
    let service = service(&server, false).await;
    let listed = service.my_files(None, None, 0, 20).await.unwrap();
    assert_eq!(listed["total"], 2);
    assert_eq!(listed["items"][0]["file_id"], 2);
    assert_eq!(listed["items"][0]["name"], "newer.pdf");
    assert_eq!(
        listed["items"][0]["created_at_seoul"],
        "2026-11-01T09:00:00+09:00"
    );
    assert_eq!(listed["has_more"], false);
}

#[tokio::test]
async fn dry_run_previews_without_deleting() {
    let server = MockServer::start().await;
    mount_file(&server, 500, "리눅스", Some(400)).await;
    let service = service(&server, false).await;

    let preview = service.delete_file(10, "", true, false).await.unwrap();
    assert_eq!(preview["dry_run"], true);
    assert_eq!(preview["deleted"], false);
    assert_eq!(preview["file"]["submission_folder"], true);
    assert_eq!(preview["next"]["confirm"], "DELETE 10");
    assert_eq!(
        preview["warnings"].as_array().unwrap().len(),
        1,
        "submission folders warn"
    );
    let requests = server.received_requests().await.unwrap();
    assert!(
        requests
            .iter()
            .all(|request| request.method != wiremock::http::Method::DELETE),
        "dry run must not delete"
    );
}

#[tokio::test]
async fn refuses_real_deletes_without_opt_in_or_confirmation() {
    let server = MockServer::start().await;
    mount_file(&server, 600, "기타", None).await;
    let disabled = service(&server, false).await;
    let error = disabled
        .delete_file(10, "DELETE 10", false, false)
        .await
        .unwrap_err();
    assert!(matches!(error, EclassError::DeleteDisabled), "{error}");

    let enabled = service(&server, true).await;
    let error = enabled
        .delete_file(10, "delete 10", false, false)
        .await
        .unwrap_err();
    assert!(
        matches!(error, EclassError::DeleteConfirmation(10)),
        "{error}"
    );

    let requests = server.received_requests().await.unwrap();
    assert!(
        requests
            .iter()
            .all(|request| request.method != wiremock::http::Method::DELETE),
        "refused deletes must not reach the server"
    );
}

#[tokio::test]
async fn refuses_submission_folder_until_explicitly_allowed() {
    let server = MockServer::start().await;
    mount_file(&server, 500, "리눅스", Some(400)).await;
    let service = service(&server, true).await;
    let error = service
        .delete_file(10, "DELETE 10", false, false)
        .await
        .unwrap_err();
    assert!(matches!(error, EclassError::SubmissionFolder), "{error}");
}

#[tokio::test]
async fn deletes_when_every_gate_passes() {
    let server = MockServer::start().await;
    mount_file(&server, 600, "기타", None).await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/files/10"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": 10, "workflow_state": "deleted"})),
        )
        .mount(&server)
        .await;
    let service = service(&server, true).await;

    let result = service
        .delete_file(10, "DELETE 10", false, false)
        .await
        .unwrap();
    assert_eq!(result["deleted"], true);
    assert_eq!(result["workflow_state"], "deleted");
    assert_eq!(result["file"]["submission_folder"], false);

    let requests = server.received_requests().await.unwrap();
    let deletes: Vec<_> = requests
        .iter()
        .filter(|request| request.method == wiremock::http::Method::DELETE)
        .collect();
    assert_eq!(deletes.len(), 1);
    assert_eq!(deletes[0].url.path(), "/api/v1/files/10");
}
