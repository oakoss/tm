//! `tm setup`: tm's hooks written into an agent harness's settings for the
//! repo, shown as a diff and written only on a person's yes (ADR 0014), and
//! taken out again by `--undo`. A handler is tm's when its command is one tm
//! writes, so one added by hand with the same command counts too.

use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{Serialize, SerializeMap as _, SerializeSeq as _, Serializer};
use serde_json::{Number, json};
use tm_store::workspace;

use crate::diff;
use crate::error::CliError;
use crate::hook::Harness;
use crate::identity;
use crate::instructions;
use crate::output::Output;
use crate::prompt::{ask, human, print_now};

/// The repo file `harness` reads its hooks from.
fn settings(harness: Harness) -> &'static str {
    match harness {
        Harness::Claude => ".claude/settings.json",
        Harness::Codex => ".codex/hooks.json",
    }
}

pub(crate) struct Setup {
    pub(crate) harness: Harness,
    pub(crate) undo: bool,
    pub(crate) yes: bool,
}

fn refused(message: impl Into<String>) -> CliError {
    CliError::new("setup_refused", message)
}

fn io_failed(path: &Path, err: &io::Error) -> CliError {
    CliError::new("io", format!("{}: {err}", path.display()))
        .with("path", path.display().to_string())
}

/// Adds tm's hooks, or with `undo` removes them, after showing the diff.
/// Nothing to change writes nothing and asks nothing.
pub(crate) fn run(setup: &Setup, json: bool) -> Result<Output, CliError> {
    let cwd = std::env::current_dir()
        .map_err(|err| CliError::new("io", format!("working directory: {err}")))?;
    let root = workspace::repo_root(&cwd)?.ok_or_else(|| {
        refused("tm setup runs inside a git repository; none contains this directory")
    })?;
    let file = settings(setup.harness);
    let path = root.join(file);
    let before = read(&path)?;
    let mut settings = parse(before.as_deref(), file)?;
    let handlers = handlers(setup.harness);
    let changed = if setup.undo {
        remove(&mut settings, &handlers, file)?
    } else {
        add(&mut settings, &handlers, file)?
    };
    if changed.is_empty() {
        let mut output = unchanged(setup.undo, file);
        if !setup.undo {
            output.warnings.extend(inline_hooks(&root, setup.harness));
        }
        return Ok(output);
    }
    writable(&path, file)?;
    let emptied = setup.undo && settings == Json::Object(Vec::new());
    let after = (!emptied).then(|| render(&settings, before.as_deref()));
    let shown = diff::unified(file, before.as_deref(), after.as_deref().unwrap_or(""));
    confirmed(&shown, emptied, setup, json)?;
    // The file may have been edited while tm asked; writing the text
    // computed before would lose that edit.
    if read(&path)? != before {
        return Err(refused(format!(
            "{file} changed while tm asked; nothing written"
        )));
    }
    writable(&path, file)?;
    match &after {
        Some(text) => {
            let create = before.is_none();
            let dir = path.parent().unwrap_or(&root).to_path_buf();
            let made_dir = create && !dir.exists();
            if made_dir {
                fs::create_dir_all(&dir).map_err(|err| io_failed(&dir, &err))?;
            }
            instructions::write(&path, text, create).map_err(|err| {
                if made_dir {
                    let _ = fs::remove_dir(&dir);
                }
                io_failed(&path, &err)
            })?;
        }
        None => fs::remove_file(&path).map_err(|err| io_failed(&path, &err))?,
    }
    let mut output = written(setup.harness, setup.undo, emptied, &changed, &shown);
    if !setup.undo {
        output.warnings.extend(inline_hooks(&root, setup.harness));
    }
    Ok(output)
}

/// Codex also reads hooks inline from `.codex/config.toml`, loading both
/// forms and warning at startup when a layer holds both.
fn inline_hooks(root: &Path, harness: Harness) -> Option<String> {
    let Harness::Codex = harness else {
        return None;
    };
    let text = match fs::read_to_string(root.join(".codex/config.toml")) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return None,
        Err(err) => {
            return Some(format!(
                "cannot read .codex/config.toml to check it for hooks too: {err}"
            ));
        }
    };
    match toml::from_str::<toml::Table>(&text) {
        Ok(config) => config.contains_key("hooks").then(|| {
            ".codex/config.toml also defines hooks; Codex loads both and warns when a session \
             starts"
                .to_owned()
        }),
        Err(err) => Some(format!(
            ".codex/config.toml is not valid TOML, so tm cannot tell whether it defines hooks \
             too: {}",
            err.message()
        )),
    }
}

