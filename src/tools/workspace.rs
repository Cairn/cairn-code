use std::path::PathBuf;

/// Resolves `path` against the current working directory and refuses any
/// path that would end up outside it (`..` escapes, absolute paths
/// elsewhere, or symlinks pointing outside the workspace). The path itself
/// does not need to exist yet — this only confines *where* it may land, so
/// it works for both reads and new-file writes.
pub fn resolve_in_workspace(path: &str) -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| format!("cannot resolve workspace root: {e}"))?;
    let cwd = cwd.canonicalize().unwrap_or(cwd);

    let candidate = std::path::Path::new(path);
    let absolute = if candidate.is_absolute() { candidate.to_path_buf() } else { cwd.join(candidate) };

    // Walk up to the deepest ancestor that actually exists so it can be
    // canonicalized (resolving any symlinks); the remaining, not-yet-created
    // tail is re-appended lexically.
    let mut existing = absolute.clone();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    while !existing.exists() {
        let Some(name) = existing.file_name().map(|n| n.to_os_string()) else { break; };
        tail.push(name);
        if !existing.pop() { break; }
    }
    let mut resolved = existing.canonicalize().unwrap_or(existing);
    for part in tail.into_iter().rev() {
        resolved.push(part);
    }

    if !resolved.starts_with(&cwd) {
        return Err(format!("refusing to access '{path}': outside the workspace ({})", cwd.display()));
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rejects_path_escaping_workspace() {
        let err = resolve_in_workspace("../../outside.txt").unwrap_err();
        assert!(err.contains("outside the workspace"), "unexpected error: {err}");
    }

    #[test]
    fn test_allows_existing_file_inside_workspace() {
        let resolved = resolve_in_workspace("Cargo.toml").unwrap();
        assert!(resolved.ends_with("Cargo.toml"));
    }

    #[test]
    fn test_allows_new_file_in_new_subdirectory_inside_workspace() {
        let resolved = resolve_in_workspace("target/cairn_workspace_test_dir/new_file.txt").unwrap();
        assert!(resolved.ends_with("new_file.txt"));
    }

    #[test]
    fn test_rejects_absolute_path_outside_workspace() {
        let outside = if cfg!(windows) { "C:\\Windows\\System32\\drivers\\etc\\hosts" } else { "/etc/hosts" };
        let err = resolve_in_workspace(outside).unwrap_err();
        assert!(err.contains("outside the workspace"), "unexpected error: {err}");
    }
}
