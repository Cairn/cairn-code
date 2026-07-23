use std::cell::RefCell;
use std::path::PathBuf;

/// In-process stack of file snapshots taken before mutating writes
/// (`file_edit` / `file_write`). `file_undo` pops the most recent entry and
/// restores the previous content (or deletes a file that did not exist).
///
/// Thread-local so the agent loop (single worker thread) sees a coherent
/// stack, and so unit tests running in parallel do not clobber each other.
struct Entry {
    path: PathBuf,
    label: String,
    /// `None` means the file did not exist before the write (undo = delete).
    previous: Option<String>,
}

const MAX_ENTRIES: usize = 64;

thread_local! {
    static STACK: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
}

fn push(entry: Entry) {
    STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        if stack.len() >= MAX_ENTRIES {
            stack.remove(0);
        }
        stack.push(entry);
    });
}

/// Snapshot known previous content (avoids a second disk read when the
/// caller already loaded the file, e.g. `file_edit`).
pub fn record_snapshot(path: PathBuf, label: &str, previous: Option<String>) {
    push(Entry {
        path,
        label: label.to_string(),
        previous,
    });
}

/// Restore the most recent mutation. Returns a short status string.
pub fn undo_last() -> Result<String, String> {
    let entry = STACK.with(|stack| stack.borrow_mut().pop()).ok_or_else(|| {
        "Nothing to undo. Only changes made with file_edit/file_write in this process can be undone.".to_string()
    })?;

    let result = match entry.previous.as_deref() {
        Some(content) => super::workspace::restore(&entry.path, Some(content))
            .map(|()| format!("Restored previous contents of {}", entry.label)),
        None => super::workspace::restore(&entry.path, None)
            .map(|()| format!("Removed newly created file {}", entry.label)),
    };
    if result.is_err() {
        push(entry);
    }
    result
}

#[cfg(test)]
pub fn clear() {
    STACK.with(|stack| stack.borrow_mut().clear());
}
