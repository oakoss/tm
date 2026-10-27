# Security policy

## Supported versions

Only the latest release receives security fixes; pre-1.0 minor bumps may be breaking.

## Reporting a vulnerability

Use GitHub's [private vulnerability reporting](https://github.com/oakoss/tm/security/advisories/new) on this repository. It keeps the disclosure private and creates a draft advisory for coordinated disclosure.

If the form is unavailable, email `jbabin91@gmail.com` with `[tm security]` in the subject line.

Please include:

- A description of the issue and its impact
- Steps to reproduce (a minimal `tm` invocation and any required config)
- Affected version (`tm --version` output)
- Any proposed mitigation

## Response expectations

- **Acknowledgement** within 72 hours
- **Initial assessment** within 7 days
- **Fix or mitigation timeline** communicated after assessment

This is a solo-maintained project; response times are best effort, not a contractual SLA. Coordinated-disclosure timelines are negotiable based on severity.

## Scope

In scope:

- The `tm` binary's behavior given hostile input: malformed import files, hostile config, crafted Issue content.
- Leaks of per-machine values (hosts, tokens) into tracked files or output.
- The local web UI's access control (`tm ui` binds to localhost with a token).
- Supply-chain integrity of releases.

Out of scope:

- Vulnerabilities in upstream dependencies; report those to their maintainers. Advisories are tracked with `cargo-deny`, and patched versions ship in the next release.
- Issues in the AI agents `tm` integrates with; those have their own disclosure channels.
