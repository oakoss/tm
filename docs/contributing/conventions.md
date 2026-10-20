# Conventions

- Commit messages: `type(scope): summary`, checked by cog in the commit-msg hook and in CI.
- One branch per reviewable unit: `<type>/tm-<n>-<theme>`, where `TM-n` is the Issue or parent Issue the branch carries; a branch that carries no Issue is `<type>/<theme>`.
- Commits are signed; the pre-push hook refuses unsigned ones.
- Don't push or open pull requests unless asked.
