//! Optional GitHub release check. Enabled by config "update_repo" = "owner/repo"
//! (or UU_UPDATE_REPO). Only notifies; installing stays a manual download.
use std::time::Duration;

const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

pub fn spawn() {
    let Some(repo) = repo() else { return };
    tokio::spawn(async move {
        loop {
            match check(&repo, true).await {
                Ok(_) => {}
                Err(error) => log::debug!("Update check failed: {error}"),
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

/// Manual check from the window: returns the newer tag without a desktop notification.
pub async fn check_now() -> Result<Option<String>, String> {
    let repo = repo().ok_or("update_repo is not configured")?;
    check(&repo, false).await
}

fn repo() -> Option<String> {
    std::env::var("UU_UPDATE_REPO").ok().filter(|v| !v.is_empty())
        .or_else(|| crate::config::file().ok().flatten()?.get("update_repo")?.as_str().map(str::to_owned))
        .filter(|v| v.matches('/').count() == 1 && !v.chars().any(char::is_whitespace))
}

async fn check(repo: &str, desktop_notify: bool) -> Result<Option<String>, String> {
    let client = crate::auth::http_client_builder()
        .timeout(Duration::from_secs(10))
        .user_agent(concat!("uuremote-server/", env!("CARGO_PKG_VERSION")))
        // Read the tag from the redirect instead of api.github.com, whose anonymous
        // rate limit (60/h per IP) is often exhausted on shared networks.
        .redirect(reqwest::redirect::Policy::none())
        .build().map_err(|e| e.to_string())?;
    let response = client.get(format!("https://github.com/{repo}/releases/latest"))
        .send().await.map_err(|e| crate::auth::transport_error(&e))?;
    let location = response.headers().get(reqwest::header::LOCATION).and_then(|v| v.to_str().ok());
    let Some(tag) = location.and_then(tag_from_location) else {
        // No releases yet redirects to /releases, not /releases/tag/<tag>.
        if response.status().is_redirection() { return Ok(None); }
        return Err(format!("HTTP {}", response.status().as_u16()));
    };
    if !is_newer(tag, env!("CARGO_PKG_VERSION")) { return Ok(None); }
    let seen = seen_path();
    let already_seen = std::fs::read_to_string(&seen).is_ok_and(|value| value.trim() == tag);
    if desktop_notify && !already_seen {
        let url = format!("https://github.com/{repo}/releases/latest");
        log::info!("New release available: {tag}");
        if send_notification(tag, Some(&url)) { save_seen(&seen, tag); }
    } else if !desktop_notify {
        save_seen(&seen, tag);
    }
    Ok(Some(tag.to_owned()))
}

fn save_seen(path: &std::path::Path, tag: &str) {
    if let Err(error) = std::fs::write(path, tag) { log::debug!("Cannot save update state: {error}"); }
}

fn seen_path() -> std::path::PathBuf { crate::config::path().with_file_name("update_seen") }

fn send_notification(tag: &str, url: Option<&str>) -> bool {
    let body = match url {
        Some(url) => format!("新版本 {tag} 已发布\n{url}"),
        None => format!("新版本 {tag} 已发布"),
    };
    std::process::Command::new("notify-send").args(["UU Remote 更新", &body]).status().is_ok_and(|s| s.success())
}

fn tag_from_location(location: &str) -> Option<&str> {
    location.rsplit_once("/tag/").map(|(_, tag)| tag.trim_end_matches('/')).filter(|tag| !tag.is_empty())
}

fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let version = version.strip_prefix('v').unwrap_or(version);
    let mut parts = version.split('.').map(|part| part.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

fn is_newer(tag: &str, current: &str) -> bool {
    matches!((parse(tag), parse(current)), (Some(remote), Some(local)) if remote > local)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compares_release_versions() {
        assert!(is_newer("v0.2.12", "0.2.11"));
        assert!(is_newer("0.3.0", "0.2.99"));
        assert!(!is_newer("v0.2.11", "0.2.11"));
        assert!(!is_newer("v0.2.9", "0.2.10"));
        assert!(!is_newer("nightly", "0.2.11"));
    }

    #[test]
    fn reads_tag_from_release_redirect() {
        assert_eq!(tag_from_location("https://github.com/o/r/releases/tag/v0.2.12"), Some("v0.2.12"));
        assert_eq!(tag_from_location("https://github.com/o/r/releases"), None);
    }
}
