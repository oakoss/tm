# Conventions

- Commit messages: `type(scope): summary`, checked by cog in the commit-msg hook and in CI.
- One branch per reviewable unit: `<type>/tm-<id>-<theme>`, where the id is the bead or parent bead the branch carries; a branch that carries no bead is `<type>/<theme>`.
- Commits are signed; the pre-push hook refuses unsigned ones.
- Don't push or open pull requests unless asked.
