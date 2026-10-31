//! `tm remember`, `tm memories` and `tm forget`: what a Project's sessions
//! should know, kept in tm (ADR 0017).

use std::fmt::Write as _;

use serde_json::{Value, json};
use tm_core::Actor;
use tm_core::memory::is_key;
use tm_store::{Memory, Remembered};

use crate::error::CliError;
use crate::here::{self, Here};
use crate::identity::{self, Identity};
use crate::output::{Output, printable, span};

/// Memories `tm prime` shows.
pub(crate) const PRIME_SHOWN: usize = 10;

/// A memory's first line as a list shows it, in bytes.
const LINE_BYTES: usize = 100;

#[derive(clap::Args)]
pub(crate) struct Remember {
    /// The fact, in a line or two; - reads it from stdin.
    insight: String,
    /// Name it, or replace what this key holds; made from its first words
    /// when not given.
    #[arg(long)]
    key: Option<String>,
}

#[derive(clap::Args)]
pub(crate) struct Memories {
    /// A memory's key to show it in full, or words every listed memory
    /// holds; every memory when not given.
    query: Vec<String>,
}

#[derive(clap::Args)]
pub(crate) struct Forget {
    /// The memory's key, as tm memories lists it.
    key: String,
}

pub(crate) fn remember(args: &Remember) -> Result<Output, CliError> {
    here::write(|here| {
        let identity = identity(here)?;
        match write_remember(here, args, &identity.actor) {
            Ok(output) => Ok((output, identity.warnings)),
            Err(err) => Err(err.warned(identity.warnings)),
        }
    })
}

pub(crate) fn forget(args: &Forget) -> Result<Output, CliError> {
    here::write(|here| {
        let identity = identity(here)?;
        let project = here.project()?;
        let (memory, ops) = here.store.forget(&identity.actor, project, &args.key)?;
        let key = memory.key.as_deref().unwrap_or(&args.key);
        let text = format!("forgot {}\n", printable(key));
        let data = json!({ "memory": memory_json(&memory) });
        let output = here.undoable(Output::new(text, data), ops[0].op_id, &identity.actor)?;
        Ok((output, identity.warnings))
    })
}

pub(crate) fn memories(args: &Memories) -> Result<Output, CliError> {
    let mut here = here::read()?;
    let warnings = std::mem::take(&mut here.warnings);
    let mut output = list(&here, &args.query).map_err(|err| err.warned(warnings.clone()))?;
    let mut all = warnings;
    all.append(&mut output.warnings);
    output.warnings = all;
    Ok(output)
}

fn identity(here: &Here) -> Result<Identity, CliError> {
    identity::resolve(identity::env, || identity::git_email(&here.found.dir))
        .map_err(|message| CliError::new("no_identity", message))
}

fn write_remember(here: &mut Here, args: &Remember, actor: &Actor) -> Result<Output, CliError> {
    if args.insight.trim().is_empty() {
        return Err(CliError::new("invalid_value", "the insight is blank")
            .with("field", "body")
            .with("reason", "is blank"));
    }
    let project = here.project()?;
    // Read once all else is checked, so a mistake keeps the piped text.
    let body = if args.insight == "-" {
        crate::fields::read_stdin_text("insight")?
    } else {
        args.insight.clone()
    };
    let body = body.trim().to_owned();
    let remembered = here
        .store
        .remember(actor, project, &body, args.key.as_deref())?;
    let Remembered {
        memory,
        replaced,
        restored,
        alike,
        warnings,
        ops,
    } = remembered;
    let key = printable(memory.key.as_deref().unwrap_or_default());
    let created = ops.iter().any(|op| op.kind == "create_memory");
    let text = match (&replaced, restored, ops.is_empty()) {
        (_, _, true) => format!("{key} already holds that\n"),
        (Some(_), _, _) => format!("{key} now reads: {}\n", first_line(&body)),
        (None, true, _) => format!("{key} is remembered again: {}\n", first_line(&body)),
        (None, false, _) => format!("remembered {key}: {}\n", first_line(&body)),
    };
    let data = json!({
        "memory": memory_json(&memory),
        "created": created,
        "restored": restored,
        "replaced": replaced.as_ref().map(|(body, by)| json!({ "body": body, "by": by })),
        "alike": alike,
    });
    let mut output = Output::new(text, data);
    output.warnings = warnings;
    if let Some((old, by)) = &replaced
        && by.as_deref() != Some(actor.name())
    {
        let by = by.as_deref().unwrap_or("someone");
        output.warnings.push(format!(
            "{key} held what {} wrote, now replaced: {}",
            printable(by),
            first_line(old)
        ));
    }
    for other in &alike {
        let other = printable(other);
        output.warnings.push(format!(
            "this reads like {other}; tm remember --key {other} \"...\" updates it, and tm forget {key} drops this one"
        ));
    }
    match ops.first() {
        Some(op) => here.undoable(output, op.op_id, actor),
        None => Ok(output),
    }
}

