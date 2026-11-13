use eclass_core::{
    EclassError,
    session::{Cookie, Session, mint_learningx_token},
};
use url::Url;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn session(domain: &str) -> Session {
    Session {
        version: 1,
        saved_at: "2026-11-13T00:00:00Z".to_string(),
        endpoint: None,
        browser: None,
        learningx_token_at: None,
        cookies: vec![Cookie {
            name: "_normandy_session".to_string(),
            value: "synthetic".to_string(),
            domain: domain.to_string(),
            path: "/".to_string(),
            expires: None,
            secure: false,
            http_only: true,
            same_site: None,
            host_only: None,
        }],
    }
}

#[tokio::test]
async fn mints_learningx_token_from_the_lti_launch() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/courses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"id": "7", "workflow_state": "available"}
        ])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/courses/7"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"<a href="/courses/7/external_tools/9">게시판</a>"#),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/courses/7/external_tools/9"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<form id="tool_form" action="/learningx/lti/learningx_board/boards" method="POST">
                <input name="oauth_consumer_key" value="synthetic">
                <input name="custom_course_id" value="7">
            </form>"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/learningx/lti/learningx_board/boards"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("set-cookie", "xn_api_token=token-123; Path=/; Secure"),
        )
        .mount(&server)
        .await;

    let base = Url::parse(&server.uri()).unwrap();
    let mut session = session("127.0.0.1");
    let minted = mint_learningx_token(&mut session, &base, None, false)
        .await
        .unwrap();
    assert!(minted);
    assert_eq!(session.token("xn_api_token"), Some("token-123"));
    assert!(session.learningx_token_age_secs().is_some());
}

#[tokio::test]
async fn force_refresh_replaces_a_stale_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/courses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"id": "7", "workflow_state": "available"}
        ])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/courses/7"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"<a href="/courses/7/external_tools/9">게시판</a>"#),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/courses/7/external_tools/9"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<form id="tool_form" action="/learningx/lti/learningx_board/boards" method="POST">
                <input name="oauth_consumer_key" value="synthetic">
            </form>"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/learningx/lti/learningx_board/boards"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("set-cookie", "xn_api_token=fresh-token; Path=/; Secure"),
        )
        .mount(&server)
        .await;

    let base = Url::parse(&server.uri()).unwrap();
    let mut session = session("127.0.0.1");
    session.cookies.push(Cookie {
        name: "xn_api_token".to_string(),
        value: "stale-token".to_string(),
        domain: "127.0.0.1".to_string(),
        path: "/".to_string(),
        expires: None,
        secure: false,
        http_only: false,
        same_site: None,
        host_only: None,
    });
    let minted = mint_learningx_token(&mut session, &base, None, true)
        .await
        .unwrap();
    assert!(minted);
    assert_eq!(session.token("xn_api_token"), Some("fresh-token"));
}

#[tokio::test]
async fn rejects_a_missing_canvas_session() {
    let base = Url::parse("https://eclass3.cau.ac.kr/").unwrap();
    let mut session = session("example.com");
    let error = mint_learningx_token(&mut session, &base, None, false)
        .await
        .unwrap_err();
    assert!(matches!(error, EclassError::NoSession));
    assert_eq!(error.code(), "NO_SESSION");
}
