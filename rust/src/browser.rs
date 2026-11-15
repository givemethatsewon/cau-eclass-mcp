//! Discover the Chromium browser already installed for e-Class login.

use std::path::PathBuf;

use crate::error::EclassError;

pub(crate) fn find_browser(explicit: Option<PathBuf>) -> Result<PathBuf, EclassError> {
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
            std::env::var_os("LOCALAPPDATA"),
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
