//! The `tm` command.

mod create;
mod error;
mod here;
mod identity;
mod init;
mod list;
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
    /// List the Issues a filter matches; with none, the open work.
    List {
        /// A filter such as `status:todo priority:high`. Negated terms go
        /// after `--`, as in `tm list -- -category:done`.
        filter: Vec<String>,
        /// Show at most this many.
        #[arg(long)]
        limit: Option<usize>,
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
    /// Print the Scheme's types, Workflows and fields.
    Show,
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
            action: SchemeAction::Show,
        }) => {
            let scheme = Scheme::built_in();
            Ok(Output::new(scheme_text(&scheme), scheme_json(&scheme)))
        }
        Some(Command::Init { key, name, yes }) => init::run(init::Init { key, name, yes }, json),
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
        Some(Command::List { filter, limit }) => list::run(&filter, limit),
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
