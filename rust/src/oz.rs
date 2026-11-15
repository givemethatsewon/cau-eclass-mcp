//! Drive OZ's own PDF export in a short-lived Chromium profile.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};
use tokio::time::{sleep, timeout};
use url::Url;

use crate::{
    browser::find_browser,
    cdp::{Cdp, valid_guid},
    documents::{MAX_DOCUMENT_BYTES, inspect_pdf, read_pdf},
    error::EclassError,
    session::{Session, is_cau_domain},
};

const VIEWER: &str = "https://rpt80.cau.ac.kr/oz80/ozViewer.jsp";
const POLL: Duration = Duration::from_millis(200);
const STEP: Duration = Duration::from_secs(40);

pub(crate) async fn export(
    canvas: &Url,
    session: &Session,
    output: &Path,
    course_id: u32,
    extract_text: bool,
) -> Result<Value, EclassError> {
    let viewer = Url::parse(VIEWER)?;
    let syllabus = canvas.join(&format!("/courses/{course_id}/assignments/syllabus"))?;
    let cookies = browser_cookies(session, [&syllabus, &viewer]);
    if cookies.is_empty() {
        return Err(EclassError::NoSession);
    }
    let executable = find_browser(std::env::var_os("ECLASS_OZ_BROWSER").map(PathBuf::from))
        .map_err(|_| EclassError::OzBrowserNotFound)?;
    export_pdf(
        &executable,
        &syllabus,
        &viewer,
        cookies,
        output,
        course_id,
        extract_text,
    )
    .await
}

async fn export_pdf(
    executable: &Path,
    syllabus: &Url,
    viewer: &Url,
    cookies: Vec<Value>,
    output: &Path,
    course_id: u32,
    extract_text: bool,
) -> Result<Value, EclassError> {
    let output = std::path::absolute(output)?;
    fs::create_dir_all(&output)?;
    let staging = tempfile::tempdir_in(&output)?;
    let mut browser = Cdp::launch(executable).await?;
    let result = timeout(
        Duration::from_secs(180),
        browser_download(&mut browser, syllabus, viewer, cookies, staging.path()),
    )
    .await;
    browser.close().await;
    let guid = result.map_err(|_| EclassError::OzExport("browser export timed out"))??;

    tokio::task::spawn_blocking(move || {
        let temp_path = staging.path().join(&guid);
        let info = inspect_pdf(&temp_path).map_err(|_| EclassError::InvalidPdf)?;
        let (text, pages, complete) = if extract_text {
            let read = read_pdf(&temp_path, 1, Some(info.page_count), 0, 100_000)?;
            let text = read
                .pages
                .iter()
                .map(|page| page.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            let pages = read
                .pages
                .iter()
                .map(|page| json!({"page": page.page, "text": page.text}))
                .collect::<Vec<_>>();
            (Some(text), pages, Some(read.document_complete))
        } else {
            (None, Vec::new(), None)
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o600))?;
        }
        let filename = output.join(format!("syllabus-{course_id}-{guid}.pdf"));
        tempfile::TempPath::try_from_path(&temp_path)?
            .persist_noclobber(&filename)
            .map_err(|error| EclassError::Io(error.error))?;
        Ok(json!({
            "course_id": course_id,
            "path": filename,
            "bytes": info.bytes,
            "sha256": info.sha256,
            "content_type": info.content_type,
            "source": "oz_native_export",
            "page_count": info.page_count,
            "text": text,
            "pages": pages,
            "text_complete": complete
        }))
    })
    .await
    .map_err(|_| EclassError::OzExport("could not process the downloaded PDF"))?
}

