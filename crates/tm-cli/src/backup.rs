//! The day's backup of a Workspace, and the collection of old ones (ADR 0015).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use jiff::ToSpan as _;
use jiff::civil::Date;
use serde::Deserialize;
use uuid::Uuid;

use crate::error::CliError;
use crate::export;
use crate::here::Here;

/// `[backups]` in `config.toml` in tm's home.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Policy {
    /// Whether a write takes the day's backup.
    pub(crate) enabled: bool,
    pub(crate) keep_days: u32,
    pub(crate) keep_weeks: u32,
    pub(crate) keep_months: u32,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            enabled: true,
            keep_days: 14,
            keep_weeks: 8,
            keep_months: 12,
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    backups: Policy,
}

/// The policy `home`'s `config.toml` sets, with the defaults where it sets
/// none; a warning when it cannot be read.
pub(crate) fn policy(home: &Path) -> Result<Policy, String> {
    let path = home.join("config.toml");
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Policy::default()),
        Err(err) => return Err(format!("{}: {err}", path.display())),
    };
    toml::from_str::<Config>(&text)
        .map(|config| config.backups)
        .map_err(|err| format!("{}: {}", path.display(), err.message()))
}

fn dir(home: &Path, workspace: Uuid) -> PathBuf {
    home.join("exports").join(workspace.to_string())
}

/// A Workspace's exports directory, locked while this is held, so one export
/// or collection runs in it at a time.
pub(crate) struct Exports {
    dir: PathBuf,
    _lock: fs::File,
}

impl Exports {
    pub(crate) fn lock(home: &Path, workspace: Uuid) -> Result<Self, CliError> {
        let dir = dir(home, workspace);
        fs::create_dir_all(&dir).map_err(|err| export::io_error(&dir, &err))?;
        let path = dir.join(".lock");
        let lock = fs::File::create(&path).map_err(|err| export::io_error(&path, &err))?;
        lock.lock().map_err(|err| export::io_error(&path, &err))?;
        Ok(Self { dir, _lock: lock })
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    pub(crate) fn file(&self, date: Date) -> PathBuf {
        self.dir.join(format!("{date}.jsonl"))
    }

    /// Removes staging directories an export left when it was killed, since
    /// any still writing holds the lock, and, with a `policy`, the backups it
    /// does not keep; a warning for each that could not be removed. Nothing is
    /// removed through a symbolic link, which may lead out of tm's home.
    pub(crate) fn collect(&self, today: Date, policy: Option<&Policy>) -> Vec<String> {
        let parent = self.dir.parent().unwrap_or(&self.dir);
        for dir in [parent, &self.dir] {
            if fs::symlink_metadata(dir).is_ok_and(|meta| meta.file_type().is_symlink()) {
                return vec![format!(
                    "old backups were not removed: {} is a symbolic link, and tm removes nothing through one",
                    dir.display()
                )];
            }
        }
        let failed = |path: &Path, err: &io::Error| {
            format!("old backups were not removed: {}: {err}", path.display())
        };
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(err) => return vec![failed(&self.dir, &err)],
        };
        let mut warnings = Vec::new();
        let mut backups = Vec::new();
        for entry in entries {
            let entry = match entry.and_then(|entry| Ok((entry.file_type()?, entry))) {
                Ok(entry) => entry,
                Err(err) => {
                    warnings.push(failed(&self.dir, &err));
                    continue;
                }
            };
            let (kind, entry) = entry;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if kind.is_dir() && name.starts_with(".tm-import-") {
                if let Err(err) = fs::remove_dir_all(entry.path()) {
                    warnings.push(failed(&entry.path(), &err));
                }
            } else if let Some(date) = kind.is_file().then(|| date_of(name)).flatten() {
                backups.push((date, entry.path()));
            }
        }
        let Some(policy) = policy else {
            return warnings;
        };
        let dates: Vec<Date> = backups.iter().map(|(date, _)| *date).collect();
        let kept = keep(&dates, today, policy);
        for (date, path) in backups {
            if !kept.contains(&date)
                && let Err(err) = fs::remove_file(&path)
            {
                warnings.push(failed(&path, &err));
            }
        }
        warnings
    }
}

/// A policy that could not be read, which keeps every backup.
pub(crate) fn unread(warning: &str) -> String {
    format!("{warning}; no old backup was removed")
}

