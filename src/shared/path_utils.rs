use super::config::get_config;
use super::source::Source;
use anyhow::Result;
use glob::glob;
use std::path::{Path, PathBuf};

/// Compact ordinary UUIDs for display. Codex typed record IDs share long
/// prefixes, so keeping those IDs whole is necessary for exact retrieval.
pub fn short_uuid(uuid: &str) -> &str {
    if uuid.contains('_') {
        uuid
    } else {
        &uuid[..8.min(uuid.len())]
    }
}

/// Replace home directory with ~ for display
pub fn home_to_tilde(path: &str) -> String {
    if path.is_empty() || path == "unknown" {
        return path.to_string();
    }
    let Some(home) = dirs::home_dir() else {
        return path.to_string();
    };
    let home_str = home.to_string_lossy();
    if home_str.is_empty() {
        path.to_string()
    } else {
        path.replace(home_str.as_ref(), "~")
    }
}

/// Convert an absolute path to Claude's project directory name format.
/// Claude replaces `/`, `\`, and `.` with `-`.
pub fn project_dir_name(path: &str) -> String {
    path.replace(['/', '\\', '.'], "-")
}

/// Path to the `.claude/projects/` directory.
pub fn projects_dir() -> Result<PathBuf> {
    Ok(get_config()
        .get_claude_dir()?
        .join("projects"))
}

/// Construct path to a session's JSONL file.
pub fn session_jsonl_path(project_path: &str, session_id: &str) -> Option<PathBuf> {
    let dir_name = project_dir_name(project_path);
    Some(
        get_config()
            .get_claude_dir()
            .ok()?
            .join("projects")
            .join(dir_name)
            .join(format!("{}.jsonl", session_id)),
    )
}

/// Find a session's JSONL file by session ID (prefix match).
/// Searches all project directories for a file named `<session_id>.jsonl`.
pub fn find_session_jsonl(session_id: &str) -> Result<Option<PathBuf>> {
    let pattern = projects_dir()?.join("**/*.jsonl");
    for path in glob(&pattern.to_string_lossy())?.flatten() {
        if let Some(stem) = path
            .file_stem()
            .and_then(|s| s.to_str())
            && (stem == session_id || stem.starts_with(session_id))
        {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// Codex roots selected explicitly through `CODEX_HOME`, or inferred for the
/// current OS user. Never scan every Windows profile visible from WSL.
fn codex_home_dirs() -> Vec<PathBuf> {
    if let Some(codex_home) = std::env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
        return vec![PathBuf::from(codex_home)];
    }

    let mut homes = Vec::new();
    if let Some(home) = dirs::home_dir() {
        homes.push(home.join(".codex"));

        if let Some(user_name) = home
            .file_name()
            .and_then(|name| name.to_str())
            && let Ok(windows_users) = std::fs::read_dir("/mnt/c/Users")
        {
            for entry in windows_users.flatten() {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(user_name)
                {
                    let windows_home = entry
                        .path()
                        .join(".codex");
                    if !homes.contains(&windows_home) {
                        homes.push(windows_home);
                    }
                }
            }
        }
    }
    homes
}

/// Find active and archived Codex session directories.
pub fn codex_sessions_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for codex_home in codex_home_dirs() {
        for child in ["sessions", "archived_sessions"] {
            let session_dir = codex_home.join(child);
            if session_dir.exists() && !dirs.contains(&session_dir) {
                dirs.push(session_dir);
            }
        }
    }
    dirs
}

/// Identify the adapter responsible for a discovered transcript path.
pub fn source_for_jsonl_path(path: &Path) -> Option<Source> {
    if projects_dir().is_ok_and(|projects| path.starts_with(projects)) {
        return Some(Source::Claude);
    }
    if codex_sessions_dirs()
        .iter()
        .any(|sessions| path.starts_with(sessions))
    {
        return Some(Source::Codex);
    }
    None
}

/// Discover all JSONL session files under both `.claude/projects/` and `.codex/sessions/`.
pub fn discover_jsonl_files() -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();

    for source in super::source::conversation_sources() {
        files.extend(source.discover()?);
    }
    files.sort();
    files.dedup();
    Ok(files)
}

pub fn discover_claude_jsonl_files() -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if let Ok(p_dir) = projects_dir() {
        let pattern = p_dir.join("**/*.jsonl");
        if let Ok(entries) = glob(&pattern.to_string_lossy()) {
            files.extend(entries.flatten());
        }
    }
    Ok(files)
}

pub fn discover_codex_jsonl_files() -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for codex_dir in codex_sessions_dirs() {
        let pattern = codex_dir.join("**/*.jsonl");
        if let Ok(entries) = glob(&pattern.to_string_lossy()) {
            files.extend(entries.flatten());
        }
    }
    Ok(files)
}

