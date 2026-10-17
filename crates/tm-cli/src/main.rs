//! The `tm` command.

mod init;

use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};
use tm_core::Scheme;
use tm_core::scheme::Category;

/// A local-first issue tracker for one human and their AI agents.
#[derive(Parser)]
#[command(name = "tm", version)]
struct Cli {
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
    Show {
        /// Print JSON instead of text.
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let output = match cli.command {
        None => Cli::command().render_help().to_string(),
        Some(Command::Scheme {
            action: SchemeAction::Show { json: false },
        }) => scheme_text(&Scheme::built_in()),
        Some(Command::Scheme {
            action: SchemeAction::Show { json: true },
        }) => format!("{}\n", scheme_json(&Scheme::built_in())),
        Some(Command::Init { key, name, yes }) => match init::run(init::Init { key, name, yes }) {
            Ok(text) => text,
            Err(err) => {
                eprintln!("tm: {err}");
                return ExitCode::FAILURE;
            }
        },
    };
    // `print!` panics when the reader goes away; a reader that stops early,
    // as `head` does, is not an error, but a full disk is.
    let mut stdout = io::stdout().lock();
    match stdout
        .write_all(output.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("tm: cannot write output: {err}");
            ExitCode::FAILURE
        }
    }
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
