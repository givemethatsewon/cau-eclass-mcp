//! Aside browser bridge for refreshing credentials without a fresh login.
//!
//! Aside drives the user's already signed-in browser. The REPL script below
//! reads the LearningX `xn_api_token` cookie and, when it is absent, opens a
//! LearningX board tool so the browser performs the LTI launch itself.

use std::time::Duration;

use serde_json::Value;

use crate::error::EclassError;

const SCRIPT: &str = r#"
const eclassPage = await openTab('https://eclass3.cau.ac.kr/');
await eclassPage.waitForLoadState('domcontentloaded');
const readToken = () => eclassPage.evaluate(() => {
  const match = document.cookie.match(/(^|; ?)xn_api_token=([^;]*)/);
  return match ? match[2] : null;
});
let asideToken = await readToken();
if (!asideToken) {
  const boardHref = await eclassPage.evaluate(async () => {
    const headers = { Accept: 'application/json+canvas-string-ids' };
    const courses = await (await fetch('/api/v1/courses?per_page=100', { headers })).json();
    for (const course of courses) {
      if (course.workflow_state === 'completed') continue;
      const html = await (await fetch(`/courses/${course.id}`, { headers })).text();
      const doc = new DOMParser().parseFromString(html, 'text/html');
      const links = Array.from(doc.querySelectorAll('a[href*="external_tools"]'));
      const link = links.find((item) => /게시판|board|강의자료|주차학습|모듈/i.test(item.textContent || ''));
      if (link) return link.getAttribute('href');
    }
    return null;
  });
  if (boardHref) {
    await eclassPage.goto('https://eclass3.cau.ac.kr' + boardHref);
    await sleep(2500);
    asideToken = await readToken();
  }
}
console.log('TOKEN:' + (asideToken || ''));
await closeTab(eclassPage);
"#;

/// Read a LearningX token from the user's signed-in Aside browser.
///
/// # Errors
///
/// Returns [`EclassError::BrowserOnly`] when Aside is unavailable, times out,
/// or returns no token.
pub async fn learningx_token() -> Result<String, EclassError> {
    let mut command = tokio::process::Command::new("aside");
    command
        .args(["repl", "--json", SCRIPT])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(90), command.output())
        .await
        .map_err(|_| EclassError::BrowserOnly("Aside token refresh timed out"))?
        .map_err(|_| EclassError::BrowserOnly(
            "Aside CLI is not installed or could not start; install it or use `eclass-mcp login`",
        ))?;
    if !output.status.success() {
        return Err(EclassError::BrowserOnly(
            "Aside could not read a LearningX token; make sure the browser is signed in to e-Class",
        ));
    }
    let value: Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| EclassError::BrowserOnly("Aside returned an unexpected response"))?;
    let text = value
        .get("run")
        .and_then(|run| run.get("content"))
        .and_then(Value::as_array)
        .and_then(|content| content.first())
        .and_then(|content| content.get("text"))
        .and_then(Value::as_str)
        .ok_or(EclassError::BrowserOnly("Aside returned no output"))?;
    let token = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("TOKEN:"))
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or(EclassError::BrowserOnly(
            "Aside could not find a LearningX token; open the course board once and retry",
        ))?;
    Ok(token.to_string())
}