/// The date of a backup by its file name, for the names tm writes only.
fn date_of(name: &str) -> Option<Date> {
    let stem = name.strip_suffix(".jsonl")?;
    let date = Date::strptime("%Y-%m-%d", stem).ok()?;
    (date.to_string() == stem).then_some(date)
}

/// The dates `policy` keeps of `dates`: each in the last `keep_days` days,
/// the newest of each of the last `keep_weeks` ISO weeks and of each of the
/// last `keep_months` months, the newest up to `today`, and each after it,
/// from a clock that ran ahead.
pub(crate) fn keep(dates: &[Date], today: Date, policy: &Policy) -> BTreeSet<Date> {
    let monday = |date: Date| date - i64::from(date.weekday().to_monday_zero_offset()).days();
    let month = |date: Date| i32::from(date.year()) * 12 + i32::from(date.month());
    let (past, future): (Vec<Date>, Vec<Date>) = dates.iter().partition(|&&date| date <= today);
    let mut kept: BTreeSet<Date> = future.into_iter().collect();
    let mut weeks = BTreeMap::new();
    let mut months = BTreeMap::new();
    for &date in &past {
        let days = (today - date).get_days();
        if days < i32::try_from(policy.keep_days).unwrap_or(i32::MAX) {
            kept.insert(date);
        }
        let week = monday(date);
        if (monday(today) - week).get_days() / 7
            < i32::try_from(policy.keep_weeks).unwrap_or(i32::MAX)
        {
            let newest = weeks.entry(week).or_insert(date);
            *newest = (*newest).max(date);
        }
        let months_ago = month(today) - month(date);
        if months_ago < i32::try_from(policy.keep_months).unwrap_or(i32::MAX) {
            let newest = months.entry(month(date)).or_insert(date);
            *newest = (*newest).max(date);
        }
    }
    kept.extend(weeks.into_values());
    kept.extend(months.into_values());
    kept.extend(past.iter().max());
    kept
}

/// Takes the day's backup of `here`'s Workspace when it has none, then
/// collects old ones; the warnings of what failed. Run after a write that
/// added to the log, so the log always differs from the last backup's.
pub(crate) fn daily(here: &Here) -> Vec<String> {
    let today = jiff::Zoned::now().date();
    let file = dir(&here.home, here.workspace.id).join(format!("{today}.jsonl"));
    if held(&file) == Held::Backup {
        return Vec::new();
    }
    let policy = policy(&here.home);
    if policy.as_ref().is_ok_and(|policy| !policy.enabled) {
        return Vec::new();
    }
    let mut warnings = match back_up(here, today) {
        Ok(Some(exports)) => exports.collect(today, policy.as_ref().ok()),
        Ok(None) => Vec::new(),
        Err(err) => vec![format!("today's backup was not written: {}", err.message)],
    };
    warnings.extend(policy.err().as_deref().map(unread));
    warnings
}

#[derive(Debug, PartialEq, Eq)]
enum Held {
    Backup,
    /// Nothing, or an empty file, which a backup replaces.
    Missing,
    /// Something tm did not write there, which it leaves alone.
    Other,
}

fn held(path: &Path) -> Held {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && meta.len() > 0 => Held::Backup,
        Ok(meta) if meta.is_file() => Held::Missing,
        Err(err) if err.kind() == io::ErrorKind::NotFound => Held::Missing,
        Ok(_) | Err(_) => Held::Other,
    }
}

