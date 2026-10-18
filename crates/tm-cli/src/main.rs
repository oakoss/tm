//! The `tm` command.

mod error;
mod init;
mod output;

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
            Ok(Output {
                data: json!({ "help": help }),
                text: help,
            })
        }
        Some(Command::Scheme {
            action: SchemeAction::Show,
        }) => {
            let scheme = Scheme::built_in();
            Ok(Output {
                text: scheme_text(&scheme),
                data: scheme_json(&scheme),
            })
        }
        Some(Command::Init { key, name, yes }) => init::run(init::Init { key, name, yes }, json),
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
            Ok(Output {
                data: json!({ "help": rendered }),
                text: String::new(),
            }),
            &[],
        );
    }
    let message = rendered.lines().next().unwrap_or_default();
    let message = message.strip_prefix("error: ").unwrap_or(message);
    output::finish(true, Err(CliError::new("usage", message)), &[]);
    ExitCode::from(2)
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