/// Whether `tm setup claude` would add anything to the repo at `root`; an
/// error names a settings file it cannot read, merge into or write.
pub(crate) fn needed(root: &Path) -> Result<bool, String> {
    let file = settings(Harness::Claude);
    let path = root.join(file);
    let before = read(&path).map_err(|err| err.message)?;
    let mut settings = parse(before.as_deref(), file).map_err(|err| err.message)?;
    let added = add(&mut settings, &handlers(Harness::Claude), file).map_err(|err| err.message)?;
    if added.is_empty() {
        return Ok(false);
    }
    writable(&path, file)
        .map(|()| true)
        .map_err(|err| err.message)
}

/// Refuses a file tm would write through a symlink, whose rename would
/// replace the link with a copy, or one marked read-only.
fn writable(path: &Path, file: &str) -> Result<(), CliError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(refused(format!(
            "{file} is a symlink, so tm leaves it alone; change the file it points to instead"
        ))),
        Ok(meta) if meta.permissions().readonly() => Err(refused(format!(
            "{file} is read-only, so tm leaves it alone"
        ))),
        _ => Ok(()),
    }
}

fn parse(text: Option<&str>, file: &str) -> Result<Json, CliError> {
    match text {
        Some(text) => serde_json::from_str(text).map_err(|err| {
            refused(format!(
                "{file} is not valid JSON ({err}); fix it, then run tm setup again"
            ))
        }),
        None => Ok(Json::Object(Vec::new())),
    }
}

/// The file's text, through a symlink, `None` where there is none.
fn read(path: &Path) -> Result<Option<String>, CliError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(io_failed(path, &err)),
    }
}

/// Shows `shown` and asks, unless `--yes` confirms it already. JSON mode
/// shows and asks nothing; its refusal carries the diff. An Agent is only
/// shown it, `--yes` or not: the hooks run commands in every session, so a
/// person writes them (ADR 0014).
fn confirmed(shown: &str, emptied: bool, setup: &Setup, json: bool) -> Result<(), CliError> {
    let file = settings(setup.harness);
    let not_written = |why: &str| refused(format!("{file} not written{why}")).with("diff", shown);
    if identity::agent_detected(identity::env) {
        let unshown = if json { None } else { print_now(shown).err() };
        let undo = if setup.undo { " --undo" } else { "" };
        let refusal = not_written(&format!(
            ": an Agent is shown the diff; a person runs tm setup {}{undo} to write it",
            setup.harness.name()
        ));
        return Err(match unshown {
            Some(err) => refusal.warned(vec![format!("the diff was not shown: {err}")]),
            None => refusal,
        });
    }
    let yes = setup.yes;
    if json {
        return if yes {
            Ok(())
        } else {
            Err(not_written(": confirm with --yes"))
        };
    }
    if yes {
        return print_now(shown).map_err(|err| CliError::new("io", err));
    }
    if !human() {
        print_now(shown).map_err(|err| CliError::new("io", err))?;
        return Err(not_written(
            ": confirm with --yes when not a person at a terminal",
        ));
    }
    let question = if emptied {
        format!("Remove {file}?")
    } else {
        format!("Write {file}?")
    };
    match ask(shown, &question) {
        Ok(true) => Ok(()),
        Ok(false) => Err(not_written("")),
        Err(err) => Err(CliError::new("io", err)),
    }
}

fn unchanged(undo: bool, file: &str) -> Output {
    let text = if undo {
        format!("{file} holds none of tm's hooks; nothing to remove\n")
    } else {
        format!("tm's hooks are already in {file}\n")
    };
    Output::new(
        text,
        json!({ "file": file, "changed": [], "deleted": false, "commit": [] }),
    )
}

