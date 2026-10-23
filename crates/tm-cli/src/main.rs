//! The `tm` command.

mod backup;
mod claim;
mod comment;
mod create;
mod diff;
mod doctor;
mod edit;
mod error;
mod export;
mod fields;
mod follow;
mod guide;
mod here;
mod history;
mod hook;
mod identity;
mod init;
mod instructions;
mod link;
mod list;
mod moves;
mod output;
mod prime;
mod prompt;
mod ready;
mod recover;
mod scan;
mod shortlist;
mod show;
mod start;
mod undo;

use std::fmt::Write as _;
use std::process::ExitCode;

use clap::error::ErrorKind;
use clap::{ArgGroup, CommandFactory, Parser, Subcommand};
use serde_json::json;
use tm_core::Scheme;
use tm_core::scheme::Category;

use crate::error::CliError;
use crate::output::Output;

/// A local-first issue tracker for one human and their AI agents.
#[derive(Parser)]
#[command(name = "tm", version)]
struct Cli {
    /// Print one JSON document instead of text; `TM_JSON=1` does the same.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Create a Workspace for this repo and map the repo to it in
    /// .config/tm.toml, after showing the file.
    ///
    /// It also offers a pointer to `tm guide` for AGENTS.md or CLAUDE.md,
    /// written only on a yes from a person at a terminal, and prints the
    /// `SessionStart` hook that runs `tm prime`; an Agent is given both to add
    /// itself.
    ///
    /// A person at a terminal is also asked for an export to restore a fresh
    /// clone's Workspace from, and in a repo with .beads/ for a `bd export`
    /// to import into the new one; anyone else is told the commands.
    Init {
        /// The Project key, such as TM. A person at a terminal without --yes is
        /// asked for it; otherwise it defaults to the initials of the repo
        /// directory's name, or the first 3 letters of a one-word name.
        #[arg(long)]
        key: Option<String>,
        /// The Project name. A person at a terminal without --yes is asked
        /// for it; otherwise it defaults to the repo directory's name.
        #[arg(long)]
        name: Option<String>,
        /// Ask nothing: use --key and --name or their defaults, write
        /// .config/tm.toml, and only show tm's instruction lines; needed when
        /// not at a terminal.
        #[arg(long)]
        yes: bool,
    },
    /// Claim an Issue for the Agent writing; prints the Claim id that
    /// releases it.
    Claim {
        /// The Issue's Key or full id.
        issue: String,
        #[command(flatten)]
        lasts: claim::Lasts,
    },
    /// Start work on an Issue: an Agent claims it, it moves from triage or
    /// todo to its first active Status with its parents, and it gains its
    /// Assignee if it has none, all as one undo; prints a branch name for it.
    Start {
        /// The Issue's Key or full id.
        issue: String,
        #[command(flatten)]
        lasts: claim::Lasts,
    },
    /// Renew this session's Claims, as its writes do, through quiet work
    /// such as a long read or test run; a hook may run it after each tool use.
    Heartbeat {
        /// Renew only this Issue's Claim, by Key or full id.
        issue: Option<String>,
    },
    /// Release a Claim. The session holding it, and the human it is held
    /// for, may leave out the id `tm claim` printed.
    #[command(group = ArgGroup::new("what").required(true).args(["issue", "mine"]))]
    Release {
        /// The Issue's Key or full id.
        issue: Option<String>,
        claim: Option<uuid::Uuid>,
        /// Release every Claim this session took, as a harness does when the
        /// session ends.
        #[arg(long, conflicts_with_all = ["issue", "claim"])]
        mine: bool,
    },
    /// List logged operations that did not apply, and Claims another take,
    /// renewal or release won.
    Doctor,
    /// Comment on an Issue.
    ///
    /// Comments keep a dated trail an edit would overwrite: progress, what
    /// was learned and changes of plan.
    Comment {
        /// The Issue's Key or full id.
        issue: String,
        /// The comment, or `-` to read it from stdin.
        body: String,
    },
    /// Create an Issue in the Project this directory maps to.
    ///
    /// Created in an active Status, such as with `--set status=in_progress`,
    /// it starts each parent still in triage or todo, as `tm move` does.
    Create {
        /// The Issue type, such as task or bug; `tm scheme show` lists them.
        #[arg(long = "type")]
        issue_type: Option<String>,
        #[arg(long)]
        title: Option<String>,
        /// Set a field, such as priority=high; FIELD=- reads the value from
        /// stdin. Repeat for more.
        #[arg(long, value_name = "FIELD=VALUE", value_parser = create::field_value)]
        set: Vec<(String, String)>,
        /// Add a label; repeat for more.
        #[arg(long = "label", value_name = "LABEL")]
        labels: Vec<String>,
        /// The parent Issue's Key or id.
        #[arg(long)]
        parent: Option<String>,
    },
    /// Change an Issue's fields, labels or parent in one write; the Status
    /// changes with `tm move`.
    ///
    /// The description holds the current plan; record progress against it
    /// with `tm comment`.
    #[command(group(
        ArgGroup::new("changes")
            .required(true)
            .multiple(true)
            .args(["set", "unset", "labels", "remove_labels", "parent", "no_parent"])
    ))]
    Edit {
        /// The Issue's Key or full id.
        issue: String,
        /// Set a field, such as priority=high; FIELD=- reads the value from
        /// stdin. Repeat for more.
        #[arg(long, value_name = "FIELD=VALUE", value_parser = edit::field_value)]
        set: Vec<(String, String)>,
        /// Clear a field; repeat for more.
        #[arg(long, value_name = "FIELD", value_parser = edit::unset_field)]
        unset: Vec<String>,
        /// Add a label; repeat for more.
        #[arg(long = "label", value_name = "LABEL")]
        labels: Vec<String>,
        /// Remove a label; repeat for more.
        #[arg(long = "remove-label", value_name = "LABEL")]
        remove_labels: Vec<String>,
        /// The new parent Issue's Key or id.
        #[arg(long, conflicts_with = "no_parent")]
        parent: Option<String>,
        /// Remove the parent.
        #[arg(long)]
        no_parent: bool,
    },
    /// Link two Issues, read from either end: `tm link TM-1 blocks TM-2` or
    /// `tm link TM-2 is blocked by TM-1`.
    Link {
        #[arg(value_name = "ISSUE NAME… ISSUE", num_args = 3.., required = true)]
        words: Vec<String>,
    },
    /// Remove a Link, named as `tm link` names it.
    Unlink {
        #[arg(value_name = "ISSUE NAME… ISSUE", num_args = 3.., required = true)]
        words: Vec<String>,
    },
    /// List the Issues a filter matches; with none, the open work.
    List {
        /// A filter such as `status:todo priority:high`. Negated terms go
        /// after `--`, as in `tm list -- -category:done`.
        filter: Vec<String>,
        /// Show at most this many.
        #[arg(long)]
        limit: Option<usize>,
        /// With no filter, every Status, done and cancelled included, rather
        /// than only the open work.
        #[arg(long)]
        all: bool,
    },
    /// Move an Issue to a Status of its Workflow.
    ///
    /// Starting work on it starts each parent still in triage or todo, in the
    /// same batch. Finishing its parent's last open child prints the move
    /// that closes the parent, and reopening a child of a finished parent
    /// the move that reopens it; tm leaves both to you. It prints a line per
    /// change; `tm show` or `--json` gives the whole Issue.
    Move(moves::Move),
    /// Print the context an Agent session starts from, for a `SessionStart` hook.
    Prime,
    /// Print how to work with tm: finding, starting, recording and finishing
    /// work.
    Guide,
    /// List the work that can start now: unclaimed todo Issues no open Issue
    /// blocks, most urgent first, with what each would unblock.
    Ready {
        /// Show at most this many.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// List who holds which Issue: every held Claim with its time left, and
    /// the lapsed ones another Agent may take.
    Claims,
    /// Show your Short-lists, the ordered lists of what to do next, or with
    /// a subcommand create and change them.
    ///
    /// A member that cannot be taken next stays on its list, marked with why:
    /// blocked, claimed, started, archived, an epic or in triage.
    Shortlist(shortlist::Shortlists),
    /// Show an Issue's fields and Claim.
    Show {
        /// The Issue's Key, such as CPL-42, or its full id.
        issue: String,
    },
    /// Show what happened to an Issue, oldest first: each write with when,
    /// who, and the batch to give `tm undo`. Claim renewals are left out.
    History {
        /// The Issue's Key, such as CPL-42, or its full id.
        issue: String,
        /// Show only the latest N writes.
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
        /// Include Claim renewals.
        #[arg(long)]
        all: bool,
    },
    /// Write this Workspace's log to stdout as JSONL, for `tm import`; there
    /// `--json` changes only how an error prints.
    Export {
        /// Write it to tm's home as exports/<Workspace>/<date>.jsonl instead,
        /// unless nothing changed since the newest one there, and remove old
        /// ones by the [backups] policy in tm's config.toml.
        #[arg(long)]
        dated: bool,
    },
    /// Restore an export into its Workspace on this machine, creating the
    /// Workspace when this machine lacks it; with `--records`, write its
    /// Issue records into this repo's Workspace instead, and with `--from`,
    /// another tracker's export as records.
    Import {
        /// The export, records file or tracker export, or `-` to read it from
        /// stdin.
        file: String,
        /// Write the file's Issue records as ordinary writes, keeping current
        /// state and comments but not history.
        #[arg(long)]
        records: bool,
        /// Write another tracker's export, such as `bd export`'s, as records.
        #[arg(long, value_enum, conflicts_with = "records")]
        from: Option<export::Adapter>,
        /// Check the file and the Workspace, and write nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Rebuild every table from the log.
    Rebuild,
    /// Undo one command's writes, named by the batch id it printed; with no
    /// batch, your own last command, stepping back each time.
    Undo {
        /// The batch id, or a unique start or end of it, 8 characters or more.
        batch: Option<String>,
        /// Show what would be restored, and write nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Inspect the Scheme.
    Scheme {
        #[command(subcommand)]
        action: SchemeAction,
    },
    /// Hooks an agent harness can run tm from; tm writes no hook config.
    Hook {
        #[command(subcommand)]
        action: HookAction,
    },
    /// Read this repo's git history; tm never writes to git.
    Git {
        #[command(subcommand)]
        action: GitAction,
    },
}

#[derive(Subcommand)]
enum GitAction {
    /// Move each Issue a `Closes TM-n` line names to done once its commit is
    /// on the default branch, as this clone last fetched it, commenting the
    /// commit and pull request it shipped in. Done Issues are skipped, so it
    /// is safe to run after every merge.
    Scan {
        /// Show what would move, and write nothing.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum HookAction {
    /// Print the hook entries to paste into a harness's settings: a heartbeat
    /// after each tool use, and releasing the session's Claims when it ends.
    Print { harness: hook::Harness },
    /// Renew this session's Claims as tm heartbeat does, printing the
    /// harness's hook output only for a warning not yet given this session.
    Heartbeat { harness: hook::Harness },
}

#[derive(Subcommand)]
enum SchemeAction {
    /// Print the Scheme's types, Workflows and fields; with a type, that
    /// type's Workflow and the fields it carries.
    Show {
        /// An Issue type by id or label, such as task.
        #[arg(value_name = "TYPE")]
        issue_type: Option<String>,
    },
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args_os()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let json = output::json_mode(&args, std::env::var("TM_JSON").ok().as_deref());
    if json {
        output::install_panic_hook();
    }
    let cli = match Cli::try_parse_from(std::env::args_os()) {
        Ok(cli) => cli,
        Err(err) => return parse_failed(json, &err),
    };
    if matches!(cli.command, Some(Command::Export { dated: false })) {
        return match export::export() {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => output::finish(json, Err(err), &[]),
        };
    }
    // The harness reads only its own hook format, which TM_JSON must not wrap.
    if let Some(Command::Hook {
        action: HookAction::Heartbeat { harness },
    }) = cli.command
    {
        return output::finish(false, Ok(hook::heartbeat(harness)), &[]);
    }
    output::finish(json, run(cli.command, json), &[])
}

fn run(command: Option<Command>, json: bool) -> Result<Output, CliError> {
    match command {
        None => {
            let help = Cli::command().render_help().to_string();
            Ok(Output::new(help.clone(), json!({ "help": help })))
        }
        Some(Command::Scheme {
            action: SchemeAction::Show { issue_type: None },
        }) => {
            let scheme = Scheme::built_in();
            Ok(Output::new(scheme_text(&scheme), scheme_json(&scheme)))
        }
        Some(Command::Scheme {
            action: SchemeAction::Show {
                issue_type: Some(issue_type),
            },
        }) => scheme_type(&Scheme::built_in(), &issue_type),
        Some(Command::Init { key, name, yes }) => init::run(&init::Init { key, name, yes }, json),
        Some(Command::Start { issue, lasts }) => start::run(&issue, &lasts),
        Some(Command::Claim { issue, lasts }) => claim::take(&issue, &lasts),
        Some(Command::Heartbeat { issue }) => claim::heartbeat(issue.as_deref()),
        Some(Command::Release {
            issue: Some(issue),
            claim,
            ..
        }) => claim::release(&issue, claim),
        Some(Command::Release { issue: None, .. }) => claim::release_mine(),
        Some(Command::Hook {
            action: HookAction::Print { harness },
        }) => Ok(hook::print(harness)),
        Some(Command::Hook {
            action: HookAction::Heartbeat { .. },
        }) => unreachable!("main runs the hook heartbeat outside JSON mode"),
        Some(Command::Git {
            action: GitAction::Scan { dry_run },
        }) => scan::run(dry_run),
        Some(Command::Comment { issue, body }) => comment::run(&issue, &body),
        Some(Command::Doctor) => doctor::run(),
        Some(Command::Create {
            issue_type,
            title,
            set,
            labels,
            parent,
        }) => create::run(create::Create {
            issue_type,
            title,
            set,
            labels,
            parent,
        }),
        Some(Command::Edit {
            issue,
            set,
            unset,
            labels,
            remove_labels,
            parent,
            no_parent,
        }) => edit::run(
            &issue,
            &edit::Edit {
                set,
                unset,
                labels,
                remove_labels,
                parent,
                no_parent,
            },
        ),
        Some(Command::Link { words }) => link::run(&words, true),
        Some(Command::Unlink { words }) => link::run(&words, false),
        Some(Command::List { filter, limit, all }) => list::run(&filter, limit, all, json),
        Some(Command::Move(args)) => moves::run(&args),
        Some(Command::Prime) => prime::run(json),
        Some(Command::Guide) => Ok(guide::run()),
        Some(Command::Ready { limit }) => ready::run(limit, json),
        Some(Command::Claims) => claim::list(),
        Some(Command::Shortlist(args)) => shortlist::run(&args, json),
        Some(Command::Show { issue }) => show::run(&issue),
        Some(Command::History { issue, limit, all }) => history::run(&issue, limit, all),
        Some(Command::Undo { batch, dry_run }) => undo::run(batch.as_deref(), dry_run),
        Some(Command::Export { dated: true }) => backup::dated(),
        Some(Command::Export { dated: false }) => unreachable!("main writes the export itself"),
        Some(Command::Import {
            file,
            records: true,
            dry_run,
            ..
        }) => export::import_records(&file, dry_run, None),
        Some(Command::Import {
            file,
            from: Some(adapter),
            dry_run,
            ..
        }) => export::import_records(&file, dry_run, Some(adapter)),
        Some(Command::Import { file, dry_run, .. }) => export::import(&file, dry_run),
        Some(Command::Rebuild) => export::rebuild(),
    }
}

/// Help and the version asked for are output, not errors; anything else clap
/// refuses, a missing subcommand included, is a usage error, exit 2.
fn parse_failed(json: bool, err: &clap::Error) -> ExitCode {
    let shown = matches!(
        err.kind(),
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
    );
    if !json {
        let _ = err.print();
        return if shown {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(2)
        };
    }
    let rendered = err.render().to_string();
    if shown {
        return output::finish(
            true,
            Ok(Output::new(String::new(), json!({ "help": rendered }))),
            &[],
        );
    }
    let message = rendered.lines().next().unwrap_or_default();
    let message = message.strip_prefix("error: ").unwrap_or(message);
    output::finish(true, Err(CliError::new("usage", message)), &[])
}

fn scheme_json(scheme: &Scheme) -> serde_json::Value {
    let mut json = serde_json::to_value(scheme).expect("a Scheme always serializes");
    json["status_categories"] = serde_json::json!(Category::ALL);
    json
}

/// One Issue type: its Workflow's Statuses in order and every field, marked
/// required where the Scheme requires it of every Issue.
fn scheme_type(scheme: &Scheme, wanted: &str) -> Result<Output, CliError> {
    let issue_type = scheme
        .types
        .iter()
        .find(|t| t.id.eq_ignore_ascii_case(wanted) || t.label.eq_ignore_ascii_case(wanted))
        .ok_or_else(|| {
            CliError::new("invalid_value", format!("{wanted:?} is not an Issue type"))
                .with("field", "type")
                .with(
                    "allowed",
                    scheme
                        .types
                        .iter()
                        .map(|t| t.id.clone())
                        .collect::<Vec<_>>(),
                )
        })?;
    let workflow = scheme.workflow(&issue_type.id);
    let data = json!({
        "type": issue_type,
        "workflow": workflow,
        "fields": scheme.fields,
    });
    let mut text = format!("type {} ({})\n", issue_type.id, issue_type.label);
    if let Some(workflow) = workflow {
        let _ = writeln!(
            text,
            "workflow {}, starting in {}",
            workflow.name, workflow.start
        );
        for status in &workflow.statuses {
            let _ = writeln!(
                text,
                "  {} ({}): {}",
                status.id,
                status.label,
                status.category.id()
            );
        }
    }
    for field in scheme.fields.iter().filter(|field| !field.derived) {
        let required = if field.required { " [required]" } else { "" };
        let _ = writeln!(text, "field {} ({}){required}", field.name, field.label);
    }
    Ok(Output::new(text, data))
}

fn scheme_text(scheme: &Scheme) -> String {
    let mut out = String::new();
    let line = |out: &mut String, text: String| {
        let _ = writeln!(out, "{text}");
    };
    line(&mut out, format!("Scheme {}", scheme.name));
    for issue_type in &scheme.types {
        line(
            &mut out,
            format!("type {} ({})", issue_type.id, issue_type.label),
        );
    }
    for workflow in &scheme.workflows {
        line(
            &mut out,
            format!("workflow {}, starting in {}", workflow.name, workflow.start),
        );
        for status in &workflow.statuses {
            line(
                &mut out,
                format!(
                    "  {} ({}): {}",
                    status.id,
                    status.label,
                    status.category.id()
                ),
            );
        }
    }
    for field in &scheme.fields {
        let mut notes = Vec::new();
        if field.required {
            notes.push("required");
        }
        if field.derived {
            notes.push("computed");
        }
        let notes = if notes.is_empty() {
            String::new()
        } else {
            format!(" [{}]", notes.join(", "))
        };
        line(
            &mut out,
            format!("field {} ({}){notes}", field.name, field.label),
        );
    }
    out
}