async fn browser_download(
    browser: &mut Cdp,
    syllabus: &Url,
    viewer: &Url,
    cookies: Vec<Value>,
    directory: &Path,
) -> Result<String, EclassError> {
    browser
        .call(
            "Browser.setDownloadBehavior",
            json!({"behavior": "deny"}),
            None,
        )
        .await?;
    browser
        .call("Storage.setCookies", json!({"cookies": cookies}), None)
        .await?;
    let target = browser
        .call("Target.createTarget", json!({"url": "about:blank"}), None)
        .await?;
    let target = target["targetId"]
        .as_str()
        .ok_or(EclassError::OzExport("could not create a Chromium tab"))?;
    let page_session = attach(browser, target).await?;
    let navigation = browser
        .call(
            "Page.navigate",
            json!({"url": syllabus.as_str()}),
            Some(&page_session),
        )
        .await?;
    if navigation.get("errorText").is_some() {
        return Err(EclassError::OzExport("could not open the e-Class syllabus"));
    }
    let (frame, frame_session, world) =
        timeout(STEP, open_viewer(browser, &page_session, syllabus, viewer))
            .await
            .map_err(|_| EclassError::OzExport("OZ viewer Save button did not appear"))??;

    viewer_action(browser, &frame_session, world, viewer, "select").await?;
    browser
        .call(
            "Browser.setDownloadBehavior",
            json!({"behavior":"allowAndName", "downloadPath": directory, "eventsEnabled":true}),
            None,
        )
        .await?;
    viewer_action(browser, &frame_session, world, viewer, "confirm").await?;

    let mut guid = None;
    loop {
        let event = browser.next_download().await?;
        let params = &event["params"];
        let Some(event_guid) = params["guid"].as_str().filter(|value| valid_guid(value)) else {
            continue;
        };
        if event["method"] == "Browser.downloadWillBegin" && params["frameId"] == frame {
            guid.get_or_insert_with(|| event_guid.to_owned());
        }
        if event["method"] != "Browser.downloadProgress" || guid.as_deref() != Some(event_guid) {
            continue;
        }
        if ["receivedBytes", "totalBytes"].into_iter().any(|key| {
            params[key]
                .as_f64()
                .is_some_and(|size| size > MAX_DOCUMENT_BYTES as f64)
        }) {
            let _ = browser
                .call("Browser.cancelDownload", json!({"guid": event_guid}), None)
                .await;
            return Err(EclassError::DownloadTooLarge);
        }
        match params["state"].as_str() {
            Some("completed") => return Ok(event_guid.to_owned()),
            Some("canceled") => {
                return Err(EclassError::OzExport("Chromium canceled the PDF download"));
            }
            _ => {}
        }
    }
}

async fn attach(browser: &mut Cdp, target: &str) -> Result<String, EclassError> {
    let result = browser
        .call(
            "Target.attachToTarget",
            json!({"targetId": target, "flatten": true}),
            None,
        )
        .await?;
    result["sessionId"]
        .as_str()
        .map(str::to_owned)
        .ok_or(EclassError::OzExport("could not attach to the OZ frame"))
}

