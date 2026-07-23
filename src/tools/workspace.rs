use cap_std::ambient_authority;
use cap_std::fs::Dir;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

#[derive(Clone)]
pub struct Workspace {
    root: PathBuf,
    path_root: PathBuf,
    dir: Arc<Dir>,
}

impl Workspace {
    pub fn current() -> Result<Self, String> {
        Self::new(".")
    }

    pub fn new(root: impl AsRef<Path>) -> Result<Self, String> {
        let requested_root = root.as_ref();
        let dir = Dir::open_ambient_dir(requested_root, ambient_authority())
            .map_err(|e| format!("cannot open workspace root: {e}"))?;
        let root = std::path::absolute(requested_root)
            .map_err(|e| format!("cannot resolve workspace root: {e}"))?;
        let path_root = without_verbatim_prefix(&root);
        Ok(Self {
            root,
            path_root,
            dir: Arc::new(dir),
        })
    }

    pub fn relative_path(&self, path: impl AsRef<Path>) -> Result<PathBuf, String> {
        let path = path.as_ref();
        let relative = if path.is_absolute() {
            path.strip_prefix(&self.root)
                .or_else(|_| path.strip_prefix(&self.path_root))
                .map(Path::to_path_buf)
                .ok()
                .or_else(|| strip_prefix_windows(path, &self.root))
                .or_else(|| strip_prefix_windows(path, &self.path_root))
                .ok_or_else(|| self.outside(path))?
        } else {
            path.to_path_buf()
        };

        let mut normalized = PathBuf::new();
        for component in relative.components() {
            match component {
                Component::CurDir => {}
                Component::Normal(part) => normalized.push(part),
                Component::ParentDir if normalized.pop() => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(self.outside(path));
                }
            }
        }
        Ok(normalized)
    }

    pub fn dir(&self) -> &Dir {
        &self.dir
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn access_error(&self, path: &Path, error: std::io::Error) -> String {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            self.outside(path)
        } else {
            format!("cannot access '{}': {error}", path.display())
        }
    }

    fn outside(&self, path: &Path) -> String {
        format!(
            "refusing to access '{}': outside the workspace ({})",
            path.display(),
            self.root.display()
        )
    }
}

fn without_verbatim_prefix(path: &Path) -> PathBuf {
    let value = path.to_string_lossy();
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = value.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path.to_path_buf()
    }
}

#[cfg(windows)]
fn strip_prefix_windows(path: &Path, base: &Path) -> Option<PathBuf> {
    let path_components: Vec<_> = path.components().collect();
    let base_components: Vec<_> = base.components().collect();
    if path_components.len() < base_components.len()
        || !path_components
            .iter()
            .zip(&base_components)
            .all(|(left, right)| {
                left.as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
            })
    {
        return None;
    }

    Some(path_components[base_components.len()..].iter().collect())
}

#[cfg(not(windows))]
fn strip_prefix_windows(_path: &Path, _base: &Path) -> Option<PathBuf> {
    None
}

/// Resolves `path` against the current working directory and refuses any
/// path that would end up outside it (`..` escapes, absolute paths
/// elsewhere, or symlinks pointing outside the workspace). The path itself
/// does not need to exist yet — this only confines *where* it may land, so
/// it works for both reads and new-file writes.
pub fn resolve_in_workspace(path: &str) -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| format!("cannot resolve workspace root: {e}"))?;
    let cwd = cwd.canonicalize().unwrap_or(cwd);

    let candidate = std::path::Path::new(path);
    let mut absolute = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        cwd.join(candidate)
    };

    // A missing component prevents the filesystem from resolving later `..`
    // components, so normalize those lexically before finding an existing
    // ancestor. Existing paths are canonicalized as-is to preserve symlink
    // semantics.
    if !absolute.exists() {
        let mut normalized = PathBuf::new();
        for component in absolute.components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    normalized.pop();
                }
                Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                    normalized.push(component.as_os_str());
                }
            }
        }
        absolute = normalized;
    }

    // Walk up to the deepest ancestor that actually exists so it can be
    // canonicalized (resolving any symlinks); the remaining, not-yet-created
    // tail is re-appended lexically.
    let mut existing = absolute.clone();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    while !existing.exists() {
        let Some(name) = existing.file_name().map(|n| n.to_os_string()) else {
            break;
        };
        tail.push(name);
        if !existing.pop() {
            break;
        }
    }
    let mut resolved = existing.canonicalize().unwrap_or(existing);
    for part in tail.into_iter().rev() {
        if part == ".." {
            resolved.pop();
        } else if part != "." {
            resolved.push(part);
        }
    }

    if !resolved.starts_with(&cwd) {
        return Err(format!(
            "refusing to access '{path}': outside the workspace ({})",
            cwd.display()
        ));
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rejects_path_escaping_workspace() {
        let err = resolve_in_workspace("../../outside.txt").unwrap_err();
        assert!(
            err.contains("outside the workspace"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_rejects_path_escaping_workspace_through_missing_tail() {
        let err = resolve_in_workspace("cairn-definitely-missing/../../outside.txt").unwrap_err();
        assert!(
            err.contains("outside the workspace"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_allows_existing_file_inside_workspace() {
        let resolved = resolve_in_workspace("Cargo.toml").unwrap();
        assert!(resolved.ends_with("Cargo.toml"));
    }

    #[test]
    fn test_allows_new_file_in_new_subdirectory_inside_workspace() {
        let resolved =
            resolve_in_workspace("target/cairn_workspace_test_dir/new_file.txt").unwrap();
        assert!(resolved.ends_with("new_file.txt"));
    }

    #[test]
    fn test_rejects_absolute_path_outside_workspace() {
        let outside = if cfg!(windows) {
            "C:\\Windows\\System32\\drivers\\etc\\hosts"
        } else {
            "/etc/hosts"
        };
        let err = resolve_in_workspace(outside).unwrap_err();
        assert!(
            err.contains("outside the workspace"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn workspace_rejects_existing_and_missing_outside_paths_identically() {
        let workspace = Workspace::current().unwrap();
        let existing = workspace.root.parent().unwrap();
        let missing = workspace
            .root
            .parent()
            .unwrap()
            .join("cairn-definitely-missing-outside");

        let existing_error = workspace.relative_path(existing).unwrap_err();
        let missing_error = workspace.relative_path(missing).unwrap_err();
        assert!(existing_error.contains("outside the workspace"));
        assert!(missing_error.contains("outside the workspace"));
    }

    #[cfg(windows)]
    #[test]
    fn workspace_accepts_differently_cased_absolute_root() {
        let workspace = Workspace::current().unwrap();
        let differently_cased =
            PathBuf::from(workspace.root.to_string_lossy().to_ascii_uppercase());

        assert_eq!(
            workspace.relative_path(differently_cased).unwrap(),
            PathBuf::new()
        );
    }
}
