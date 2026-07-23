use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions, Permissions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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

fn workspace_relative(path: &str) -> Result<PathBuf, String> {
    let ordinary = std::env::current_dir().map_err(|e| format!("cannot resolve workspace root: {e}"))?;
    let canonical = ordinary.canonicalize().map_err(|e| format!("cannot resolve workspace root: {e}"))?;
    let candidate = Path::new(path);
    let relative = if candidate.is_absolute() {
        candidate.strip_prefix(&ordinary).or_else(|_| candidate.strip_prefix(&canonical))
            .map_err(|_| format!("refusing to access '{path}': outside the workspace"))?
            .to_path_buf()
    } else {
        candidate.to_path_buf()
    };
    let mut clean = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::ParentDir => return Err(format!("refusing to access '{path}': outside the workspace (parent-directory traversal is not allowed)")),
            Component::RootDir | Component::Prefix(_) => return Err(format!("refusing to access '{path}': invalid workspace file path")),
        }
    }
    if clean.as_os_str().is_empty() {
        return Err(format!("refusing to access '{path}': invalid workspace file path"));
    }
    Ok(clean)
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
    let root = Dir::open_ambient_dir(".", ambient_authority())
        .map_err(|e| format!("cannot open workspace root: {e}"))?;
    let relative = workspace_relative(path)?;
    let mut parent = root;
    if let Some(path_parent) = relative.parent() {
        for component in path_parent.components() {
            let Component::Normal(part) = component else { continue };
            if create_parents {
                match parent.create_dir(part) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(format!("mkdir: {e}")),
                }
            }
            parent = parent.open_dir_nofollow(part).map_err(|e| format!("open directory: {e}"))?;
        }
    }
    let name: PathBuf = relative.file_name().ok_or("invalid workspace file path")?.into();
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let (previous, permissions) = match parent.open_with(&name, &options) {
        Ok(mut file) => {
            let permissions = file.metadata().map_err(|e| format!("metadata error: {e}"))?.permissions();
            let mut content = String::new();
            file.read_to_string(&mut content).map_err(|e| format!("read error: {e}"))?;
            (Some(content), Some(permissions))
        }
        Err(e) if !require_existing && e.kind() == std::io::ErrorKind::NotFound => (None, None),
        Err(e) => return Err(format!("open file: {e}")),
    };
    Ok(Target { parent, name, relative, previous, permissions })
}

pub fn atomic_replace(target: &Target, content: &str) -> Result<(), String> {
    let (temp, mut file) = loop {
        let temp = next_temp_name(&target.name);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true).follow(FollowSymlinks::No);
        match target.parent.open_with(&temp, &options) {
            Ok(file) => break (temp, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("temp create failed: {error}")),
        }
    };
    let result = (|| {
        if let Some(permissions) = &target.permissions {
            file.set_permissions(permissions.clone()).map_err(|e| format!("temp permissions failed: {e}"))?;
        }
        file.write_all(content.as_bytes()).map_err(|e| format!("write error: {e}"))?;
        file.sync_all().map_err(|e| format!("sync error: {e}"))?;
        drop(file);
        target.parent.rename(&temp, &target.parent, &target.name).map_err(|e| format!("replace error: {e}"))
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
        if target.previous.is_none() { return Ok(()); }
        target.parent.remove_file(&target.name).map_err(|e| format!("undo remove failed: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_escape_and_accepts_dot() {
        assert!(workspace_relative("../../outside.txt").unwrap_err().contains("outside"));
        assert_eq!(workspace_relative("./Cargo.toml").unwrap(), Path::new("Cargo.toml"));
    }

    #[test]
    fn rejects_traversal_after_nonexistent_prefix() {
        let error = workspace_relative(
            "target/cairn_workspace_missing_prefix/../../../outside.txt",
        )
        .unwrap_err();
        assert!(error.contains("outside the workspace"), "unexpected error: {error}");
    }

    #[cfg(windows)]
    #[test]
    fn rejects_mixed_separator_windows_traversal() {
        let error = workspace_relative(
            r"target/cairn_workspace_missing_prefix\../..\../outside.txt",
        )
        .unwrap_err();
        assert!(error.contains("outside the workspace"), "unexpected error: {error}");
    }

    #[test]
    fn accepts_ordinary_absolute_workspace_path() {
        let absolute = std::env::current_dir().unwrap().join("target/new.txt");
        assert_eq!(workspace_relative(absolute.to_str().unwrap()).unwrap(), Path::new("target/new.txt"));
    }

    #[test]
    fn rejects_absolute_path_outside_workspace() {
        let outside = if cfg!(windows) {
            r"C:\Windows\System32\drivers\etc\hosts"
        } else {
            "/etc/hosts"
        };
        let error = workspace_relative(outside).unwrap_err();
        assert!(error.contains("outside the workspace"), "unexpected error: {error}");
    }

    #[test]
    fn temporary_name_never_matches_destination() {
        let sequence = TEMP_COUNTER.load(Ordering::Relaxed);
        let destination = temp_name(sequence);
        assert_ne!(next_temp_name(&destination), destination);
    }
}
