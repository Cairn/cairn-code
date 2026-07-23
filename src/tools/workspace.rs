use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions, Permissions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_name(sequence: u64) -> PathBuf {
    PathBuf::from(format!(".cairn-tmp-{}-{sequence}", std::process::id()))
}

fn next_temp_name(destination: &Path) -> PathBuf {
    loop {
        let candidate = temp_name(TEMP_COUNTER.fetch_add(1, Ordering::Relaxed));
        if candidate != destination {
            return candidate;
        }
    }
}

pub struct Target {
    parent: Dir,
    name: PathBuf,
    pub relative: PathBuf,
    pub previous: Option<String>,
    permissions: Option<Permissions>,
}

/// Acquires the workspace capability once, then resolves every component from
/// that exact handle without following directory or final-component symlinks.
pub fn acquire(path: &str, create_parents: bool, require_existing: bool) -> Result<Target, String> {
    let workspace = Workspace::current()?;
    let relative = workspace.relative_path(path)?;
    if relative.as_os_str().is_empty() {
        return Err(format!(
            "refusing to access '{path}': invalid workspace file path"
        ));
    }
    let mut parent = workspace
        .dir()
        .try_clone()
        .map_err(|e| format!("cannot clone workspace root: {e}"))?;
    if let Some(path_parent) = relative.parent() {
        for component in path_parent.components() {
            let Component::Normal(part) = component else {
                continue;
            };
            if create_parents {
                match parent.create_dir(part) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(format!("mkdir: {e}")),
                }
            }
            parent = parent
                .open_dir_nofollow(part)
                .map_err(|e| format!("open directory: {e}"))?;
        }
    }
    let name: PathBuf = relative
        .file_name()
        .ok_or("invalid workspace file path")?
        .into();
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let (previous, permissions) = match parent.open_with(&name, &options) {
        Ok(mut file) => {
            let permissions = file
                .metadata()
                .map_err(|e| format!("metadata error: {e}"))?
                .permissions();
            let mut content = String::new();
            file.read_to_string(&mut content)
                .map_err(|e| format!("read error: {e}"))?;
            (Some(content), Some(permissions))
        }
        Err(e) if !require_existing && e.kind() == std::io::ErrorKind::NotFound => (None, None),
        Err(e) => return Err(format!("open file: {e}")),
    };
    Ok(Target {
        parent,
        name,
        relative,
        previous,
        permissions,
    })
}

pub fn atomic_replace(target: &Target, content: &str) -> Result<(), String> {
    let (temp, mut file) = loop {
        let temp = next_temp_name(&target.name);
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        match target.parent.open_with(&temp, &options) {
            Ok(file) => break (temp, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("temp create failed: {error}")),
        }
    };
    let result = (|| {
        if let Some(permissions) = &target.permissions {
            file.set_permissions(permissions.clone())
                .map_err(|e| format!("temp permissions failed: {e}"))?;
        }
        file.write_all(content.as_bytes())
            .map_err(|e| format!("write error: {e}"))?;
        file.sync_all().map_err(|e| format!("sync error: {e}"))?;
        drop(file);
        target
            .parent
            .rename(&temp, &target.parent, &target.name)
            .map_err(|e| format!("replace error: {e}"))
    })();
    if result.is_err() {
        let _ = target.parent.remove_file(&temp);
    }
    result
}

pub fn restore(relative: &Path, previous: Option<&str>) -> Result<(), String> {
    let path = relative.to_str().ok_or("history path is not valid UTF-8")?;
    if let Some(content) = previous {
        let target = acquire(path, true, false)?;
        atomic_replace(&target, content)
    } else {
        let target = acquire(path, false, false)?;
        if target.previous.is_none() {
            return Ok(());
        }
        target
            .parent
            .remove_file(&target.name)
            .map_err(|e| format!("undo remove failed: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_rejects_escape_and_accepts_dot() {
        let workspace = Workspace::current().unwrap();
        let err = workspace.relative_path("../../outside.txt").unwrap_err();
        assert!(
            err.contains("outside the workspace"),
            "unexpected error: {err}"
        );
        assert_eq!(
            workspace.relative_path("./Cargo.toml").unwrap(),
            Path::new("Cargo.toml")
        );
    }

    #[test]
    fn workspace_rejects_traversal_after_nonexistent_prefix() {
        let workspace = Workspace::current().unwrap();
        let err = workspace
            .relative_path("target/cairn_workspace_missing_prefix/../../../outside.txt")
            .unwrap_err();
        assert!(
            err.contains("outside the workspace"),
            "unexpected error: {err}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn workspace_rejects_mixed_separator_windows_traversal() {
        let workspace = Workspace::current().unwrap();
        let error = workspace
            .relative_path(r"target/cairn_workspace_missing_prefix\../..\../outside.txt")
            .unwrap_err();
        assert!(
            error.contains("outside the workspace"),
            "unexpected error: {error}"
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

    #[test]
    fn temporary_name_never_matches_destination() {
        let sequence = TEMP_COUNTER.load(Ordering::Relaxed);
        let destination = temp_name(sequence);
        assert_ne!(next_temp_name(&destination), destination);
    }
}
