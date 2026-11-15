//! The small subset of Chrome DevTools Protocol used by the OZ exporter.

use std::{collections::VecDeque, ffi::OsString, path::Path, process::Stdio, time::Duration};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::{
    net::TcpStream,
    process::{Child, Command},
    time::{sleep, timeout},
};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

use crate::error::EclassError;

pub(crate) struct Cdp {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    next_id: u64,
    downloads: VecDeque<Value>,
    child: Child,
    _profile: TempDir,
}

impl Cdp {
    pub(crate) async fn launch(executable: &Path) -> Result<Self, EclassError> {
        let profile = tempfile::tempdir()?;
        let mut profile_arg = OsString::from("--user-data-dir=");
        profile_arg.push(profile.path());
        let mut child = Command::new(executable)
            .args([
                "--headless",
                "--remote-debugging-port=0",
                "--remote-debugging-address=127.0.0.1",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-background-networking",
                "--disable-extensions",
                "--use-mock-keychain",
                "--password-store=basic",
                "--window-size=1440,1000",
            ])
            .arg(profile_arg)
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| {
                EclassError::OzExport("could not launch the configured Chromium browser")
            })?;

        let socket = timeout(Duration::from_secs(15), async {
            loop {
                if child.try_wait()?.is_some() {
                    return Err(EclassError::OzExport(
                        "Chromium exited before OZ export started",
                    ));
                }
                if let Ok(port_file) =
                    tokio::fs::read_to_string(profile.path().join("DevToolsActivePort")).await
                {
                    let mut lines = port_file.lines();
                    if let (Some(port), Some(path)) = (lines.next(), lines.next())
                        && let Ok(port) = port.parse::<u16>()
                        && path
                            .strip_prefix("/devtools/browser/")
                            .is_some_and(valid_guid)
                        && let Ok((socket, _)) =
                            connect_async(format!("ws://127.0.0.1:{port}{path}")).await
                    {
                        return Ok(socket);
                    }
                }
                sleep(Duration::from_millis(100)).await;
            }
        })
        .await;
        match socket.unwrap_or(Err(EclassError::OzExport("Chromium did not become ready"))) {
            Ok(socket) => Ok(Self {
                socket,
                child,
                _profile: profile,
                next_id: 0,
                downloads: VecDeque::new(),
            }),
            Err(error) => {
                let _ = child.kill().await;
                Err(error)
            }
        }
    }

    pub(crate) async fn call(
        &mut self,
        method: &'static str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Value, EclassError> {
        self.next_id += 1;
        let id = self.next_id;
        let mut request = json!({"id": id, "method": method, "params": params});
        if let Some(session) = session {
            request["sessionId"] = json!(session);
        }
        self.socket
            .send(Message::Text(request.to_string().into()))
            .await
            .map_err(|_| EclassError::OzExport("Chromium connection closed"))?;
        loop {
            let value = self.receive().await?;
            if value["id"].as_u64() == Some(id) {
                // CDP errors may contain URLs or command arguments. Return only a static operation label.
                return value
                    .get("result")
                    .cloned()
                    .ok_or(EclassError::OzExport(method));
            }
            if is_download(&value) {
                if self.downloads.len() == 64 {
                    self.downloads.pop_front();
                }
                self.downloads.push_back(value);
            }
        }
    }

    pub(crate) async fn next_download(&mut self) -> Result<Value, EclassError> {
        if let Some(event) = self.downloads.pop_front() {
            return Ok(event);
        }
        loop {
            let value = self.receive().await?;
            if is_download(&value) {
                return Ok(value);
            }
        }
    }

    async fn receive(&mut self) -> Result<Value, EclassError> {
        while let Some(message) = self.socket.next().await {
            match message {
                Ok(Message::Text(text)) => {
                    return serde_json::from_str(&text)
                        .map_err(|_| EclassError::OzExport("invalid Chromium response"));
                }
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
        Err(EclassError::OzExport("Chromium connection closed"))
    }

    pub(crate) async fn close(mut self) {
        let _ = timeout(Duration::from_secs(3), async {
            let _ = self.call("Browser.close", json!({}), None).await;
            self.child.wait().await
        })
        .await;
        let _ = self.child.kill().await;
    }
}

fn is_download(value: &Value) -> bool {
    matches!(
        value["method"].as_str(),
        Some("Browser.downloadWillBegin" | "Browser.downloadProgress")
    )
}

pub(crate) fn valid_guid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}
