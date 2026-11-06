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
        Ok(Self {
            path: path.to_path_buf(),
            seen: opened(path)?,
        })
    }

    /// Watches the Snapshot at `path` for a publish replacing it.
    ///
    /// # Errors
    ///
    /// [`StoreError::Snapshot`] when its metadata cannot be read.
    pub fn snapshot(path: &Path) -> Result<Self, StoreError> {
        let (metadata, modified) = metadata(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            seen: Seen::Snapshot {
                file: (metadata.dev(), metadata.ino(), modified, metadata.len()),
            },
        })
    }

    /// Whether a write has landed since the last call, or since the Watch
    /// was made. A store replaced by another file counts as a write.
    ///
    /// # Errors
    ///
    /// As [`Watch::store`] or [`Watch::snapshot`], including when the file
    /// is gone.
    pub fn changed(&mut self) -> Result<bool, StoreError> {
        match &mut self.seen {
            Seen::Store {
                conn,
                version,
                file,
            } => {
                if store_id(&self.path)? != *file {
                    self.seen = opened(&self.path)?;
                    return Ok(true);
                }
                let now = data_version(conn)?;
                Ok(std::mem::replace(version, now) != now)
            }
            Seen::Snapshot { file } => {
                let (metadata, modified) = metadata(&self.path)?;
                let now = (metadata.dev(), metadata.ino(), modified, metadata.len());
                Ok(std::mem::replace(file, now) != now)
            }
        }
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
