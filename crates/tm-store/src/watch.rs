//! Noticing another process's writes, for `--watch` and the TUI (ADR 0018).

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rusqlite::{Connection, OpenFlags};

use crate::StoreError;

/// Says whether the store, or the Snapshot, has changed since it last asked.
/// It holds no read transaction between asks, so writers' checkpoints pass.
pub struct Watch {
    path: PathBuf,
    seen: Seen,
    /// When the first Claim held at the last read lapses: no write marks a
    /// lapse, so the Watch reports one itself.
    lapses: Option<i64>,
    /// A write seen whose lapse read then failed, reported by the next call.
    unreported: bool,
}

enum Seen {
    /// `PRAGMA data_version` is per connection, so one connection asks until
    /// the file at the path is another; one opened on a deleted file would
    /// never see a write again.
    Store {
        conn: Connection,
        version: i64,
        file: (u64, u64),
    },
    /// A publish renames a new file over the Snapshot, which its metadata
    /// shows; an `immutable=1` reader's `data_version` never moves.
    Snapshot { file: (u64, u64, SystemTime, u64) },
}

impl Watch {
    /// Watches the store at `path`, read-only.
    ///
    /// # Errors
    ///
    /// Any SQLite failure opening it, or [`StoreError::StoreGone`] when its
    /// metadata cannot be read.
    pub fn store(path: &Path) -> Result<Self, StoreError> {
        let mut watch = Self {
            path: path.to_path_buf(),
            seen: opened(path)?,
            lapses: None,
            unreported: false,
        };
        watch.lapses = watch.next_lapse(now_ms())?;
        Ok(watch)
    }

    /// Watches the Snapshot at `path` for a publish replacing it.
    ///
    /// # Errors
    ///
    /// [`StoreError::Snapshot`] when its metadata cannot be read,
    /// [`StoreError::SnapshotVersion`] when another tm published it, or any
    /// SQLite failure reading its Claims.
    pub fn snapshot(path: &Path) -> Result<Self, StoreError> {
        let (metadata, modified) = metadata(path)?;
        let mut watch = Self {
            path: path.to_path_buf(),
            seen: Seen::Snapshot {
                file: (metadata.dev(), metadata.ino(), modified, metadata.len()),
            },
            lapses: None,
            unreported: false,
        };
        watch.lapses = watch.next_lapse(now_ms())?;
        Ok(watch)
    }

    /// Whether a write has landed, or a held Claim has lapsed, since the
    /// last call or since the Watch was made. A store replaced by another
    /// file counts as a write.
    ///
    /// # Errors
    ///
    /// As [`Watch::store`] or [`Watch::snapshot`], including when the file
    /// is gone.
    pub fn changed(&mut self) -> Result<bool, StoreError> {
        let seen = match &mut self.seen {
            Seen::Store {
                conn,
                version,
                file,
            } => {
                if store_id(&self.path)? == *file {
                    let now = data_version(conn)?;
                    std::mem::replace(version, now) != now
                } else {
                    self.seen = opened(&self.path)?;
                    true
                }
            }
            Seen::Snapshot { file } => {
                let (metadata, modified) = metadata(&self.path)?;
                let now = (metadata.dev(), metadata.ino(), modified, metadata.len());
                std::mem::replace(file, now) != now
            }
        };
        let wrote = seen || self.unreported;
        let now = now_ms();
        let lapsed = self.lapses.is_some_and(|at| at <= now);
        if wrote || lapsed {
            self.unreported = wrote;
            self.lapses = self.next_lapse(now)?;
            self.unreported = false;
        }
        Ok(wrote || lapsed)
    }

    /// When the first Claim still held at `now` lapses; none when no held
    /// Claim expires.
    fn next_lapse(&self, now: i64) -> Result<Option<i64>, StoreError> {
        const FIRST: &str = "SELECT MIN(expires_ms) FROM claims WHERE expires_ms > ?1";
        let first = |conn: &Connection| conn.query_row(FIRST, [now], |row| row.get(0));
        Ok(match &self.seen {
            Seen::Store { conn, .. } => first(conn)?,
            Seen::Snapshot { .. } => {
                let conn = Connection::open_with_flags(
                    format!("{}?immutable=1", crate::snapshot::uri(&self.path)),
                    OpenFlags::SQLITE_OPEN_READ_ONLY
                        | OpenFlags::SQLITE_OPEN_URI
                        | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )?;
                let found: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
                if found != crate::schema::PROJECTION_VERSION {
                    return Err(StoreError::SnapshotVersion {
                        found,
                        expected: crate::schema::PROJECTION_VERSION,
                    });
                }
                first(&conn)?
            }
        })
    }
}

/// Opens the file at `path`, again if another replaced it meanwhile, so the
/// identity kept is the connection's.
fn opened(path: &Path) -> Result<Seen, StoreError> {
    loop {
        let file = store_id(path)?;
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let version = data_version(&conn)?;
        if store_id(path)? == file {
            return Ok(Seen::Store {
                conn,
                version,
                file,
            });
        }
    }
}

/// Wall-clock milliseconds, as a Claim's expiry is written.
fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

fn data_version(conn: &Connection) -> Result<i64, StoreError> {
    Ok(conn.query_row("PRAGMA data_version", [], |row| row.get(0))?)
}

fn store_id(path: &Path) -> Result<(u64, u64), StoreError> {
    let metadata = fs::metadata(path).map_err(|source| StoreError::StoreGone {
        path: path.to_path_buf(),
        source,
    })?;
    Ok((metadata.dev(), metadata.ino()))
}

fn metadata(path: &Path) -> Result<(fs::Metadata, SystemTime), StoreError> {
    let failed = |source| StoreError::Snapshot {
        path: path.to_path_buf(),
        source,
    };
    let metadata = fs::metadata(path).map_err(failed)?;
    let modified = metadata.modified().map_err(failed)?;
    Ok((metadata, modified))
}
