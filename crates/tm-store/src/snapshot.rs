//! The read-only Snapshot: a copy of the store published after writes, for
//! readers that cannot write any file, such as a sandboxed Agent (ADR 0001).

use std::fmt::Write as _;
use std::fs::{self, File};
use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use tm_core::Scheme;
use uuid::Uuid;

use crate::{Store, StoreError, SystemClock, schema};

/// Held while a Snapshot is copied and renamed, so publishes land in the
/// order their writes did and a slow one never replaces a newer copy.
const LOCK: &str = "snapshot.lock";

/// The prefix of a Snapshot being copied; one left behind was interrupted.
const PARTIAL: &str = ".snapshot-";

impl Store {
    /// Opens the Snapshot at `path` of Workspace `workspace`, read-only and
    /// immutable: SQLite takes no lock and creates no file, so a reader that
    /// can write nothing opens it. Nothing is set up or rebuilt, so a
    /// Snapshot this build cannot read as it is is refused.
    ///
    /// # Errors
    ///
    /// As [`Store::open_existing`] for a file that is not this Workspace's
    /// store, or [`StoreError::SnapshotVersion`] when it was published by a
    /// build with other projections.
    pub fn open_snapshot(path: &Path, workspace: Uuid) -> Result<Self, StoreError> {
        let conn = Connection::open_with_flags(
            format!("{}?immutable=1", uri(path)),
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Self::check(&conn, path, workspace)?;
        let found: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if found != schema::PROJECTION_VERSION {
            return Err(StoreError::SnapshotVersion {
                found,
                expected: schema::PROJECTION_VERSION,
            });
        }
        // A large sort can spill to a temporary file, which a sandbox that
        // denies every write refuses.
        conn.execute_batch("PRAGMA temp_store = MEMORY")?;
        Self::collate(&conn)?;
        let store = Self {
            conn,
            clock: Box::new(SystemClock),
            scheme: Scheme::built_in(),
            opened: false,
            last_write: None,
        };
        store.check_log_columns()?;
        Ok(store)
    }

    /// Publishes this store as the Snapshot at `path`: copied beside it, then
    /// renamed over it, so a reader finds the old copy or the new one, never
    /// part of either. Then checkpoints the log, a natural point to make
    /// recent writes durable.
    ///
    /// # Errors
    ///
    /// [`StoreError::Snapshot`] when the lock or the rename fails, or any SQLite
    /// failure copying the store.
    pub fn publish_snapshot(&self, path: &Path) -> Result<(), StoreError> {
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        let failed = |path: &Path| {
            let path = path.to_path_buf();
            move |source| StoreError::Snapshot { path, source }
        };
        let lock_path = dir.join(LOCK);
        let lock = File::create(&lock_path).map_err(failed(&lock_path))?;
        lock.lock().map_err(failed(&lock_path))?;
        remove_partial(dir);
        let partial = dir.join(format!("{PARTIAL}{}.db", Uuid::now_v7()));
        let copied = self
            .conn
            .execute("VACUUM INTO ?1", [uri(&partial)])
            .map_err(StoreError::from)
            .and_then(|_| fs::rename(&partial, path).map_err(failed(path)));
        if copied.is_err() {
            let _ = fs::remove_file(&partial);
        }
        drop(lock);
        copied?;
        // PASSIVE waits for no reader, so a write never stalls on one; the
        // log can then outgrow what one checkpoint copies, until a later one.
        self.conn
            .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()))?;
        Ok(())
    }
}

/// Removes copies an interrupted publish left; the lock is held, so none is
/// live.
fn remove_partial(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(PARTIAL))
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Percent-encoded so `?`, `#` and non-UTF-8 bytes survive; the empty
/// authority keeps a path starting `//` from reading as a host.
fn uri(path: &Path) -> String {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut out = String::from("file://");
    for byte in path.as_os_str().as_encoded_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(*byte));
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_uri_encodes_what_a_uri_would_read_as_syntax() {
        assert_eq!(
            uri(Path::new("/a b/c?d#e%f/ü.db")),
            "file:///a%20b/c%3Fd%23e%25f/%C3%BC.db"
        );
        assert_eq!(uri(Path::new("//host/x")), "file:////host/x");
        assert!(uri(Path::new("rel/x.db")).starts_with("file:///"));
    }
}
