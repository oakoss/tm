//! `tm notifications`: the mentions of you and writes past your Claims, and
//! marking them read (ADR 0019).

use serde_json::{Value, json};
use tm_core::Actor;
use tm_core::mention::{self, Mentioned};
use tm_core::notification;
use tm_store::{Notification, Store};

use crate::error::CliError;
use crate::here::{self, Here};
use crate::identity::{self, Identity};
use crate::memory::now_ms;
use crate::mention::Names;
use crate::output::{Output, id_by_part, id_has_part, printable, short, span};

/// A Notification's line as a list shows it, in bytes.
const LINE_BYTES: usize = 100;

/// The writer and the line in a brief, in bytes each, so `tm prime` stays
/// within its budget.
const BRIEF_BY_BYTES: usize = 24;
const BRIEF_LINE_BYTES: usize = 60;

#[derive(clap::Args)]
#[command(args_conflicts_with_subcommands = true)]
pub(crate) struct Notifications {
    /// Read ones too, not only the unread.
    #[arg(long)]
    all: bool,
    #[command(subcommand)]
    action: Option<Action>,
}

#[derive(clap::Subcommand)]
enum Action {
    /// Mark a Notification read, or all of yours.
    ///
    /// tm undo marks them unread again.
    Read {
        /// Its id as tm notifications lists it, or all.
        id: String,
    },
}

pub(crate) fn run(args: &Notifications) -> Result<Output, CliError> {
    match &args.action {
        None => list(args.all),
        Some(Action::Read { id }) => read(id),
    }
}

fn identity(here: &Here) -> Result<Identity, CliError> {
    identity::resolve(identity::env, || identity::git_email(&here.found.dir))
        .map_err(|message| CliError::new("no_identity", message))
}

fn list(all: bool) -> Result<Output, CliError> {
    let mut here = here::read()?;
    let mut warnings = std::mem::take(&mut here.warnings);
    let identity = identity(&here).map_err(|err| err.warned(warnings.clone()))?;
    warnings.extend(identity.warnings);
    let listed = here.store.notifications(&identity.actor, all)?;
    let Shown { lines, entries, .. } = shown(&here.store, &identity.actor, &listed)?;
    let mut text = lines.join("\n");
    text.push('\n');
    if listed.is_empty() {
        text = if all {
            "no Notifications\n".to_owned()
        } else {
            "no unread Notifications\n".to_owned()
        };
    }
    let mut output = Output::new(text, json!({ "notifications": entries }));
    output.warnings = warnings;
    Ok(output)
}

pub(crate) fn read(reference: &str) -> Result<Output, CliError> {
    here::write(|here| {
        let identity = identity(here)?;
        let actor = &identity.actor;
        let chosen = if reference == "all" {
            here.store.notifications(actor, false)?
        } else {
            vec![one(&here.store, actor, reference)?]
        };
        let ops = here.store.mark_read(actor, &chosen)?;
        let text = match (ops.len(), reference) {
            (0, "all") => "no unread Notifications\n".to_owned(),
            (0, _) => format!("{reference} was read already\n"),
            (1, _) => "marked 1 Notification read\n".to_owned(),
            (n, _) => format!("marked {n} Notifications read\n"),
        };
        let marked: Vec<String> = chosen
            .iter()
            .filter(|notified| !notified.read)
            .map(|notified| notified.id.to_string())
            .collect();
        let output = Output::new(text, json!({ "marked": marked }));
        let output = match ops.first() {
            Some(first) => here.undoable(output, first.op_id, actor)?,
            None => output,
        };
        Ok((output, identity.warnings))
    })
}

/// Notifications as `actor` reads them, one of each per Notification.
pub(crate) struct Shown {
    /// As `tm notifications` lists them.
    pub(crate) lines: Vec<String>,
    /// `by on KEY: line`, short enough for `tm prime`.
    pub(crate) briefs: Vec<String>,
    pub(crate) entries: Vec<Value>,
}

