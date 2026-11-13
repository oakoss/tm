//! The `tm` command.

mod ask;
mod backup;
mod claim;
mod comment;
mod covering;
mod create;
mod diff;
mod doctor;
mod edit;
mod error;
mod export;
mod field_flag;
mod fields;
mod follow;
mod guide;
mod handoff;
mod harness;
mod here;
mod history;
mod hook;
mod identity;
mod init;
mod instructions;
mod link;
mod list;
mod memory;
mod mention;
mod moves;
mod next;
mod notifications;
mod output;
mod prime;
mod prompt;
mod ready;
mod recover;
mod scan;
mod session;
mod setup;
mod shortlist;
mod show;
mod start;
mod tui;
mod undo;
mod views;
mod watch;

use std::fmt::Write as _;
use std::process::ExitCode;

use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{ArgGroup, CommandFactory, FromArgMatches as _, Parser, Subcommand};
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

/// The headings `tm --help` lists commands under, in order; clap gives a
/// command list one heading only.
const GROUPS: [(&str, &[&str]); 5] = [
    (
        "Everyday",
        &[
            "create",
            "list",
            "ready",
            "show",
            "edit",
            "move",
            "comment",
            "notifications",
            "link",
            "unlink",
            "tui",
        ],
    ),
    ("History and undo", &["history", "undo"]),
    (
        "Agent work",
        &[
            "next",
            "start",
            "claim",
            "heartbeat",
            "handoff",
            "release",
            "claims",
            "sessions",
            "shortlist",
            "ask",
            "answer",
            "withdraw",
            "prime",
            "guide",
        ],
    ),
    ("Memories", &["remember", "memories", "forget"]),
    (
        "Setup and data",
        &[
            "init", "setup", "scheme", "git", "export", "import", "rebuild", "doctor", "help",
        ],
    ),
];

/// tm's command line, with its help listing commands under [`GROUPS`].
fn cli() -> clap::Command {
    // Building adds the `help` subcommand the listing names, but also
    // expands `tm help help`; only a copy is built.
    let mut built = Cli::command();
    built.build();
    // tm tui refuses --json and a hook prints its harness's format, so their
    // help leaves the flag out; a subcommand's own `json` takes the global
    // one's place there and below, still accepted.
    let hidden_json = |command: clap::Command| {
        command.arg(
            clap::Arg::new("json")
                .long("json")
                .action(clap::ArgAction::SetTrue)
                .global(true)
                .hide(true),
        )
    };
    Cli::command()
        .help_template(grouped_help(&built))
        .mut_subcommand("tui", hidden_json)
        .mut_subcommand("hook", hidden_json)
}

/// A help template listing `command`'s subcommands under [`GROUPS`], each by
/// its first line, then the options.
fn grouped_help(command: &clap::Command) -> String {
    let styles = command.get_styles();
    let (header, literal) = (styles.get_header(), styles.get_literal());
    let width = GROUPS
        .iter()
        .flat_map(|(_, names)| names.iter().map(|name| name.len()))
        .max()
        .unwrap_or(0);
    let mut out = String::from("{about-with-newline}\n{usage-heading} {usage}\n");
    for (heading, names) in GROUPS {
        let _ = write!(out, "\n{header}{heading}:{header:#}\n");
        for name in names {
            let about = command
                .find_subcommand(name)
                .and_then(clap::Command::get_about)
                .map(ToString::to_string)
                .unwrap_or_default();
            let pad = " ".repeat(width - name.len() + 2);
            let _ = writeln!(out, "  {literal}{name}{literal:#}{pad}{about}");
        }
    }
    let _ = write!(out, "\n{header}Options:{header:#}\n{{options}}");
    out
}

