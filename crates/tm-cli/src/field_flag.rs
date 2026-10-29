//! A field given to edit, create or move as its own flag, such as
//! `--priority low`, refused with the command line rewritten to `--set`.

use clap::parser::ValueSource;
use clap::{Arg, ArgAction, ArgMatches, Command};
use tm_core::scheme::Scheme;

use crate::edit;
use crate::error::CliError;
use crate::fields::quoted;

/// Starts the id of each hidden flag that accepts a field.
const PREFIX: &str = "field:";

/// The fields `--set` writes, by name.
fn settable() -> Vec<String> {
    Scheme::built_in()
        .fields
        .into_iter()
        .filter(|f| !f.derived && edit::own_flag(&f.name).is_ok())
        .map(|f| f.name)
        .collect()
}

/// `sub` with each settable field its own optional flag, and nothing it
/// requires but its first positional, the Issue, so the line parses as typed.
fn accepting_fields(mut sub: Command) -> Command {
    let longs: Vec<String> = sub
        .get_arguments()
        .filter_map(|arg| arg.get_long().map(str::to_owned))
        .collect();
    // clap takes only static names; this runs once, as tm exits refusing.
    let leak = |text: String| -> &'static str { Box::leak(text.into_boxed_str()) };
    for name in settable() {
        let long = name.replace('_', "-");
        if !longs.contains(&long) {
            sub = sub.arg(
                Arg::new(leak(format!("{PREFIX}{name}")))
                    .long(leak(long))
                    .num_args(0..=1)
                    .default_missing_value("")
                    .action(ArgAction::Append)
                    .hide(true),
            );
        }
    }
    let groups: Vec<String> = sub.get_groups().map(|g| g.get_id().to_string()).collect();
    for id in groups {
        sub = sub.mut_group(id, |group| group.required(false));
    }
    let issue = sub
        .get_arguments()
        .find(|arg| arg.is_positional())
        .map(|arg| arg.get_id().to_string());
    let required: Vec<String> = sub
        .get_arguments()
        .filter(|arg| arg.is_required_set() && Some(arg.get_id().to_string()) != issue)
        .map(|arg| arg.get_id().to_string())
        .collect();
    for id in required {
        sub = sub.mut_arg(id, |arg| arg.required(false));
    }
    sub
}

fn occurrences(matches: &ArgMatches, id: &str) -> Vec<Vec<String>> {
    matches
        .get_raw_occurrences(id)
        .map(|all| {
            all.map(|values| values.map(|v| v.to_string_lossy().into_owned()).collect())
                .collect()
        })
        .unwrap_or_default()
}

/// The refused line as clap parsed it the second time.
struct Parsed {
    positionals: Vec<String>,
    /// Each field given as a flag, with each use's value; empty is none.
    fields: Vec<(String, Vec<Option<String>>)>,
    /// Every other option given: its long name and each use's values.
    options: Vec<(String, Vec<Vec<String>>)>,
}

impl Parsed {
    fn read(def: &Command, matches: &ArgMatches) -> Self {
        let mut parsed = Self {
            positionals: Vec::new(),
            fields: Vec::new(),
            options: Vec::new(),
        };
        for arg in def.get_arguments() {
            let id = arg.get_id().as_str();
            if matches.value_source(id) != Some(ValueSource::CommandLine) {
                continue;
            }
            let given = occurrences(matches, id);
            if let Some(name) = id.strip_prefix(PREFIX) {
                let values = given
                    .into_iter()
                    // `--set FIELD=-` reads stdin, so a lone `-` is no value.
                    .map(|values| {
                        values
                            .into_iter()
                            .next()
                            .filter(|v| !v.is_empty() && v != "-")
                    })
                    .collect();
                parsed.fields.push((name.to_owned(), values));
            } else if arg.is_positional() {
                parsed.positionals.extend(given.into_iter().flatten());
            } else if let Some(long) = arg.get_long() {
                let values = if arg.get_action().takes_values() {
                    given
                } else {
                    vec![Vec::new()]
                };
                parsed.options.push((long.to_owned(), values));
            }
        }
        parsed
    }

