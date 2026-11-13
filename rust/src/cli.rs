//! Command-line entry points for the Rust core.

use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::Context;
use clap::{Parser, Subcommand};
use serde_json::{Value, json};

use crate::{
    error::EclassError,
    http::HttpTransport,
    mcp,
    service::EClassService,
    session::{
        Cookie, ORIGIN, chrome_profile_dir, default_session_file, delete_session, load_session,
        login_from_profile, refresh_from_profile, save_session, verify_session,
    },
};

#[derive(Debug, Parser)]
#[command(name = "eclass-mcp-rs", version, about = "CAU e-Class MCP (Rust)")]
struct Cli {
    /// Saved session path (env: ECLASS_SESSION_FILE).
    #[arg(long, global = true)]
    session_file: Option<PathBuf>,
    /// Dedicated Chrome profile directory (env: ECLASS_CHROME_PROFILE).
    #[arg(long, global = true)]
    profile_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the stdio MCP server (default).
    Serve,
    /// Check the saved session and login state.
    Doctor,
    /// Open Chrome once, then import its profile cookies.
    Login {
        #[arg(long)]
        browser_path: Option<PathBuf>,
        #[arg(long, default_value_t = 300.0)]
        timeout: f64,
        #[arg(long)]
        no_launch: bool,
    },
    /// Delete the saved session.
    Logout,
    /// First-run onboarding: login, verify course access and report next actions.
    Onboard {
        #[arg(long)]
        browser_path: Option<PathBuf>,
        #[arg(long, default_value_t = 300.0)]
        timeout: f64,
        #[arg(long)]
        no_launch: bool,
        #[arg(long)]
        client: Option<String>,
    },
    /// Print MCP client configuration.
    Config {
        #[arg(long, default_value = "claude")]
        client: String,
    },
    /// Refresh credentials without reopening the dedicated profile.
    Auth {
        #[command(subcommand)]
        action: AuthCommand,
    },
    /// Inspect or prune files created by the download tools.
    Downloads {
        /// Keep only the newest N files (deletes the rest).
        #[arg(long)]
        keep: Option<usize>,
        /// Report what would be removed without deleting anything.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Subcommand)]
enum AuthCommand {
    /// Mint a LearningX token (`auto`, `http`, or `aside`).
    Refresh {
        #[arg(long, default_value = "auto")]
        via: String,
    },
}

/// Run the CLI and return a process exit code.
///
/// # Errors
///
/// Returns an error for unexpected I/O or serialization failures.
pub async fn run() -> anyhow::Result<i32> {
    let cli = Cli::parse();
    let session_file = cli.session_file;
    let profile = cli.profile_dir.unwrap_or_else(chrome_profile_dir);
    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve(session_file, profile).await.map(|()| 0),
        Command::Doctor => doctor(session_file, profile).await,
        Command::Login {
            browser_path,
            timeout,
            no_launch,
        } => login(browser_path, profile, timeout, no_launch, session_file).await,
        Command::Logout => logout(session_file),
        Command::Onboard {
            browser_path,
            timeout,
            no_launch,
            client,
        } => {
            onboard(
                browser_path,
                profile,
                timeout,
                no_launch,
                session_file,
                client,
            )
            .await
        }
        Command::Config { client } => config(&client, session_file),
        Command::Auth { action } => match action {
            AuthCommand::Refresh { via } => auth_refresh(&via, session_file).await,
        },
        Command::Downloads { keep, dry_run } => downloads(keep, dry_run),
    }
}

fn downloads(keep: Option<usize>, dry_run: bool) -> anyhow::Result<i32> {
    let dir = crate::service::default_output_dir();
    let mut files = Vec::new();
    if dir.exists() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if !metadata.is_file() {
                continue;
            }
            files.push((entry.path(), metadata.len(), metadata.modified().ok()));
        }
    }
    // Newest first; unknown timestamps sort last.
    files.sort_by_key(|file| std::cmp::Reverse(file.2));
    let bytes: u64 = files.iter().map(|(_, size, _)| *size).sum();
    let mut removed = Vec::new();
    if let Some(keep) = keep {
        for (path, _, _) in files.iter().skip(keep) {
            if !dry_run {
                std::fs::remove_file(path)?;
            }
            removed.push(json!(
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            ));
        }
    }
    print_json(&json!({
        "directory": dir,
        "files": files.len(),
        "bytes": bytes,
        "keep": keep,
        "dry_run": dry_run,
        "removed": removed.len(),
        "removed_files": removed
    }))?;
    Ok(0)
}