fn written(harness: Harness, undo: bool, deleted: bool, changed: &[String], shown: &str) -> Output {
    let file = settings(harness);
    let (hooks, runs) = match harness {
        Harness::Claude => (
            "tm prime when a session starts, each sub-agent's tm commands written under its own \
             name, a heartbeat after each tool use, and releasing Claims when a sub-agent or the \
             session ends",
            "Claude Code runs them once you trust this folder, which it asks when a session \
             starts here.",
        ),
        Harness::Codex => (
            "tm prime when a session starts, a heartbeat after each tool use, and releasing \
             Claims when it ends",
            "Codex runs them once you trust this project and approve each hook in /hooks; \
             tm's hooks in ~/.codex would run as well, not instead.",
        ),
    };
    let text = match (undo, deleted) {
        (true, true) => format!("Removed {file}, which held only tm's hooks\n"),
        (true, false) => format!("Removed tm's hooks from {file}\n"),
        (false, _) => format!("Added tm's hooks to {file}: {hooks}. {runs}\n"),
    };
    Output::new(
        format!("{text}Commit {file}\n"),
        json!({
            "file": file,
            "changed": changed,
            "deleted": deleted,
            "diff": shown,
            "commit": [file],
        }),
    )
}

/// One of tm's hook handlers, with the event and matcher it goes under.
struct Handler {
    event: &'static str,
    matcher: Option<&'static str>,
    command: String,
    extra: Vec<(&'static str, Json)>,
    /// The command an older tm wrote for this handler, rewritten in place.
    replaces: Option<String>,
}

impl Handler {
    /// Its group: the matcher, if any, and a list holding the handler.
    fn group(&self) -> Json {
        let mut handler = vec![
            ("type".to_owned(), Json::String("command".into())),
            ("command".to_owned(), Json::String(self.command.clone())),
        ];
        handler.extend(
            self.extra
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone())),
        );
        let mut group: Vec<(String, Json)> = self
            .matcher
            .map(|matcher| ("matcher".to_owned(), Json::String(matcher.into())))
            .into_iter()
            .collect();
        group.push(("hooks".to_owned(), Json::Array(vec![Json::Object(handler)])));
        Json::Object(group)
    }
}

/// tm's handlers for `harness`.
fn handlers(harness: Harness) -> Vec<Handler> {
    match harness {
        // The heartbeat and release run from `$CLAUDE_PROJECT_DIR`, since a
        // hook's working directory follows the Agent's `cd` out of the repo;
        // the heartbeat is async, so no tool use waits on it.
        Harness::Claude => {
            let from_project =
                |args: &str| format!("cd \"${{CLAUDE_PROJECT_DIR:?}}\" && tm {args}");
            let timeout = || ("timeout", Json::Number(10.into()));
            // Exit 2 blocks the Bash call, or keeps a sub-agent from stopping,
            // and a tm older than these hooks exits 2 for a subcommand it does
            // not know; exit 1 shows the failure without blocking.
            let unblocking = |command: String| format!("{command} || exit 1");
            vec![
                Handler {
                    event: "SessionStart",
                    matcher: None,
                    command: "tm prime".into(),
                    extra: Vec::new(),
                    replaces: None,
                },
                Handler {
                    event: "PreToolUse",
                    matcher: Some("Bash"),
                    command: unblocking("tm hook pre-bash claude".into()),
                    extra: Vec::new(),
                    replaces: None,
                },
                Handler {
                    event: "PostToolUse",
                    matcher: Some("*"),
                    command: from_project("hook heartbeat claude"),
                    extra: vec![("async", Json::Bool(true))],
                    replaces: None,
                },
                Handler {
                    event: "SubagentStop",
                    matcher: None,
                    command: unblocking(from_project("hook subagent-stop claude")),
                    extra: vec![timeout()],
                    replaces: None,
                },
                Handler {
                    event: "SessionEnd",
                    matcher: None,
                    command: from_project("hook session-end claude"),
                    extra: vec![timeout()],
                    replaces: Some(from_project("release --mine")),
                },
            ]
        }
        // Codex runs hooks in the session's directory and caps SessionEnd at
        // 3 seconds; each command reads the session from the hook's stdin.
        Harness::Codex => {
            let status = |text: &str| ("statusMessage", Json::String(text.into()));
            vec![
                Handler {
                    event: "SessionStart",
                    matcher: None,
                    command: "tm hook session-start codex".into(),
                    extra: vec![status("tm: priming the session")],
                    replaces: None,
                },
                Handler {
                    event: "PostToolUse",
                    matcher: Some("*"),
                    command: "tm hook heartbeat codex".into(),
                    extra: vec![("async", Json::Bool(true)), status("tm: renewing Claims")],
                    replaces: None,
                },
                Handler {
                    event: "SessionEnd",
                    matcher: None,
                    command: "tm hook session-end codex".into(),
                    extra: vec![
                        ("timeout", Json::Number(3.into())),
                        status("tm: releasing Claims"),
                    ],
                    replaces: None,
                },
            ]
        }
    }
}