    /// `--set` for each use of each field but `except`.
    fn sets(&self, except: Option<&str>) -> Vec<String> {
        self.fields
            .iter()
            .filter(|(name, _)| Some(name.as_str()) != except)
            .flat_map(|(name, values)| {
                values.iter().flat_map(move |value| {
                    let pair = value.as_ref().map_or_else(
                        || format!("{name}=<value>"),
                        |v| quoted(&format!("{name}={v}")),
                    );
                    ["--set".to_owned(), pair]
                })
            })
            .collect()
    }

    fn options(&self) -> Vec<String> {
        self.options
            .iter()
            .flat_map(|(long, uses)| {
                uses.iter().flat_map(move |values| {
                    let flag = format!("--{long}");
                    if values.is_empty() {
                        return vec![flag];
                    }
                    // clap takes a value starting with - only when joined by =.
                    values
                        .iter()
                        .flat_map(|v| {
                            if v.starts_with('-') {
                                vec![quoted(&format!("{flag}={v}"))]
                            } else {
                                vec![flag.clone(), quoted(v)]
                            }
                        })
                        .collect()
                })
            })
            .collect()
    }

    /// The `tm move` an edit or move given one `--status` becomes, when move
    /// takes every other option: the Status from the flag, or none when move
    /// was given its own.
    fn as_move(&self, takes: &Command) -> Option<Vec<String>> {
        let [(_, values)] = &self
            .fields
            .iter()
            .filter(|(n, _)| n == "status")
            .collect::<Vec<_>>()[..]
        else {
            return None;
        };
        let [value] = &values[..] else {
            return None;
        };
        let movable = self.options.iter().all(|(long, _)| {
            takes
                .get_arguments()
                .any(|arg| arg.get_long() == Some(long.as_str()))
        });
        let status = match (value, self.positionals.len()) {
            (Some(value), 1) => Some(quoted(value)),
            (None, 1) => Some("<status>".to_owned()),
            (None, 2) => None,
            _ => return None,
        };
        movable.then(|| {
            let mut line = vec!["move".to_owned()];
            line.extend(self.positionals.iter().map(|p| quoted(p)));
            line.extend(self.sets(Some("status")));
            line.extend(self.options());
            line.extend(status);
            line
        })
    }
}

/// A settable field given to edit, create or move as its own flag: a usage
/// error whose fix is the line clap parses with each field accepted as a
/// flag, rebuilt with `--set`, or for one `--status` beside options tm move
/// takes, the move. A line even that parse refuses gets `--set` alone as its
/// fix. `None` leaves clap's error.
pub(crate) fn refused(cli: Command, args: &[String], flag: &str) -> Option<CliError> {
    let field = flag.strip_prefix("--")?.replace('-', "_");
    let command = args
        .iter()
        .skip(1)
        .find(|arg| !arg.starts_with('-'))?
        .clone();
    // After `--` the person gave the flag as a word, which clap's error says.
    let as_flag = args.iter().take_while(|arg| *arg != "--").any(|arg| {
        arg.strip_prefix(flag)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('='))
    });
    if !as_flag
        || !["edit", "create", "move"].contains(&command.as_str())
        || !settable().contains(&field)
    {
        return None;
    }
    let error = |how: &str| {
        CliError::new("usage", format!("tm {command} has no {flag} flag; {how}"))
            .with("field", field.as_str())
    };
    let set = "a field is set with --set FIELD=VALUE";
    let mut cli = cli.mut_subcommand(&command, accepting_fields);
    let Ok(matches) = cli.try_get_matches_from_mut(args) else {
        return Some(error(set).fix(format!("--set {field}=<value>")));
    };
    let def = cli.find_subcommand(&command)?;
    let parsed = Parsed::read(def, matches.subcommand_matches(&command)?);
    let moved = (command != "create")
        .then(|| {
            cli.find_subcommand("move")
                .and_then(|takes| parsed.as_move(takes))
        })
        .flatten();
    let (how, line) = if let Some(line) = moved {
        ("the Status changes with tm move ISSUE STATUS", line)
    } else {
        let mut line = vec![command.clone()];
        line.extend(parsed.positionals.iter().map(|p| quoted(p)));
        line.extend(parsed.sets(None));
        line.extend(parsed.options());
        (set, line)
    };
    Some(error(how).fix(format!("tm {}", line.join(" "))))
}