async fn serve(session_file: Option<PathBuf>, profile: PathBuf) -> anyhow::Result<()> {
    let transport = HttpTransport::from_session(session_file, Some(profile))
        .context("loading saved session")?;
    let service = EClassService::new(
        Arc::new(transport),
        None,
        std::env::var("ECLASS_ENABLE_SUBMISSION").as_deref() == Ok("1"),
    );
    mcp::serve_stdio(service).await
}

async fn doctor(session_file: Option<PathBuf>, profile: PathBuf) -> anyhow::Result<i32> {
    let path = session_file.unwrap_or_else(default_session_file);
    let mut checks = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "transport": "http",
        "profile_dir": profile,
        "session_file": path,
        "session_present": path.exists(),
        "submission_enabled": std::env::var("ECLASS_ENABLE_SUBMISSION").as_deref() == Ok("1")
    });
    let session = match load_session(&path) {
        Ok(Some(session)) => session,
        Ok(None) => match refresh_from_profile(&profile, "chrome").await {
            Ok(mut session) => {
                save_session(&mut session, &path)?;
                checks["session_present"] = json!(true);
                session
            }
            Err(error) => {
                checks["canvas_ok"] = json!(false);
                checks["learningx_ok"] = json!(false);
                checks["ok"] = json!(false);
                checks["error_code"] = json!(error.code());
                checks["error"] = json!(error.to_string());
                checks["next_action"] = json!(error.next_action());
                print_json(&checks)?;
                return Ok(1);
            }
        },
        Err(error) => {
            checks["canvas_ok"] = json!(false);
            checks["learningx_ok"] = json!(false);
            checks["ok"] = json!(false);
            checks["error_code"] = json!(error.code());
            checks["error"] = json!(error.to_string());
            checks["next_action"] = json!(error.next_action());
            print_json(&checks)?;
            return Ok(1);
        }
    };
    let transport = HttpTransport::new(
        session,
        path,
        url::Url::parse(crate::session::ORIGIN)?,
        Duration::from_secs(20),
    )?
    .with_profile_dir(profile);
    let service = EClassService::new(Arc::new(transport), None, false);
    let canvas = service.session_status().await;
    let canvas_ok = canvas
        .as_ref()
        .ok()
        .and_then(|status| status.get("authenticated"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut learningx_ok = false;
    let mut token_present = false;
    let mut token_age = None;
    let mut error_code = canvas.as_ref().err().map(EclassError::code);
    let mut error_message = canvas.as_ref().err().map(ToString::to_string);
    let mut next_action = canvas.as_ref().err().and_then(EclassError::next_action);
    if canvas_ok {
        let _ = service.transport().mint_learningx_token().await;
        {
            let status = service.onboarding_status().await;
            token_present = status
                .get("learningx_token")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            token_age = status.get("learningx_token_age_secs").cloned();
            if let Ok(courses) = service.courses(None, None, false).await
                && let Some(course_id) = courses
                    .get("items")
                    .and_then(Value::as_array)
                    .and_then(|items| {
                        let year = chrono::Utc::now().format("%Y").to_string();
                        items
                            .iter()
                            .find(|course| {
                                course
                                    .get("term")
                                    .and_then(|term| term.get("name"))
                                    .and_then(Value::as_str)
                                    .is_some_and(|name| name.contains(&year))
                            })
                            .or_else(|| items.first())
                    })
                    .and_then(|course| course.get("id"))
                    .and_then(|value| match value {
                        Value::Number(number) => number.as_u64(),
                        Value::String(text) => text.parse::<u64>().ok(),
                        _ => None,
                    })
                    .and_then(|value| u32::try_from(value).ok())
            {
                match service.boards(course_id).await {
                    Ok(_) => learningx_ok = token_present,
                    Err(error) => {
                        learningx_ok = false;
                        error_code = Some(error.code());
                        error_message = Some(error.to_string());
                        next_action = error.next_action();
                    }
                }
            }
        }
    }
    let ok = canvas_ok && learningx_ok;
    checks["canvas_ok"] = json!(canvas_ok);
    checks["learningx_ok"] = json!(learningx_ok);
    checks["learningx_token"] = json!(token_present);
    checks["learningx_token_age_secs"] = token_age.unwrap_or(Value::Null);
    checks["ok"] = json!(ok);
    checks["error_code"] = json!(error_code);
    checks["error"] = json!(error_message);
    checks["next_action"] = json!(next_action);
    print_json(&checks)?;
    Ok(if ok { 0 } else { 1 })
}

fn browser_id(executable: &Path) -> String {
    let name = executable
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("chrome")
        .to_ascii_lowercase();
    if name.contains("edge") {
        "edge".to_string()
    } else if name.contains("brave") {
        "brave".to_string()
    } else if name.contains("chromium") {
        "chromium".to_string()
    } else {
        "chrome".to_string()
    }
}

async fn login_flow(
    browser_path: Option<PathBuf>,
    profile_dir: PathBuf,
    timeout: f64,
    no_launch: bool,
    session_file: Option<PathBuf>,
) -> anyhow::Result<(crate::session::Session, PathBuf)> {
    let profile = expand_home(&profile_dir);
    let executable = if no_launch {
        None
    } else {
        Some(find_browser(browser_path)?)
    };
    let id = executable
        .as_deref()
        .map_or_else(|| "chrome".to_string(), browser_id);
    if let Some(executable) = &executable {
        launch_browser(executable, &profile, "https://eclass3.cau.ac.kr/")?;
        eprintln!(
            "A Chrome window is open. Sign in to e-Class; this command waits up to {timeout} seconds. If it stalls, close Chrome after signing in."
        );
    } else {
        eprintln!(
            "Reading cookies from the existing Chrome profile at {}.",
            profile.display()
        );
    }
    let mut session = if no_launch {
        refresh_from_profile(&profile, &id).await?
    } else {
        login_from_profile(&profile, &id, Duration::from_secs_f64(timeout.max(1.0))).await?
    };
    if let Ok(base) = url::Url::parse(ORIGIN) {
        let _ = crate::session::mint_learningx_token(&mut session, &base, None, true).await;
    }
    let path = save_session(
        &mut session,
        &session_file.unwrap_or_else(default_session_file),
    )?;
    Ok((session, path))
}

async fn login(
    browser_path: Option<PathBuf>,
    profile_dir: PathBuf,
    timeout: f64,
    no_launch: bool,
    session_file: Option<PathBuf>,
) -> anyhow::Result<i32> {
    let profile_label = profile_dir.display().to_string();
    let (mut session, path) =
        login_flow(browser_path, profile_dir, timeout, no_launch, session_file).await?;
    let canvas_ok = verify_session(&session).await;
    if canvas_ok
        && let Ok(base) = url::Url::parse(ORIGIN)
        && let Ok(true) =
            crate::session::mint_learningx_token(&mut session, &base, None, true).await
    {
        let _ = save_session(&mut session, &path);
    }
    let learningx_token = session.token("xn_api_token").is_some();
    let ok = canvas_ok && learningx_token;
    print_json(&json!({
        "ok": ok,
        "canvas_ok": canvas_ok,
        "authenticated": canvas_ok,
        "session_file": path,
        "profile_dir": profile_label,
        "browser": session.browser,
        "saved_at": session.saved_at,
        "learningx_token": learningx_token,
        "learningx_token_age_secs": session.learningx_token_age_secs(),
        "next_action": (!ok).then_some("eclass-mcp auth refresh --via aside")
    }))?;
    Ok(if ok { 0 } else { 1 })
}

async fn auth_refresh(via: &str, session_file: Option<PathBuf>) -> anyhow::Result<i32> {
    if !matches!(via, "auto" | "http" | "aside") {
        anyhow::bail!("--via must be one of auto, http, aside");
    }
    let path = session_file.unwrap_or_else(default_session_file);
    let mut session = load_session(&path)?.unwrap_or_else(|| {
        crate::session::Session::from_browser(Vec::new(), Some("aside".to_string()))
    });
    let mut token = None;
    let mut method = None;
    let mut http_error = None;
    if via != "aside" {
        let transport = HttpTransport::new(
            session.clone(),
            path.clone(),
            url::Url::parse(ORIGIN)?,
            Duration::from_secs(20),
        )?
        .with_profile_dir(expand_home(&chrome_profile_dir()));
        match transport.refresh_learningx_token().await {
            Ok(true) => {
                session = transport.session().unwrap_or(session);
                token = session.token("xn_api_token").map(str::to_string);
                method = Some("http");
            }
            Ok(false) => {
                http_error = Some("the LTI launch did not return a token".to_string());
            }
            Err(error) => {
                http_error = Some(error.to_string());
            }
        }
    }
    if token.is_none() && via != "http" {
        match crate::aside::learningx_token().await {
            Ok(value) => {
                token = Some(value);
                method = Some("aside");
            }
            Err(error) => {
                if http_error.is_none() {
                    http_error = Some(error.to_string());
                }
            }
        }
    }
    if let Some(token) = token {
        session.upsert(Cookie {
            name: "xn_api_token".to_string(),
            value: token,
            domain: "eclass3.cau.ac.kr".to_string(),
            path: "/".to_string(),
            expires: None,
            secure: true,
            http_only: false,
            same_site: Some("Lax".to_string()),
            host_only: None,
        });
        session.learningx_token_at = Some(crate::service::iso_now());
    } else {
        session
            .cookies
            .retain(|cookie| cookie.name != "xn_api_token");
    }
    let saved = save_session(&mut session, &path)?;
    let canvas_ok = verify_session(&session).await;
    let learningx_token = session.token("xn_api_token").is_some();
    let ok = canvas_ok && learningx_token;
    print_json(&json!({
        "ok": ok,
        "via": via,
        "method": method,
        "canvas_ok": canvas_ok,
        "learningx_token": learningx_token,
        "learningx_token_age_secs": session.learningx_token_age_secs(),
        "session_file": saved,
        "error": http_error,
        "next_action": if !canvas_ok {
            Some("eclass-mcp login")
        } else if !learningx_token {
            Some("eclass-mcp auth refresh --via aside")
        } else {
            None
        }
    }))?;
    Ok(if ok { 0 } else { 1 })
}

async fn onboard(
    browser_path: Option<PathBuf>,
    profile_dir: PathBuf,
    timeout: f64,
    no_launch: bool,
    session_file: Option<PathBuf>,
    client: Option<String>,
) -> anyhow::Result<i32> {
    let path = session_file.clone().unwrap_or_else(default_session_file);
    let profile = expand_home(&profile_dir);
    let session = load_session(&path)?;
    let authenticated = if let Some(session) = session {
        let transport = HttpTransport::new(
            session,
            path.clone(),
            url::Url::parse(crate::session::ORIGIN)?,
            Duration::from_secs(20),
        )?
        .with_profile_dir(profile.clone());
        EClassService::new(Arc::new(transport), None, false)
            .session_status()
            .await
            .is_ok()
    } else {
        false
    };
    if !authenticated {
        let _ = login_flow(
            browser_path,
            profile.clone(),
            timeout,
            no_launch,
            session_file,
        )
        .await?;
    }
    let session = load_session(&path)?.ok_or(EclassError::NoSession)?;
    let transport = HttpTransport::new(
        session,
        path.clone(),
        url::Url::parse(crate::session::ORIGIN)?,
        Duration::from_secs(20),
    )?
    .with_profile_dir(profile);
    let _ = transport.mint_learningx_token().await;
    let status = EClassService::new(Arc::new(transport), None, false)
        .onboarding_status()
        .await;
    let ready = status
        .get("ready")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let config_command = client
        .as_ref()
        .map(|client| format!("eclass-mcp config --client {client}"));
    print_json(&json!({
        "version": env!("CARGO_PKG_VERSION"),
        "client": client,
        "status": status,
        "config_command": config_command
    }))?;
    Ok(if ready { 0 } else { 1 })
}

fn logout(session_file: Option<PathBuf>) -> anyhow::Result<i32> {
    let path = session_file.unwrap_or_else(default_session_file);
    let removed = delete_session(&path)?;
    print_json(&json!({"removed": removed, "session_file": path}))?;
    Ok(0)
}

fn config(client: &str, session_file: Option<PathBuf>) -> anyhow::Result<i32> {
    let command = std::env::current_exe()?.to_string_lossy().to_string();
    let mut env = serde_json::Map::new();
    env.insert("ECLASS_ENABLE_SUBMISSION".to_string(), json!("0"));
    if let Some(session_file) = session_file {
        env.insert("ECLASS_SESSION_FILE".to_string(), json!(session_file));
    }
    for key in [
        "ECLASS_SESSION_FILE",
        "ECLASS_CHROME_PROFILE",
        "ECLASS_DOWNLOAD_DIR",
        "ECLASS_CACHE_TTL",
        "ECLASS_AUTO_REFRESH",
        "ECLASS_OZ_HELPER",
        "ECLASS_OZ_BROWSER",
    ] {
        if let Ok(value) = std::env::var(key) {
            env.insert(key.to_string(), json!(value));
        }
    }
    let args: Vec<String> = Vec::new();
    match client {
        "codex" => {
            let mut text = String::from("[mcp_servers.eclass]\n");
            text.push_str(&format!("command = {}\n", json!(command)));
            text.push_str(&format!("args = {}\n\n", json!(args)));
            text.push_str("[mcp_servers.eclass.env]\n");
            for (key, value) in env {
                text.push_str(&format!("{key} = {}\n", json!(value)));
            }
            println!("{text}");
        }
        "vscode" => print_json(
            &json!({"servers": {"eclass": {"type": "stdio", "command": command, "args": args, "env": env}}}),
        )?,
        _ => print_json(
            &json!({"mcpServers": {"eclass": {"command": command, "args": args, "env": env}}}),
        )?,
    }
    Ok(0)
}

fn print_json(value: &serde_json::Value) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    std::io::stdout().flush()?;
    Ok(())
}