#[derive(Subcommand)]
enum Command {
    /// Create an Issue in the Project this directory maps to.
    ///
    /// Created in an active Status, such as with `--set status=in_progress`,
    /// it starts each parent still in triage or todo, as `tm move` does.
    Create {
        /// The Issue type, such as task or bug; `tm scheme show` lists them.
        #[arg(long = "type")]
        issue_type: Option<String>,
        /// The Issue's title.
        #[arg(long, value_parser = fields::text_value)]
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
    /// List the open Issues a filter matches.
    ///
    /// A filter naming status, category or archived reaches finished ones
    /// too. `tm search` is the same command.
    #[command(alias = "search")]
    List {
        /// A filter such as `status:todo priority:high -labels:bug`,
        /// `is:blocked`, `no:parent`, or words to search for (`login timeout`).
        filter: Vec<String>,
        /// Show at most this many.
        #[arg(long)]
        limit: Option<usize>,
        /// Every Status, done and cancelled included, rather than only the
        /// open work.
        #[arg(long)]
        all: bool,
        /// List again each time another process writes or a held Claim
        /// lapses, until interrupted.
        #[arg(long)]
        watch: bool,
    },
    /// List the work that can start now, most urgent first.
    ///
    /// Unclaimed todo Issues no open Issue blocks, each with what it would
    /// unblock, in the Project this directory maps to.
    Ready {
        /// Show at most this many.
        #[arg(long)]
        limit: Option<usize>,
        /// List again each time another process writes or a held Claim
        /// lapses, until interrupted.
        #[arg(long)]
        watch: bool,
    },
    /// Show an Issue's fields, Claim and latest comments.
    Show {
        /// The Issue's Key, such as CPL-42, or its full id.
        issue: String,
        /// Show the latest N comments, or all; 5 unless given.
        #[arg(long, value_name = "N|all", value_parser = show::comment_limit)]
        comments: Option<show::Comments>,
    },
    /// Change an Issue's fields, labels or parent in one write.
    ///
    /// The Status changes with `tm move`. The description holds the current
    /// plan; record progress against it with `tm comment`.
    Edit(edit::Edit),
    /// Move an Issue to a Status of its Workflow.
    ///
    /// Starting work on it starts each parent still in triage or todo, in the
    /// same batch. Finishing its parent's last open child prints the move
    /// that closes the parent, and reopening a child of a finished parent
    /// the move that reopens it; tm leaves both to you. It prints a line per
    /// change; `tm show` or `--json` gives the whole Issue.
    Move(moves::Move),
    /// Comment on an Issue.
    ///
    /// Comments keep a dated trail an edit would overwrite: progress, what
    /// was learned and changes of plan.
    Comment {
        /// The Issue's Key or full id.
        issue: String,
        /// The comment, or `-` to read it from stdin.
        body: String,
        /// Correct a comment you wrote instead: its id as `tm show` prints
        /// it, or last for your own latest.
        #[arg(long, value_name = "ID|last")]
        edit: Option<String>,
    },
    /// Link two Issues, such as `tm link TM-1 blocks TM-2`.
    ///
    /// A Link reads from either end: `tm link TM-2 is blocked by TM-1` makes
    /// the same one.
    Link {
        /// An Issue, the Link's name, then the other Issue.
        #[arg(value_name = "ISSUE NAME… ISSUE", num_args = 3.., required = true)]
        words: Vec<String>,
    },
    /// Remove a Link, named as `tm link` names it.
    Unlink {
        /// An Issue, the Link's name, then the other Issue.
        #[arg(value_name = "ISSUE NAME… ISSUE", num_args = 3.., required = true)]
        words: Vec<String>,
    },
    /// Show what happened to an Issue, oldest first.
    ///
    /// Each write with when, who, and the batch to give `tm undo`. Claim
    /// renewals are left out.
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
    /// Undo one command's writes, or your own last command.
    ///
    /// A command is named by the batch id it printed; with no batch, your own
    /// last command, stepping back each time.
    Undo {
        /// The batch id, or a unique start or end of it, 8 characters or more.
        batch: Option<String>,
        /// Show what would be restored, and write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Restore a field another Agent session's live Claim protects; only
        /// once that session has stopped work on it.
        #[arg(long)]
        force: bool,
    },
    /// Start the next Issue on your Short-lists or ready queue.
    ///
    /// As `tm start` does, it takes the first Issue your Short-lists hold
    /// that can start now (`TM_SHORTLIST` names one list), or the most urgent
    /// ready Issue when you have none; with nothing to take, says why.
    Next {
        /// Show the Issue it would take, without taking it.
        #[arg(long, conflicts_with_all = ["paths", "ttl_ms", "no_expiry"])]
        peek: bool,
        #[command(flatten)]
        lasts: claim::Lasts,
    },
    /// Start work on an Issue and print a branch name for it.
    ///
    /// An Agent claims it, it moves from triage or todo to its first active
    /// Status with its parents, and it gains its Assignee if it has none, all
    /// as one undo.
    Start {
        /// The Issue's Key or full id.
        issue: String,
        #[command(flatten)]
        lasts: claim::Lasts,
    },
    /// Claim an Issue for the Agent writing.
    ///
    /// Prints the Claim id that releases it.
    Claim {
        /// The Issue's Key or full id.
        issue: String,
        #[command(flatten)]
        lasts: claim::Lasts,
    },
    /// Renew this session's Claims during quiet work.
    ///
    /// Writes renew them too; this covers quiet work such as a long read or
    /// test run. A hook may run it after each tool use.
    Heartbeat {
        /// Renew only this Issue's Claim, by Key or full id.
        issue: Option<String>,
    },
    /// Hand an Issue's Claim to another Agent with a note.
    ///
    /// Its next session takes it with `tm start`.
    Handoff(handoff::Handoff),
    /// Release a Claim.
    ///
    /// The session holding it, and the human it is held for, may leave out
    /// the id `tm claim` printed.
    #[command(group = ArgGroup::new("what").required(true).args(["issue", "mine"]))]
    Release {
        /// The Issue's Key or full id.
        issue: Option<String>,
        /// The Claim id `tm claim` printed.
        claim: Option<uuid::Uuid>,
        /// Release every Claim this session took, as a harness does when the
        /// session ends.
        #[arg(long, conflicts_with_all = ["issue", "claim"])]
        mine: bool,
    },
    /// List who holds which Issue.
    ///
    /// Every held Claim with its time left, and the lapsed ones another Agent
    /// may take.
    Claims {
        /// List again each time another process writes or a held Claim
        /// lapses, until interrupted.
        #[arg(long)]
        watch: bool,
    },
    /// List the most recently active Agent sessions.
    ///
    /// Each with the Issues it wrote and the command that resumes it.
    Sessions {
        /// Show at most this many; 10 unless given.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Show your Short-lists, or create and change them.
    ///
    /// A Short-list is an ordered list of what to do next. A member that
    /// cannot be taken next stays on its list, marked with why: blocked,
    /// claimed, started, archived, an epic or in triage.
    Shortlist(shortlist::Shortlists),
    /// Ask a person a question the Issue waits on and take other work.
    ///
    /// The question joins its comments and the Issue leaves the ready queue;
    /// a Claim your session holds is kept for your Agent, which resumes it
    /// with `tm start` once answered. Use it when your human is not in the
    /// session or the decision can wait; when they are present, ask them
    /// there.
    Ask(ask::Ask),
    /// Answer the question an Issue waits on.
    ///
    /// The Agent that asked resumes it with `tm start`. A number or an
    /// option's words picks one of the question's options; any other reply
    /// is taken as written. At a terminal, `tm answer KEY` asks for the
    /// reply, and a bare `tm answer` walks each question waiting on you.
    Answer(ask::Answer),
    /// Withdraw a question you asked and settled yourself.
    ///
    /// Say why; the Issue waits on no one and your Agent resumes it with
    /// `tm start`.
    Withdraw(ask::Withdraw),
    /// Print the context an Agent session starts from.
    ///
    /// For a `SessionStart` hook.
    Prime,
    /// Print how to work with tm.
    ///
    /// Finding, starting, recording and finishing work.
    Guide,
    /// Open the screens over this Workspace in the terminal.
    Tui,
    /// List the Notifications of you, or mark them read.
    ///
    /// A comment, question, answer or description that mentions you notifies
    /// you; unread ones are listed unless --all is given.
    Notifications(notifications::Notifications),
    /// Remember a fact about this Project for every session.
    ///
    /// Every session's tm prime shows it. A memory is a durable fact a future
    /// session needs, such as how to run the tests or a trap in the build.
    /// Progress on an Issue belongs in tm comment, and what AGENTS.md already
    /// says needs no memory. Memories stay in this machine's tm, not in git.
    Remember(memory::Remember),
    /// List, search or show this Project's memories.
    ///
    /// Search them by words, or show one by its key.
    Memories(memory::Memories),
    /// Forget a memory; tm undo brings it back.
    Forget(memory::Forget),
    /// Create this repo's Workspace and map the repo to it.
    ///
    /// The map is .config/tm.toml, written after showing the file.
    ///
    /// It also offers a pointer to `tm guide` for AGENTS.md or CLAUDE.md,
    /// written only on a yes from a person at a terminal, which an Agent is
    /// given to add itself; and points at `tm setup claude` for the hooks.
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
    /// Write tm's hooks into an agent harness's settings for this repo.
    ///
    /// The hooks run tm prime when a session starts, a heartbeat after each
    /// tool use, and release the session's Claims when it ends; Claude Code's
    /// also warn before an edit to a file another session's Claim covers. Shows the
    /// diff and writes only on a person's yes; an Agent is only shown it. Claude
    /// Code's go in .claude/settings.local.json, yours alone, unless you choose
    /// the shared .claude/settings.json.
    Setup(setup::Setup),
    /// Inspect the Scheme.
    ///
    /// With no subcommand, print it as `tm scheme show` does.
    Scheme {
        #[command(subcommand)]
        action: Option<SchemeAction>,
    },
    /// Read this repo's git history; tm never writes to git.
    Git {
        #[command(subcommand)]
        action: GitAction,
    },
    /// Write this Workspace's log to stdout as JSONL, for `tm import`.
    ///
    /// There `--json` changes only how an error prints.
    Export {
        /// Write it to tm's home as exports/<Workspace>/<date>.jsonl instead,
        /// unless nothing changed since the newest one there, and remove old
        /// ones by the [backups] policy in tm's config.toml.
        #[arg(long)]
        dated: bool,
    },
    /// Restore an export, or import another tracker's Issues.
    ///
    /// An export restores into its Workspace on this machine, creating the
    /// Workspace when this machine lacks it; with `--records`, its Issue
    /// records are written into this repo's Workspace instead, and with
    /// `--from`, another tracker's export as records.
    Import {
        /// The export, records file or tracker export, or `-` to read it from
        /// stdin.
        file: String,
        /// Write the file's Issue records as ordinary writes, keeping current
        /// state and comments but not history.
        #[arg(long)]
        records: bool,
        /// Write another tracker's export as records: `bd export`'s, or a
        /// Trekker database such as .trekker/trekker.db, read-only.
        #[arg(long, value_enum, conflicts_with = "records")]
        from: Option<export::Adapter>,
        /// Check the file and the Workspace, and write nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Rebuild every table from the log.
    Rebuild,
    /// List logged operations that did not apply, and lost Claims.
    ///
    /// A Claim is listed when another take, renewal or release won it.
    Doctor,
    /// What the hooks tm setup writes run, each reading the hook's JSON on
    /// stdin: codex's session, and a Claude Code sub-agent's id.
    #[command(hide = true)]
    Hook {
        #[command(subcommand)]
        action: HookAction,
    },
}

#[derive(Subcommand)]
enum GitAction {
    /// Move the Issues merged commits close to done.
    ///
    /// Each Issue a `Closes TM-n` line names moves once its commit is on the
    /// default branch, as this clone last fetched it, with a comment naming
    /// the commit and pull request it shipped in. Done Issues are skipped, so
    /// it is safe to run after every merge.
    Scan {
        /// Show what would move, and write nothing.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum HookAction {
    /// Print tm prime for the session that is starting.
    SessionStart {
        /// The harness running the hook.
        harness: hook::Harness,
    },
    /// Make a Claude Code sub-agent's Bash command that runs tm write as that
    /// sub-agent, by exporting `TM_AGENT` first.
    PreBash {
        /// The harness running the hook.
        harness: ClaudeOnly,
    },
    /// Tell a Claude Code Agent about to edit a file which other sessions'
    /// Claims cover it, without blocking the edit.
    PreEdit {
        /// The harness running the hook.
        harness: ClaudeOnly,
    },
    /// Renew this session's Claims as tm heartbeat does, printing the
    /// harness's hook output only for a warning not yet given this session.
    Heartbeat {
        /// The harness running the hook.
        harness: hook::Harness,
    },
    /// Release a finished Claude Code sub-agent's Claims.
    SubagentStop {
        /// The harness running the hook.
        harness: ClaudeOnly,
    },
    /// Release the session's Claims, a Claude Code session's sub-agents'
    /// included.
    SessionEnd {
        /// The harness running the hook.
        harness: hook::Harness,
    },
}

/// The harness of a hook only Claude Code has.
#[derive(Clone, Copy, clap::ValueEnum)]
enum ClaudeOnly {
    /// Claude Code.
    Claude,
}

#[derive(Subcommand)]
enum SchemeAction {
    /// Print the Scheme's types, Workflows and fields.
    ///
    /// With a type, that type's Workflow and the fields it carries.
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
    // The harness reads only its own hook format, which JSON mode must not
    // wrap, even when the hook's arguments fail to parse. Before a command,
    // clap takes --json once; anything else ends parsing or is refused.
    let command = match args.get(1).map(String::as_str) {
        Some("--json") => args.get(2),
        _ => args.get(1),
    };
    let json = output::json_mode(&args, std::env::var("TM_JSON").ok().as_deref())
        && command.is_none_or(|arg| arg != "hook");
    if json {
        output::install_panic_hook();
        // A script reads one JSON document, never a question.
        prompt::hold();
    }
    let parsed = list::filter_last(&mut Cli::command(), std::env::args_os().collect());
    let cli = match cli()
        .try_get_matches_from(parsed)
        .and_then(|matches| Cli::from_arg_matches(&matches))
    {
        Ok(cli) => cli,
        Err(err) => {
            // A fix built from lossy arguments would write what clap refuses.
            let exact: Option<Vec<String>> = std::env::args_os()
                .map(|arg| arg.into_string().ok())
                .collect();
            return parse_failed(json, &err, exact.as_deref());
        }
    };
    if matches!(cli.command, Some(Command::Export { dated: false })) {
        return match export::export() {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => output::finish(json, Err(err), &[]),
        };
    }
    if let Some(Command::Hook { action }) = &cli.command {
        let input = hook::input();
        let enter = |harness| {
            input
                .clone()
                .and_then(|input| hook::enter(harness, input.as_ref()))
        };
        let result = match *action {
            HookAction::SessionStart { harness } => {
                enter(harness).and_then(|_| hook::session_start())
            }
            HookAction::PreBash { .. } => input.and_then(|input| hook::pre_bash(input.as_ref())),
            HookAction::PreEdit { .. } => Ok(hook::pre_edit(input)),
            HookAction::Heartbeat { harness } => enter(harness).map(|_| hook::heartbeat()),
            HookAction::SubagentStop { .. } => {
                enter(hook::Harness::Claude).and_then(hook::subagent_stop)
            }
            HookAction::SessionEnd { harness } => {
                enter(harness).and_then(|_| hook::session_end(harness))
            }
        };
        return output::finish(false, result, &[]);
    }
    match cli.command {
        Some(Command::List {
            filter,
            limit,
            all,
            watch: true,
        }) => watch::run(json, || list::run(&filter, limit, all, json)),
        Some(Command::Ready { limit, watch: true }) => watch::run(json, || ready::run(limit, json)),
        Some(Command::Claims { watch: true }) => watch::run(json, claim::list),
        Some(Command::Tui) => tui::run(json),
        command => output::finish(json, run(command, json), &[]),
    }
}

fn run(command: Option<Command>, json: bool) -> Result<Output, CliError> {
    match command {
        None => {
            let help = cli().render_help().to_string();
            Ok(Output::new(help.clone(), json!({ "help": help })))
        }
        Some(Command::Scheme {
            action: None | Some(SchemeAction::Show { issue_type: None }),
        }) => {
            let scheme = Scheme::built_in();
            Ok(Output::new(scheme_text(&scheme), scheme_json(&scheme)))
        }
        Some(Command::Scheme {
            action:
                Some(SchemeAction::Show {
                    issue_type: Some(issue_type),
                }),
        }) => scheme_type(&Scheme::built_in(), &issue_type),
        Some(Command::Init { key, name, yes }) => init::run(&init::Init { key, name, yes }, json),
        Some(Command::Start { issue, lasts }) => start::run(&issue, &lasts),
        Some(Command::Next { peek, lasts }) => next::run(peek, &lasts),
        Some(Command::Claim { issue, lasts }) => claim::take(&issue, &lasts),
        Some(Command::Heartbeat { issue }) => claim::heartbeat(issue.as_deref()),
        Some(Command::Release {
            issue: Some(issue),
            claim,
            ..
        }) => claim::release(&issue, claim),
        Some(Command::Release { issue: None, .. }) => claim::release_mine(),
        Some(Command::Handoff(args)) => handoff::run(&args),
        Some(Command::Ask(args)) => ask::ask(&args),
        Some(Command::Answer(args)) => ask::answer(&args, json),
        Some(Command::Withdraw(args)) => ask::withdraw(&args),
        Some(Command::Notifications(args)) => notifications::run(&args),
        Some(Command::Remember(args)) => memory::remember(&args),
        Some(Command::Memories(args)) => memory::memories(&args),
        Some(Command::Forget(args)) => memory::forget(&args),
        Some(Command::Setup(setup)) => setup::run(&setup, json),
        Some(Command::Hook { .. }) => unreachable!("main runs hooks outside JSON mode"),
        Some(Command::Git {
            action: GitAction::Scan { dry_run },
        }) => scan::run(dry_run),
        Some(Command::Comment { issue, body, edit }) => {
            comment::run(&issue, &body, edit.as_deref())
        }
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
        Some(Command::Edit(args)) => edit::run(&args),
        Some(Command::Link { words }) => link::run(&words, true),
        Some(Command::Unlink { words }) => link::run(&words, false),
        Some(Command::List {
            filter, limit, all, ..
        }) => list::run(&filter, limit, all, json),
        Some(Command::Move(args)) => moves::run(&args),
        Some(Command::Prime) => prime::run(json),
        Some(Command::Guide) => Ok(guide::run()),
        Some(Command::Tui) => unreachable!("main opens the TUI itself"),
        Some(Command::Ready { limit, .. }) => ready::run(limit, json),
        Some(Command::Claims { .. }) => claim::list(),
        Some(Command::Sessions { limit }) => session::list(limit, json),
        Some(Command::Shortlist(args)) => shortlist::run(&args, json),
        Some(Command::Show { issue, comments }) => show::run(&issue, comments.unwrap_or_default()),
        Some(Command::History { issue, limit, all }) => history::run(&issue, limit, all),
        Some(Command::Undo {
            batch,
            dry_run,
            force,
        }) => undo::run(batch.as_deref(), dry_run, force),
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
fn parse_failed(json: bool, err: &clap::Error, args: Option<&[String]>) -> ExitCode {
    if err.kind() == ErrorKind::UnknownArgument
        && let Some(args) = args
        && let Some(ContextValue::String(flag)) = err.get(ContextKind::InvalidArg)
        && let Some(refused) = field_flag::refused(Cli::command(), args, flag)
    {
        return output::finish(json, Err(refused), &[]);
    }
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
    if err.kind() == ErrorKind::DisplayVersion {
        let version = json!({ "version": env!("CARGO_PKG_VERSION") });
        return output::finish(true, Ok(Output::new(String::new(), version)), &[]);
    }
    let rendered = err.render().to_string();
    if shown {
        return output::finish(
            true,
            Ok(Output::new(String::new(), json!({ "help": rendered }))),
            &[],
        );
    }
    let (message, tips) = usage_message(&rendered);
    let mut err = CliError::new("usage", message);
    if let Some(tips) = tips {
        err = err.with("tip", tips);
    }
    output::finish(true, Err(err), &[])
}

/// Clap's rendered error as one line, and its tips, which are hints rather
/// than a runnable fix (ADR 0011): its message,
/// then what it names, such as missing arguments, from the indented lines
/// after it; a blank line, then the tips.
fn usage_message(rendered: &str) -> (String, Option<String>) {
    let mut lines = rendered.lines();
    let first = lines.next().unwrap_or_default();
    let message = first.strip_prefix("error: ").unwrap_or(first);
    let named: Vec<&str> = lines
        .by_ref()
        .take_while(|line| line.starts_with("  "))
        .map(str::trim)
        .collect();
    let tips: Vec<&str> = lines
        .filter_map(|line| line.trim().strip_prefix("tip: "))
        .collect();
    let message = if named.is_empty() {
        message.to_owned()
    } else {
        format!("{message} {}", named.join(", "))
    };
    (message, (!tips.is_empty()).then(|| tips.join("; ")))
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

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::{Cli, GROUPS, cli};

    /// The longest first line a command list shows before it wraps at 80
    /// columns, past its two-space indent and the name column.
    const ABOUT: usize = 64;

    /// `tm` and every command under it, by path such as `tm git scan`, each
    /// with whether a help list shows it: neither it nor a parent is hidden.
    fn commands() -> Vec<(String, clap::Command, bool)> {
        fn walk(
            path: &str,
            command: &clap::Command,
            shown: bool,
            out: &mut Vec<(String, clap::Command, bool)>,
        ) {
            let shown = shown && !command.is_hide_set();
            out.push((path.to_owned(), command.clone(), shown));
            for sub in command.get_subcommands() {
                walk(&format!("{path} {}", sub.get_name()), sub, shown, out);
            }
        }
        let mut out = Vec::new();
        walk("tm", &Cli::command(), true, &mut out);
        out
    }

    #[test]
    fn only_commands_with_a_json_form_offer_json_in_their_help() {
        fn walk(path: &str, command: &mut clap::Command, out: &mut Vec<(String, String)>) {
            out.push((path.to_owned(), command.render_help().to_string()));
            for sub in command.get_subcommands_mut() {
                walk(&format!("{path} {}", sub.get_name()), sub, out);
            }
        }
        let mut built = cli();
        built.build();
        let mut helps = Vec::new();
        walk("tm", &mut built, &mut helps);
        // Clap's own help commands take no flags.
        helps.retain(|(path, _)| !path.contains(" help"));
        for (path, help) in &helps {
            let without = path.starts_with("tm hook") || path == "tm tui";
            assert_eq!(!help.contains("--json"), without, "{path}:\n{help}");
        }
        for checked in ["tm tui", "tm hook session-start", "tm list"] {
            assert!(helps.iter().any(|(path, _)| path == checked), "{checked}");
        }
    }

    #[test]
    fn a_usage_message_names_what_clap_lists_and_keeps_its_tips() {
        let message = |args: &[&str]| {
            let err = cli().try_get_matches_from(args).unwrap_err();
            super::usage_message(&err.render().to_string())
        };
        assert_eq!(
            message(&["tm", "handoff", "X-1"]),
            (
                "the following required arguments were not provided: --to <AGENT>, <NOTE>".into(),
                None
            )
        );
        assert_eq!(
            message(&["tm", "list", "--limt", "3"]),
            (
                "unexpected argument '--limt' found".into(),
                Some("a similar argument exists: '--limit'".into())
            )
        );
    }

    #[test]
    fn every_command_says_what_it_does_in_one_short_line() {
        let long: Vec<String> = commands()
            .into_iter()
            .skip(1)
            .filter_map(|(path, command, shown)| {
                let about = command.get_about().map(ToString::to_string);
                match about.as_deref().map(str::trim) {
                    None | Some("") => Some(format!("{path}: no description")),
                    Some(about) if shown && about.chars().count() > ABOUT => {
                        Some(format!("{path}: {about}"))
                    }
                    Some(_) => None,
                }
            })
            .collect();
        assert!(long.is_empty(), "{}", long.join("\n"));
    }

    #[test]
    fn only_the_hooks_command_is_left_out_of_the_list() {
        let hidden: Vec<String> = commands()
            .into_iter()
            .filter(|(_, command, _)| command.is_hide_set())
            .map(|(path, ..)| path)
            .collect();
        assert_eq!(hidden, ["tm hook"]);
    }

    #[test]
    fn tm_help_lists_every_command_once_under_a_heading() {
        let mut command = cli();
        command.build();
        let shown: Vec<&str> = command
            .get_subcommands()
            .filter(|sub| !sub.is_hide_set())
            .map(clap::Command::get_name)
            .collect();
        let grouped: Vec<&str> = GROUPS
            .iter()
            .flat_map(|(_, names)| names.iter().copied())
            .collect();
        let mut missing: Vec<&str> = shown
            .iter()
            .copied()
            .filter(|name| !grouped.contains(name))
            .collect();
        missing.extend(grouped.iter().copied().filter(|name| !shown.contains(name)));
        assert!(
            missing.is_empty(),
            "in GROUPS or tm's commands, not both: {missing:?}"
        );
        let mut unique = grouped.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), grouped.len(), "a command under two headings");

        let help = command.clone().render_help().to_string();
        let width = grouped.iter().map(|name| name.len()).max().unwrap_or(0);
        for (heading, names) in GROUPS {
            assert!(
                help.contains(&format!("\n{heading}:\n")),
                "{heading} in {help}"
            );
            for name in names {
                assert_eq!(
                    help.matches(&format!("\n  {name} ")).count(),
                    1,
                    "{name} once in {help}"
                );
                let about = command.find_subcommand(name).unwrap().get_about().unwrap();
                let line = format!("\n  {name:width$}  {about}\n");
                assert!(help.contains(&line), "{line:?} in {help}");
            }
        }
        assert!(
            help.contains("\nOptions:\n") && help.contains("--json"),
            "{help}"
        );
        let wide: Vec<&str> = help
            .lines()
            .filter(|line| line.chars().count() > 80 && !line.trim_start().starts_with('-'))
            .collect();
        assert!(wide.is_empty(), "{wide:#?}");
    }

    #[test]
    fn every_argument_is_described() {
        let bare: Vec<String> = commands()
            .into_iter()
            .flat_map(|(path, command, _)| {
                command
                    .get_arguments()
                    .filter(|arg| !arg.is_hide_set() && arg.get_help().is_none())
                    .map(move |arg| format!("{path} {}", arg.get_id()))
                    .collect::<Vec<_>>()
            })
            .collect();
        assert!(bare.is_empty(), "{}", bare.join("\n"));
    }
}