async fn open_viewer(
    browser: &mut Cdp,
    page: &str,
    syllabus: &Url,
    viewer: &Url,
) -> Result<(String, String, u64), EclassError> {
    let mut attached_frame = None;
    loop {
        let tree = browser
            .call("Page.getFrameTree", json!({}), Some(page))
            .await?;
        let tree = &tree["frameTree"];
        let root = &tree["frame"];
        let root_url = root["url"].as_str().and_then(|url| Url::parse(url).ok());
        if let Some(url) = root_url.filter(|url| url.as_str() != "about:blank") {
            if url.path().starts_with("/login") || url.origin() != syllabus.origin() {
                return Err(EclassError::LoginRedirect);
            }
            if url.path().trim_end_matches('/') != syllabus.path() {
                return Err(EclassError::OzExport(
                    "e-Class did not return the requested syllabus",
                ));
            }
            let local = tree["childFrames"].as_array().and_then(|children| {
                children
                    .iter()
                    .map(|child| &child["frame"])
                    .find(|frame| matches_viewer(frame, viewer))
            });
            let frame = if let Some(id) = local.and_then(|frame| frame["id"].as_str()) {
                Some((id.to_owned(), page.to_owned()))
            } else {
                let targets = browser.call("Target.getTargets", json!({}), None).await?;
                let remote = targets["targetInfos"].as_array().and_then(|targets| {
                    targets.iter().find(|target| {
                        target["type"] == "iframe"
                            && matches_viewer(target, viewer)
                            && (target["parentFrameId"] == root["id"]
                                || target["parentId"] == root["id"])
                    })
                });
                if let Some(target) = remote.and_then(|target| target["targetId"].as_str()) {
                    if attached_frame
                        .as_ref()
                        .is_none_or(|(id, _): &(String, String)| id != target)
                    {
                        attached_frame = Some((target.to_owned(), attach(browser, target).await?));
                    }
                    attached_frame.clone()
                } else {
                    None
                }
            };
            if let Some((id, session)) = frame {
                // The frame may swap processes or replace its document while loading.
                let world = browser
                    .call(
                        "Page.createIsolatedWorld",
                        json!({"frameId": id, "worldName":"eclass-oz-export"}),
                        Some(&session),
                    )
                    .await;
                if let Ok(world) = world
                    && let Some(world) = world["executionContextId"].as_u64()
                {
                    match evaluate_action(browser, &session, world, viewer, "open").await {
                        Ok(true) => return Ok((id, session, world)),
                        Ok(false) | Err(EclassError::OzExport("Runtime.evaluate")) => {}
                        Err(error) => return Err(error),
                    }
                }
            }
        }
        sleep(POLL).await;
    }
}

fn matches_viewer(frame: &Value, viewer: &Url) -> bool {
    frame["url"]
        .as_str()
        .and_then(|url| Url::parse(url).ok())
        .is_some_and(|url| url.origin() == viewer.origin() && url.path() == viewer.path())
}

async fn viewer_action(
    browser: &mut Cdp,
    frame: &str,
    world: u64,
    viewer: &Url,
    action: &'static str,
) -> Result<(), EclassError> {
    timeout(STEP, wait_for_action(browser, frame, world, viewer, action))
        .await
        .map_err(|_| EclassError::OzExport("OZ Save dialog did not become ready"))?
}

async fn wait_for_action(
    browser: &mut Cdp,
    frame: &str,
    world: u64,
    viewer: &Url,
    action: &str,
) -> Result<(), EclassError> {
    loop {
        if evaluate_action(browser, frame, world, viewer, action).await? {
            return Ok(());
        }
        sleep(POLL).await;
    }
}

async fn evaluate_action(
    browser: &mut Cdp,
    frame: &str,
    world: u64,
    viewer: &Url,
    action: &str,
) -> Result<bool, EclassError> {
    let expression = format!(
        "({})({}, {}, {})",
        include_str!("oz_viewer.js"),
        json!(viewer.origin().ascii_serialization()),
        json!(viewer.path()),
        json!(action)
    );
    let response = browser.call(
        "Runtime.evaluate",
        json!({"expression":expression, "contextId":world, "returnByValue":true, "userGesture":true}),
        Some(frame),
    ).await?;
    if response.get("exceptionDetails").is_some() {
        return Err(EclassError::OzExport(
            "could not operate the OZ Save dialog",
        ));
    }
    Ok(response["result"]["value"] == true)
}

