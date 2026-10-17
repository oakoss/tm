# Pull requests

PR bodies follow [`.github/PULL_REQUEST_TEMPLATE.md`](../../.github/PULL_REQUEST_TEMPLATE.md): a `Summary` (problem first, then the change and why this approach), plus a `Notes` section only when something non-obvious earned it, such as rejected alternatives, accepted limitations, or dismissed review findings with their evidence.

- Squash-merge uses the PR title and body as the mainline commit message, so write the body as permanent history and delete the template's guidance comments.
- The PR title is the commit subject: `type(scope): summary`.
- No Claude session links in PR bodies or commit messages.
- No Test plan checklist. Verification is `mise run check`, `mise run test`, `mise run audit` and the PR's Checks tab.