/// Writes today's backup unless another process wrote it first, and returns
/// the still-locked directory for collection.
fn back_up(here: &Here, today: Date) -> Result<Option<Exports>, CliError> {
    let exports = Exports::lock(&here.home, here.workspace.id)?;
    let file = exports.file(today);
    match held(&file) {
        Held::Backup => return Ok(None),
        Held::Missing => {}
        Held::Other => {
            return Err(CliError::new(
                "io",
                format!(
                    "{} is not a file tm wrote, so it was left alone",
                    file.display()
                ),
            ));
        }
    }
    export::stage(here, exports.dir())?.persist(&file)?;
    Ok(Some(exports))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(text: &str) -> Date {
        text.parse().unwrap()
    }

    #[test]
    fn collection_keeps_each_tier_of_daily_backups() {
        let today = date("2026-10-22");
        // Oldest first, so the newest of a week or month is not the first seen.
        let dates: Vec<Date> = (0..400).rev().map(|days| today - days.days()).collect();
        let kept = keep(&dates, today, &Policy::default());
        let mut expected: BTreeSet<Date> = (0..14).map(|days| today - days.days()).collect();
        for newest in [
            // Sundays, ending the last 8 ISO weeks.
            "2026-10-04",
            "2026-09-27",
            "2026-09-20",
            "2026-09-13",
            "2026-09-06",
            // Month ends, for the last 12 months.
            "2026-09-30",
            "2026-08-31",
            "2026-07-31",
            "2026-06-30",
            "2026-05-31",
            "2026-04-30",
            "2026-03-31",
            "2026-02-28",
            "2026-01-31",
            "2025-12-31",
            "2025-11-30",
        ] {
            expected.insert(date(newest));
        }
        assert_eq!(kept, expected);
    }

    #[test]
    fn collection_keeps_the_newest_of_a_week_or_month_however_sparse() {
        let today = date("2026-10-22");
        let policy = Policy {
            keep_days: 0,
            ..Policy::default()
        };
        let dates = [
            date("2026-10-20"),
            date("2026-10-19"),
            date("2026-10-02"),
            date("2026-10-01"),
        ];
        let kept = keep(&dates, today, &policy);
        let expected = [date("2026-10-20"), date("2026-10-02")];
        assert_eq!(kept, BTreeSet::from(expected));
    }

    #[test]
    fn collection_always_keeps_the_newest_backup_and_any_dated_later() {
        let none = Policy {
            enabled: true,
            keep_days: 0,
            keep_weeks: 0,
            keep_months: 0,
        };
        let dates = [date("2019-03-01"), date("2020-01-01"), date("2999-01-01")];
        let kept = keep(&dates, date("2026-10-22"), &none);
        assert_eq!(
            kept,
            BTreeSet::from([date("2020-01-01"), date("2999-01-01")])
        );
    }

    #[test]
    fn only_tm_backup_names_have_a_date() {
        assert_eq!(date_of("2026-10-22.jsonl"), Some(date("2026-10-22")));
        for name in [
            "2026-10-22.json",
            "2026-10-22.jsonl.bak",
            "2026-1-2.jsonl",
            "notes.jsonl",
            "2026-13-01.jsonl",
        ] {
            assert_eq!(date_of(name), None, "{name}");
        }
    }

    #[test]
    fn a_config_sets_what_it_names_and_refuses_unknown_keys() {
        let root = std::env::temp_dir().join(format!("tm-policy-{}", Uuid::now_v7()));
        fs::create_dir_all(&root).unwrap();
        let missing = policy(&root);
        fs::write(root.join("config.toml"), "[backups]\nkeep_days = 3\n").unwrap();
        let partial = policy(&root);
        fs::write(root.join("config.toml"), "[backups]\nkeep_day = 3\n").unwrap();
        let key = policy(&root);
        fs::write(root.join("config.toml"), "[backup]\nenabled = false\n").unwrap();
        let table = policy(&root);
        let _ = fs::remove_dir_all(&root);
        assert_eq!(missing, Ok(Policy::default()));
        assert_eq!(
            partial,
            Ok(Policy {
                keep_days: 3,
                ..Policy::default()
            })
        );
        assert!(key.is_err_and(|warning| warning.contains("keep_day")));
        assert!(table.is_err_and(|warning| warning.contains("backup")));
    }

    #[test]
    fn only_a_nonempty_file_counts_as_the_days_backup() {
        let root = std::env::temp_dir().join(format!("tm-held-{}", Uuid::now_v7()));
        fs::create_dir_all(root.join("dir")).unwrap();
        fs::write(root.join("full"), "{}\n").unwrap();
        fs::write(root.join("empty"), "").unwrap();
        std::os::unix::fs::symlink(root.join("gone"), root.join("dangling")).unwrap();
        std::os::unix::fs::symlink(root.join("full"), root.join("link")).unwrap();
        let found: Vec<Held> = ["full", "empty", "missing", "dir", "dangling", "link"]
            .iter()
            .map(|name| held(&root.join(name)))
            .collect();
        let _ = fs::remove_dir_all(&root);
        assert_eq!(
            found,
            [
                Held::Backup,
                Held::Missing,
                Held::Missing,
                Held::Other,
                Held::Other,
                Held::Other
            ]
        );
    }
}