fn browser_cookies(session: &Session, urls: [&Url; 2]) -> Vec<Value> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    session
        .cookies
        .iter()
        .filter_map(|cookie| {
            if cookie.name.is_empty()
                || cookie.name == "xn_api_token"
                || !is_cau_domain(&cookie.domain)
            {
                return None;
            }
            let domain = cookie.domain.trim_start_matches('.').to_ascii_lowercase();
            let host_only = cookie.host_only.unwrap_or(!cookie.domain.starts_with('.'));
            if !urls.iter().filter_map(|url| url.host_str()).any(|host| {
                host == domain
                    || (!host_only
                        && host
                            .strip_suffix(&domain)
                            .is_some_and(|prefix| prefix.ends_with('.')))
            }) {
                return None;
            }
            let path = if cookie.path.starts_with('/') {
                cookie.path.as_str()
            } else {
                "/"
            };
            let mut param = json!({
                "name": cookie.name, "value": cookie.value, "path": path,
                "secure": cookie.secure, "httpOnly": cookie.http_only
            });
            if host_only {
                param["url"] = json!(format!("https://{domain}/"));
            } else {
                param["domain"] = json!(format!(".{domain}"));
            }
            if let Some(expires) = cookie.expires.filter(|value| *value > 0.0) {
                if expires <= now {
                    return None;
                }
                param["expires"] = json!(expires);
            }
            match cookie
                .same_site
                .as_ref()
                .map(|value| value.to_ascii_lowercase())
                .as_deref()
            {
                Some("strict") => param["sameSite"] = json!("Strict"),
                Some("lax") => param["sameSite"] = json!("Lax"),
                Some("none" | "no_restriction") if cookie.secure => {
                    param["sameSite"] = json!("None")
                }
                _ => {}
            }
            Some(param)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{header, method, path},
    };

    #[tokio::test]
    #[ignore = "requires installed Chrome, Edge, or Chromium"]
    async fn exports_native_pdf_from_cross_origin_oz_frame() {
        let canvas = MockServer::start().await;
        let oz = MockServer::start().await;
        let mut viewer = Url::parse(&oz.uri())
            .unwrap()
            .join("/oz80/ozViewer.jsp")
            .unwrap();
        viewer.set_host(Some("localhost")).unwrap();
        let syllabus = Url::parse(&canvas.uri())
            .unwrap()
            .join("/courses/42/assignments/syllabus")
            .unwrap();
        Mock::given(method("GET"))
            .and(path(syllabus.path()))
            .and(header("Cookie", "test_session=fixture"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!(r#"<iframe src="{viewer}?document=fixture"></iframe>"#).into_bytes(),
                "text/html",
            ))
            .mount(&canvas)
            .await;
        Mock::given(method("GET")).and(path(viewer.path()))
            .respond_with(ResponseTemplate::new(200).set_body_raw(r#"
                <input type="button" class="btnSAVEAS" title="Save" value="Save" onclick="document.querySelector('dialog').showModal()">
                <dialog aria-label="Save">
                    <select class="oz_ui_layout">
                        <option value="csv">CSV</option>
                        <option value="pdf">Adobe PDF File(*.pdf)</option>
                    </select>
                    <button onclick="savePdf()">OK</button>
                </dialog>
                <script>
                    async function savePdf() {
                        if (document.querySelector('select').value !== 'pdf') return;
                        const response = await fetch('/fixture.pdf');
                        const link = document.createElement('a');
                        link.download = 'noname.pdf';
                        link.href = URL.createObjectURL(await response.blob());
                        document.body.append(link);
                        link.click();
                    }
                </script>
            "#.as_bytes(), "text/html"))
            .mount(&oz).await;
        let fixture = include_bytes!("../tests/fixtures/sample.pdf");
        Mock::given(method("GET"))
            .and(path("/fixture.pdf"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(fixture.to_vec(), "application/pdf"),
            )
            .mount(&oz)
            .await;

        let output = tempfile::tempdir().unwrap();
        let executable = find_browser(None).unwrap();
        let result = export_pdf(
            &executable,
            &syllabus,
            &viewer,
            vec![json!({
                "name": "test_session",
                "value": "fixture",
                "url": syllabus.as_str(),
                "path": "/",
                "httpOnly": true
            })],
            output.path(),
            42,
            true,
        )
        .await
        .unwrap();
        assert_eq!(result["source"], "oz_native_export");
        assert_eq!(result["course_id"], 42);
        assert!(
            result["pages"]
                .as_array()
                .is_some_and(|pages| !pages.is_empty())
        );
        let filename = Path::new(result["path"].as_str().unwrap());
        assert_eq!(filename.parent(), Some(output.path()));
        assert_eq!(fs::read(filename).unwrap(), fixture);
        assert_eq!(fs::read_dir(output.path()).unwrap().count(), 1);
    }
}
