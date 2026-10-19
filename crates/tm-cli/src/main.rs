//! The `tm` command.

mod claim;
mod comment;
mod create;
mod doctor;
mod error;
mod here;
mod identity;
mod init;
mod link;
mod list;
mod moves;
mod output;
mod ready;
mod show;

use std::fmt::Write as _;
use std::process::ExitCode;

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand};
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
    Init {
        /// The Project key, such as TM; defaults to the repo directory's name.
        #[arg(long)]
        key: Option<String>,
        /// The Project name; defaults to the repo directory's name.
        #[arg(long)]
        name: Option<String>,
        /// Write the file without asking; needed when not at a terminal.
        #[arg(long)]
        yes: bool,
    },
    /// Claim an Issue for the Agent writing; prints the Claim id that
    /// releases it.
    Claim {
        /// The Issue's Key or full id.
        issue: String,
        /// When the Claim lapses, such as 30m or 2h; recorded now, enforced
        /// once Claims expire.
        #[arg(long = "for", value_name = "DURATION", value_parser = claim::duration)]
        ttl_ms: Option<u64>,
    },
    /// Release a Claim, named by the id `tm claim` printed.
    Release {
        /// The Issue's Key or full id.
        issue: String,
        claim: uuid::Uuid,
    },
    /// List logged operations that did not apply, and Claims another take or
    /// release won.
    Doctor,
    /// Comment on an Issue.
    Comment {
        /// The Issue's Key or full id.
        issue: String,
        /// The comment, or `-` to read it from stdin.
        body: String,
    },
    /// Create an Issue in the Project this directory maps to.
    Create {
        /// The Issue type, such as task or bug; `tm scheme show` lists them.
        #[arg(long = "type")]
        issue_type: Option<String>,
        #[arg(long)]
        title: Option<String>,
        /// Set a field, such as priority=high; repeat for more.
        #[arg(long, value_name = "FIELD=VALUE", value_parser = create::field_value)]
        set: Vec<(String, String)>,
        /// Add a label; repeat for more.
        #[arg(long = "label", value_name = "LABEL")]
        labels: Vec<String>,
        /// The parent Issue's Key or id.
        #[arg(long)]
        parent: Option<String>,
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
    },
    /// Move an Issue to a Status of its Workflow.
    Move {
        /// The Issue's Key or full id.
        issue: String,
        /// The Status, by id or label, such as `in_progress` or "In Review".
        status: String,
        /// Set a field in the same write, such as assignee=jace.
        #[arg(long, value_name = "FIELD=VALUE", value_parser = moves::field_value)]
        set: Vec<(String, String)>,
    },
    /// List the work that can start now: unclaimed todo Issues no open Issue
    /// blocks, most urgent first, with what each would unblock.
    Ready {
        /// Show at most this many.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Show an Issue's fields and Claim.
    Show {
        /// The Issue's Key, such as CPL-42, or its full id.
        issue: String,
    },
    /// Inspect the Scheme.
    Scheme {
        #[command(subcommand)]
        action: SchemeAction,
    },
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
        Some(Command::Init { key, name, yes }) => init::run(init::Init { key, name, yes }, json),
        Some(Command::Claim { issue, ttl_ms }) => claim::take(&issue, ttl_ms),
        Some(Command::Release { issue, claim }) => claim::release(&issue, claim),
        Some(Command::Comment { issue, body }) => comment::run(&issue, &body),
        Some(Command::Doctor) => doctor::run(),
        Some(Command::Create {
            issue_type,
            title,
            set,
            labels,
            parent,
        }) => create::run(&create::Create {
            issue_type,
            title,
            set,
            labels,
            parent,
        }),
        Some(Command::Link { words }) => link::run(&words, true),
        Some(Command::Unlink { words }) => link::run(&words, false),
        Some(Command::List { filter, limit }) => list::run(&filter, limit),
        Some(Command::Move { issue, status, set }) => moves::run(&issue, &status, &set),
        Some(Command::Ready { limit }) => ready::run(limit),
        Some(Command::Show { issue }) => show::run(&issue),
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
