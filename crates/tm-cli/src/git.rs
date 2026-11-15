//! The git processes tm starts.

use std::io;
use std::path::Path;
use std::process::{Command, Output};

/// What `git args` printed in `dir`, however it exited. It takes no
/// optional locks, so it never holds up another git, and git's trace
/// variables are left out, so stderr carries only what git has to say.
pub(crate) fn output(dir: &Path, args: &[&str]) -> io::Result<Output> {
    let _git = tracing::debug_span!("git", ?args).entered();
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(dir)
        .env("GIT_OPTIONAL_LOCKS", "0");
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_TRACE") {
            command.env_remove(name);
        }
    }
    command.output()
}