/// Each of `listed` as `actor` reads it.
pub(crate) fn shown(
    store: &Store,
    actor: &Actor,
    listed: &[Notification],
) -> Result<Shown, CliError> {
    let names = Names::of(store, listed.iter().map(|n| n.text.as_str()))?;
    let now_ms = now_ms();
    let mut shown = Shown {
        lines: Vec::with_capacity(listed.len()),
        briefs: Vec::with_capacity(listed.len()),
        entries: Vec::with_capacity(listed.len()),
    };
    for notified in listed {
        let key = key(store, notified)?;
        let by = by(store, notified)?;
        let said = said(notified, actor, &names);
        shown.lines.push(line(notified, (&key, &by), &said, now_ms));
        shown.briefs.push(format!(
            "{} on {}: {}",
            crate::list::cut(&printable(&by), BRIEF_BY_BYTES),
            printable(&key),
            crate::list::cut(&printable(&said), BRIEF_LINE_BYTES)
        ));
        shown.entries.push(entry(notified, &key, &by, &names));
    }
    Ok(shown)
}

/// `actor`'s Notification a full id, or a unique start or end of one, names.
fn one(store: &Store, actor: &Actor, reference: &str) -> Result<Notification, CliError> {
    let id = match uuid::Uuid::parse_str(reference) {
        Ok(id) => id,
        Err(_) => id_by_part(reference, ("Notification", "of yours"), |digits| {
            Ok(store
                .notifications(actor, true)?
                .into_iter()
                .map(|notified| notified.id)
                .filter(|id| id_has_part(*id, digits))
                .collect())
        })?,
    };
    store.notification(actor, id)?.ok_or_else(|| {
        CliError::new(
            "not_found",
            format!("no Notification of yours is {reference}"),
        )
        .with("kind", "Notification")
        .with("reference", reference)
    })
}

fn key(store: &Store, notified: &Notification) -> Result<String, CliError> {
    let view = store.issue(notified.issue_id)?;
    Ok(view
        .and_then(|view| view.key)
        .unwrap_or_else(|| notified.issue_id.to_string()))
}

/// Who wrote it: a person by their recorded name, else their email; an
/// Agent by its name.
fn by(store: &Store, notified: &Notification) -> Result<String, CliError> {
    if notified.on_behalf_of.is_some() {
        return Ok(notified.actor.clone());
    }
    Ok(store
        .name_of(&notified.actor)?
        .unwrap_or_else(|| notified.actor.clone()))
}

/// What it tells the reader: the line that mentions them, shown by name, or
/// the fields written past their Claim.
fn said(notified: &Notification, reader: &Actor, names: &Names) -> String {
    if notified.kind == "passed_claim" {
        return format!("wrote {} while you held it", notified.text);
    }
    names.shown(mentioned_line(&notified.text, reader))
}

/// Where the mention was written, or the Claim written past.
fn place(kind: &str) -> &'static str {
    match kind {
        "passed_claim" => "your Claim",
        "ask" => "a question",
        "answer" => "an answer",
        "withdraw" => "a withdrawn question",
        "set_field" => "the description",
        "edit_comment" => "an edited comment",
        _ => "a comment",
    }
}

/// `id  KEY  by in place, age ago: said`.
fn line(notified: &Notification, (key, by): (&str, &str), said: &str, now_ms: i64) -> String {
    let unread = if notified.read { "" } else { " *" };
    format!(
        "{}{unread}  {}  {} in {}, {} ago: {}",
        short(notified.id),
        printable(key),
        printable(by),
        place(&notified.kind),
        span(now_ms - notified.utc_ms),
        crate::list::cut(&printable(said), LINE_BYTES)
    )
}

/// The line of `text` that mentions `reader`, else its first.
fn mentioned_line<'a>(text: &'a str, reader: &Actor) -> &'a str {
    let me: Mentioned = notification::writer(reader.name(), reader.on_behalf_of());
    let at = mention::tokens(text)
        .into_iter()
        .find(|(_, actor)| *actor == me)
        .map_or(0, |(span, _)| span.start);
    let start = text[..at].rfind('\n').map_or(0, |n| n + 1);
    let end = text[at..].find('\n').map_or(text.len(), |n| at + n);
    text[start..end].trim()
}

fn entry(notified: &Notification, key: &str, by: &str, names: &Names) -> Value {
    json!({
        "id": notified.id,
        "issue": key,
        "op_id": notified.op_id,
        "kind": notified.kind,
        "by": by,
        "actor": notified.actor,
        "on_behalf_of": notified.on_behalf_of,
        "text": notified.text,
        "mentions": names.json([notified.text.as_str()]),
        "at_ms": notified.utc_ms,
        "read": notified.read,
    })
}