/// Find the JSONL file for the currently active Claude session by walking up from `cwd`
/// until a matching `.claude/projects/<dir>/` is found.
/// Returns the most-recently-modified JSONL in that project directory.
///
/// Note: this is only reliable when `cwd` reflects the actual current project.
/// For long-lived daemons whose cwd is fixed at startup, use
/// [`globally_active_session_jsonl`] instead.
pub fn active_session_jsonl(cwd: &Path) -> Option<PathBuf> {
    let projects = projects_dir().ok()?;
    find_session_in_projects(cwd, &projects)
}

/// Walk up from `cwd` through `projects`, returning the most-recently-modified
/// JSONL for the first matching project directory found.
pub(crate) fn find_session_in_projects(cwd: &Path, projects: &Path) -> Option<PathBuf> {
    let mut current = cwd;
    loop {
        let dir_name = project_dir_name(&current.to_string_lossy());
        let project_dir = projects.join(&dir_name);
        if project_dir.exists() {
            let best = glob(
                &project_dir
                    .join("*.jsonl")
                    .to_string_lossy(),
            )
            .ok()?
            .flatten()
            .max_by_key(|p| {
                p.metadata()
                    .and_then(|m| m.modified())
                    .ok()
            });
            if best.is_some() {
                return best;
            }
        }
        match current.parent() {
            Some(p) if p != current => current = p,
            _ => return None,
        }
    }
}

/// Find the most-recently-modified JSONL across all projects.
/// Used by long-lived processes (e.g. MCP server) whose cwd does not reflect
/// the currently active Claude session.
pub fn globally_active_session_jsonl() -> Option<PathBuf> {
    let pattern = projects_dir()
        .ok()?
        .join("**/*.jsonl");
    glob(&pattern.to_string_lossy())
        .ok()?
        .flatten()
        .max_by_key(|p| {
            p.metadata()
                .and_then(|m| m.modified())
                .ok()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_short_uuid() {
        assert_eq!(short_uuid("12345678-abcd-efgh"), "12345678");
        assert_eq!(
            short_uuid("ctc_0d7d56b337092000000000000000000000000001"),
            "ctc_0d7d56b337092000000000000000000000000001"
        );
        assert_eq!(short_uuid("abc"), "abc");
        assert_eq!(short_uuid(""), "");
    }

    #[test]
    fn test_project_dir_name() {
        assert_eq!(
            project_dir_name("/home/user/my.project"),
            "-home-user-my-project"
        );
        assert_eq!(project_dir_name("/home/user/foo"), "-home-user-foo");
        assert_eq!(project_dir_name("C:\\Users\\foo"), "C:-Users-foo");
    }

    #[test]
    fn test_active_session_jsonl_finds_session() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // Simulate: cwd = root/my/project, claude projects dir at root/claude/projects
        let cwd = root
            .join("my")
            .join("project");
        fs::create_dir_all(&cwd).unwrap();

        let dir_name = project_dir_name(&cwd.to_string_lossy());
        let project_dir = root
            .join("claude")
            .join("projects")
            .join(&dir_name);
        fs::create_dir_all(&project_dir).unwrap();

        let session_file = project_dir.join("abc123.jsonl");
        fs::write(&session_file, b"{}").unwrap();

        // active_session_jsonl uses projects_dir() which goes through config,
        // so test the walk-up logic directly with a known projects base.
        let found = find_session_in_projects(
            &cwd,
            &root
                .join("claude")
                .join("projects"),
        );
        assert_eq!(found, Some(session_file));
    }

    #[test]
    fn test_active_session_jsonl_walks_up() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // cwd is a subdirectory; session registered at parent level
        let parent = root
            .join("my")
            .join("project");
        let cwd = parent
            .join("src")
            .join("lib");
        fs::create_dir_all(&cwd).unwrap();

        let dir_name = project_dir_name(&parent.to_string_lossy());
        let project_dir = root
            .join("claude")
            .join("projects")
            .join(&dir_name);
        fs::create_dir_all(&project_dir).unwrap();
        let session_file = project_dir.join("sess.jsonl");
        fs::write(&session_file, b"{}").unwrap();

        let found = find_session_in_projects(
            &cwd,
            &root
                .join("claude")
                .join("projects"),
        );
        assert_eq!(found, Some(session_file));
    }

    #[test]
    fn test_active_session_jsonl_not_found() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp
            .path()
            .join("some")
            .join("random")
            .join("path");
        fs::create_dir_all(&cwd).unwrap();
        let projects = tmp
            .path()
            .join("claude")
            .join("projects");
        fs::create_dir_all(&projects).unwrap();

        let found = find_session_in_projects(&cwd, &projects);
        assert!(found.is_none());
    }
}