fn expand_home(path: &Path) -> PathBuf {
    let value = path.to_string_lossy();
    if let Some(rest) = value.strip_prefix("~/") {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join(rest)
    } else {
        path.to_path_buf()
    }
}

fn find_browser(explicit: Option<PathBuf>) -> Result<PathBuf, EclassError> {
    if let Some(path) = explicit {
        return path
            .is_file()
            .then_some(path)
            .ok_or(EclassError::BrowserNotFound);
    }
    if let Some(path) = std::env::var_os("ECLASS_BROWSER_PATH").map(PathBuf::from)
        && path.is_file()
    {
        return Ok(path);
    }
    let mut candidates = Vec::new();
    if cfg!(target_os = "macos") {
        for app in [
            "Google Chrome",
            "Microsoft Edge",
            "Brave Browser",
            "Chromium",
        ] {
            candidates.push(PathBuf::from(format!(
                "/Applications/{app}.app/Contents/MacOS/{app}"
            )));
        }
    } else if cfg!(windows) {
        for root in [
            std::env::var_os("ProgramFiles"),
            std::env::var_os("ProgramFiles(x86)"),
        ]
        .into_iter()
        .flatten()
        {
            let root = PathBuf::from(root);
            candidates.extend([
                root.join("Google/Chrome/Application/chrome.exe"),
                root.join("Microsoft/Edge/Application/msedge.exe"),
                root.join("BraveSoftware/Brave-Browser/Application/brave.exe"),
            ]);
        }
    }
    for name in [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "microsoft-edge",
        "brave-browser",
    ] {
        if let Some(path) = find_in_path(name) {
            candidates.push(path);
        }
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or(EclassError::BrowserNotFound)
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|directory| directory.join(name))
            .find(|candidate| candidate.is_file())
    })
}

fn launch_browser(executable: &Path, profile: &Path, url: &str) -> anyhow::Result<()> {
    std::fs::create_dir_all(profile)?;
    std::process::Command::new(executable)
        .args([
            format!("--user-data-dir={}", profile.display()),
            "--no-first-run".to_string(),
            "--no-default-browser-check".to_string(),
            url.to_string(),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("launching browser")?;
    Ok(())
}