/// Adds each handler `settings` lacks, in a group of its own at the end of
/// its event, or in place of the older command it replaces; returns the
/// commands added.
fn add(settings: &mut Json, handlers: &[Handler], file: &str) -> Result<Vec<String>, CliError> {
    let Json::Object(root) = settings else {
        return Err(refused(format!("{file} is not a JSON object")));
    };
    let Json::Object(events) = field(root, "hooks", || Json::Object(Vec::new())) else {
        return Err(refused(format!("\"hooks\" in {file} is not an object")));
    };
    let mut added = Vec::new();
    for handler in handlers {
        let event = handler.event;
        let Json::Array(groups) = field(events, event, || Json::Array(Vec::new())) else {
            return Err(refused(format!(
                "\"hooks.{event}\" in {file} is not a list"
            )));
        };
        if groups
            .iter()
            .flat_map(Json::group_handlers)
            .any(|found| found.command() == Some(&handler.command))
        {
            continue;
        }
        let old = groups
            .iter_mut()
            .filter_map(|group| match group.field_mut("hooks") {
                Some(Json::Array(list)) => Some(list),
                _ => None,
            })
            .flatten()
            .find(|found| {
                found.command().is_some() && found.command() == handler.replaces.as_deref()
            });
        match old.and_then(|found| found.field_mut("command")) {
            Some(command) => *command = Json::String(handler.command.clone()),
            None => groups.push(handler.group()),
        }
        added.push(handler.command.clone());
    }
    Ok(added)
}

/// Removes every handler whose command is one of tm's, then each group,
/// event and `hooks` object that removal left empty; returns the commands
/// removed. An event's list empty before tm added to it goes too, which
/// changes nothing Claude Code runs.
fn remove(settings: &mut Json, handlers: &[Handler], file: &str) -> Result<Vec<String>, CliError> {
    let Json::Object(root) = settings else {
        return Err(refused(format!("{file} is not a JSON object")));
    };
    let ours: Vec<&str> = handlers
        .iter()
        .flat_map(|handler| [Some(handler.command.as_str()), handler.replaces.as_deref()])
        .flatten()
        .collect();
    let mut removed = Vec::new();
    let Some(Json::Object(events)) = root
        .iter_mut()
        .find_map(|(key, value)| (key == "hooks").then_some(value))
    else {
        return Ok(removed);
    };
    events.retain_mut(|(_, groups)| {
        let Json::Array(groups) = groups else {
            return true;
        };
        let before = removed.len();
        groups.retain_mut(|group| {
            let Some(Json::Array(list)) = group.field_mut("hooks") else {
                return true;
            };
            let count = list.len();
            list.retain(|found| match found.command() {
                Some(command) if ours.contains(&command) => {
                    removed.push(command.to_owned());
                    false
                }
                _ => true,
            });
            !(list.is_empty() && list.len() < count)
        });
        !(groups.is_empty() && removed.len() > before)
    });
    if !removed.is_empty() && events.is_empty() {
        root.retain(|(key, _)| key != "hooks");
    }
    Ok(removed)
}

/// The value at `key`, inserted at the end as `default()` when missing.
fn field<'a>(
    object: &'a mut Vec<(String, Json)>,
    key: &str,
    default: impl FnOnce() -> Json,
) -> &'a mut Json {
    let index = object
        .iter()
        .position(|(found, _)| found == key)
        .unwrap_or_else(|| {
            object.push((key.to_owned(), default()));
            object.len() - 1
        });
    &mut object[index].1
}