/// One memory in full, the ones holding every word asked for, or all.
fn list(here: &Here, query: &[String]) -> Result<Output, CliError> {
    let project = here.project()?;
    let now_ms = now_ms();
    let key = match query {
        [key] if is_key(key) => Some(key.as_str()),
        _ => None,
    };
    if let Some(key) = key
        && let Some(memory) = here.store.memory(project, key)?
    {
        let shown = memory.key.as_deref().unwrap_or(key);
        let mut text = format!("{}  ({})\n", printable(shown), written(&memory, now_ms));
        for line in memory.body.as_deref().unwrap_or_default().lines() {
            let _ = writeln!(text, "  {}", printable(line));
        }
        return Ok(Output::new(text, json!({ "memory": memory_json(&memory) })));
    }
    let words: Vec<String> = query.iter().map(|word| word.to_lowercase()).collect();
    let all = here.store.memories(project)?;
    let total = all.len();
    let found: Vec<Memory> = all
        .into_iter()
        .filter(|memory| {
            let text = format!(
                "{} {}",
                memory.key.as_deref().unwrap_or_default(),
                memory.body.as_deref().unwrap_or_default()
            )
            .to_lowercase();
            words.iter().all(|word| text.contains(word.as_str()))
        })
        .collect();
    let mut text = String::new();
    if let (Some(key), false) = (key, found.is_empty()) {
        let _ = writeln!(
            text,
            "no memory has the key {}; these hold the word:",
            printable(key)
        );
    }
    for memory in &found {
        let _ = writeln!(text, "{}", line(memory, now_ms));
    }
    if found.is_empty() {
        text = if total == 0 {
            "no memories in this Project; tm remember \"...\" adds one\n".into()
        } else {
            format!("no memory holds {}\n", printable(&query.join(" ")))
        };
    }
    let data = json!({
        "memories": found.iter().map(memory_json).collect::<Vec<_>>(),
        "total": total,
    });
    Ok(Output::new(text, data))
}

/// `KEY  first line  (by, age ago)`, as `tm memories` and `tm prime` list one.
pub(crate) fn line(memory: &Memory, now_ms: i64) -> String {
    format!(
        "{}  {}  ({})",
        printable(memory.key.as_deref().unwrap_or("(no key)")),
        first_line(memory.body.as_deref().unwrap_or_default()),
        written(memory, now_ms)
    )
}

/// Who wrote a memory and how long ago, as `claude, 3d ago`.
fn written(memory: &Memory, now_ms: i64) -> String {
    let by = printable(memory.written_by.as_deref().unwrap_or("someone"));
    match memory.written_ms {
        Some(at) => format!("{by}, {} ago", span(now_ms - at)),
        None => by,
    }
}

fn first_line(body: &str) -> String {
    let first = body.lines().next().unwrap_or_default();
    crate::list::cut(&printable(first), LINE_BYTES)
}

pub(crate) fn memory_json(memory: &Memory) -> Value {
    json!({
        "id": memory.id,
        "key": memory.key,
        "body": memory.body,
        "written_by": memory.written_by,
        "written_for": memory.written_for,
        "written_ms": memory.written_ms,
    })
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use tm_core::ProjectId;
    use tm_store::Memory;
    use uuid::Uuid;

    use super::line;

    #[test]
    fn a_line_shows_who_wrote_it_and_how_long_ago() {
        let memory = Memory {
            id: Uuid::nil(),
            project: ProjectId::from_uuid(Uuid::nil()),
            key: Some("race".into()),
            body: Some("Tests need -race\nand more".into()),
            forgotten: false,
            written_by: Some("claude".into()),
            written_for: Some("jace".into()),
            written_ms: Some(1_000),
        };
        let three_days = 1_000 + 3 * 86_400_000;
        assert_eq!(
            line(&memory, three_days),
            "race  Tests need -race  (claude, 3d ago)"
        );
    }
}
