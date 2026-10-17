//! The `tm` command.

use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::process::ExitCode;

use tm_core::Scheme;
use tm_core::scheme::Category;

const USAGE: &str = "usage: tm scheme show [--json]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = match args.as_slice() {
        [] => format!("{USAGE}\n"),
        ["scheme", "show"] => scheme_text(&Scheme::built_in()),
        ["scheme", "show", "--json"] => format!("{}\n", scheme_json(&Scheme::built_in())),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
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