/// `settings` with `before`'s indent, line ending and final newline; other
/// layout, such as a one-line array, comes out expanded in the diff.
fn render(settings: &Json, before: Option<&str>) -> String {
    let indent = before
        .and_then(|text| text.lines().nth(1))
        .map(|line| &line[..line.len() - line.trim_start().len()])
        .filter(|indent| !indent.is_empty())
        .unwrap_or("  ");
    let mut out = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent.as_bytes());
    settings
        .serialize(&mut serde_json::Serializer::with_formatter(
            &mut out, formatter,
        ))
        .expect("JSON values always serialize");
    let mut text = String::from_utf8(out).expect("serde_json writes UTF-8");
    if before.is_none_or(|before| before.ends_with('\n')) {
        text.push('\n');
    }
    if before.is_some_and(|before| before.contains("\r\n")) {
        text = text.replace('\n', "\r\n");
    }
    text
}

/// A JSON value that keeps an object's keys in the order the file had them;
/// `serde_json::Value` sorts them, and the diff would show every moved key.
#[derive(Clone, Debug, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    fn field_mut(&mut self, key: &str) -> Option<&mut Json> {
        match self {
            Json::Object(fields) => fields
                .iter_mut()
                .find_map(|(found, value)| (found == key).then_some(value)),
            _ => None,
        }
    }

    /// A handler's command, `None` for anything else.
    fn command(&self) -> Option<&str> {
        match self {
            Json::Object(fields) => fields.iter().find_map(|(key, value)| match value {
                Json::String(command) if key == "command" => Some(command.as_str()),
                _ => None,
            }),
            _ => None,
        }
    }

    /// The handlers in a matcher group.
    fn group_handlers(&self) -> &[Json] {
        match self {
            Json::Object(fields) => fields
                .iter()
                .find_map(|(key, value)| match value {
                    Json::Array(list) if key == "hooks" => Some(list.as_slice()),
                    _ => None,
                })
                .unwrap_or_default(),
            _ => &[],
        }
    }
}

impl Serialize for Json {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Json::Null => serializer.serialize_unit(),
            Json::Bool(value) => serializer.serialize_bool(*value),
            Json::Number(value) => value.serialize(serializer),
            Json::String(value) => serializer.serialize_str(value),
            Json::Array(list) => {
                let mut seq = serializer.serialize_seq(Some(list.len()))?;
                for value in list {
                    seq.serialize_element(value)?;
                }
                seq.end()
            }
            Json::Object(fields) => {
                let mut map = serializer.serialize_map(Some(fields.len()))?;
                for (key, value) in fields {
                    map.serialize_entry(key, value)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(JsonVisitor)
    }
}

struct JsonVisitor;

impl<'de> Visitor<'de> for JsonVisitor {
    type Value = Json;

    fn expecting(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str("a JSON value")
    }

    fn visit_unit<E>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }

    fn visit_bool<E>(self, value: bool) -> Result<Json, E> {
        Ok(Json::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Json, E> {
        Ok(Json::Number(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Json, E> {
        Ok(Json::Number(value.into()))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Json, E> {
        Number::from_f64(value)
            .map(Json::Number)
            .ok_or_else(|| E::custom("a number JSON cannot hold"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Json, E> {
        Ok(Json::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Json, E> {
        Ok(Json::String(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
        let mut list = Vec::new();
        while let Some(value) = seq.next_element()? {
            list.push(value);
        }
        Ok(Json::Array(list))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
        let mut fields: Vec<(String, Json)> = Vec::new();
        while let Some((key, value)) = map.next_entry::<String, Json>()? {
            // Claude Code keeps the last of a repeated key and tm would read
            // the first, so the two would disagree on what the file holds.
            if fields.iter().any(|(found, _)| *found == key) {
                return Err(de::Error::custom(format!("duplicate key {key:?}")));
            }
            fields.push((key, value));
        }
        Ok(Json::Object(fields))
    }
}

#[cfg(test)]
mod tests {
    use super::{Json, add, handlers, remove, render};
    use crate::hook::Harness;

    const FILE: &str = ".claude/settings.json";

    fn parse(text: &str) -> Json {
        serde_json::from_str(text).unwrap()
    }

    fn added(text: &str) -> (String, Vec<String>) {
        let mut settings = parse(text);
        let added = add(&mut settings, &handlers(Harness::Claude), FILE).unwrap();
        (render(&settings, Some(text)), added)
    }

    fn removed(text: &str) -> (String, Vec<String>) {
        let mut settings = parse(text);
        let removed = remove(&mut settings, &handlers(Harness::Claude), FILE).unwrap();
        (render(&settings, Some(text)), removed)
    }

    #[test]
    fn the_owners_keys_keep_their_order_and_indent() {
        let before = "{\n    \"zebra\": 1,\n    \"apple\": [true, null, 2.5]\n}\n";
        let (after, added) = added(before);
        assert_eq!(added.len(), 5);
        assert!(
            after.starts_with("{\n    \"zebra\": 1,\n    \"apple\": [\n        true,"),
            "{after}"
        );
        assert!(
            after.contains("\n    \"hooks\": {\n        \"SessionStart\""),
            "{after}"
        );
        assert!(after.ends_with("}\n"));
    }

    #[test]
    fn a_handler_already_there_is_not_added_twice() {
        let (once, _) = added("{}\n");
        let (twice, again) = added(&once);
        assert_eq!(twice, once);
        assert_eq!(again, Vec::<String>::new());
    }

    #[test]
    fn undo_takes_out_only_tms_handlers_and_what_they_emptied() {
        let before = r#"{
  "hooks": {
    "SessionStart": [
      { "hooks": [{ "type": "command", "command": "tm prime" }, { "type": "command", "command": "echo hi" }] },
      { "matcher": "Edit", "hooks": [] }
    ],
    "Notification": [],
    "Stop": [{ "hooks": [{ "type": "command", "command": "tm prime --mine" }] }]
  },
  "model": "x"
}
"#;
        let (with_tm, _) = added(before);
        let (after, removed) = removed(&with_tm);
        assert_eq!(
            removed.len(),
            5,
            "the one already there and the four added: {removed:?}"
        );
        let after = parse(&after);
        let expected = parse(
            r#"{
  "hooks": {
    "SessionStart": [
      { "hooks": [{ "type": "command", "command": "echo hi" }] },
      { "matcher": "Edit", "hooks": [] }
    ],
    "Notification": [],
    "Stop": [{ "hooks": [{ "type": "command", "command": "tm prime --mine" }] }]
  },
  "model": "x"
}"#,
        );
        assert_eq!(after, expected);
    }

    #[test]
    fn an_older_session_end_is_rewritten_in_place_and_undone() {
        let before = r#"{
  "hooks": {
    "SessionEnd": [
      { "hooks": [{ "type": "command", "command": "cd \"${CLAUDE_PROJECT_DIR:?}\" && tm release --mine", "timeout": 30 }] }
    ]
  }
}
"#;
        let (after, added) = added(before);
        assert!(
            added
                .iter()
                .any(|command| command.ends_with("tm hook session-end claude"))
        );
        let ended =
            &serde_json::from_str::<serde_json::Value>(&after).unwrap()["hooks"]["SessionEnd"];
        assert_eq!(ended.as_array().unwrap().len(), 1, "{after}");
        assert_eq!(
            ended[0]["hooks"][0],
            serde_json::json!({
                "type": "command",
                "command": "cd \"${CLAUDE_PROJECT_DIR:?}\" && tm hook session-end claude",
                "timeout": 30,
            })
        );
        let (_, removed) = removed(before);
        assert_eq!(removed.len(), 1, "{removed:?}");
    }

    #[test]
    fn undo_of_a_file_holding_only_tms_hooks_leaves_an_empty_object() {
        let (with_tm, _) = added("{}\n");
        let mut settings = parse(&with_tm);
        remove(&mut settings, &handlers(Harness::Claude), FILE).unwrap();
        assert_eq!(settings, Json::Object(Vec::new()));
        let (_, none) = removed("{\"hooks\": {}}\n");
        assert_eq!(none, Vec::<String>::new());
    }

    #[test]
    fn a_settings_shape_tm_cannot_merge_into_is_refused() {
        for text in [
            "[]",
            "{\"hooks\": 1}",
            "{\"hooks\": {\"SessionStart\": {}}}",
        ] {
            let mut settings = parse(text);
            assert!(
                add(&mut settings, &handlers(Harness::Claude), FILE).is_err(),
                "{text}"
            );
        }
    }
}
