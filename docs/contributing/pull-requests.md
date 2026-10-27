# Pull requests

PR bodies follow [`.github/PULL_REQUEST_TEMPLATE.md`](../../.github/PULL_REQUEST_TEMPLATE.md): a `Summary` (problem first, then the change and why this approach), plus a `Notes` section only when something non-obvious earned it, such as rejected alternatives, accepted limitations, or dismissed review findings with their evidence.

- Squash-merge uses the PR title and body as the mainline commit message, so write the body as permanent history and delete the template's guidance comments.
- The PR title is the commit subject: `type(scope): summary`.
- No Claude session links in PR bodies or commit messages.
- No Test plan checklist. Verification is the PR's Checks tab and `mise run ci`, which runs `check`, `test`, `audit`, `scan-secrets` and `coldstart` in turn.
- A PR that changes what the `tm` binary does carries a bump file for `tm-cli`, the one versioned crate: `oakum add --packages "tm-cli:patch" --message "..."` writes one under `.changeset/`, and its message becomes the release note. CI's `Bump files` job fails a pull request that changes anything under `crates/` without adding or editing a bump file of its own, internal crates included, and runs `oakum check --strict`. `oakum add --empty` marks a change that releases nothing.

## Releases

Merging to `main` with bump files, once CI Summary passes there, makes oakum open or update the `oakum/version-packages` pull request. Merging that pull request, once its CI passes on `main`, tags `v<version>`; the tag runs `release.yml`, which builds the binaries with cargo-dist, uploads them to the GitHub release and updates the Homebrew formula in `oakoss/homebrew-tap`.
