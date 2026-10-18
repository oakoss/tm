# Workspaces, repo mapping and Key assignment

- Date: 2026-10-17
- Author: Claude (research pass for Jace Babin)
- Scope: Where Workspaces live on disk, how a repo maps to a Workspace and Project without per-machine values in the repo, what a Project key may look like, and how the local authority assigns sequential Keys that a rebuild reproduces and that are never reused. Input to bead tm-u8g.5.

Method note: sources were read on 2026-10-17. GitHub sources were cloned and pinned to the commits listed under Sources, and permalinks use those commits. WebFetch summarises pages, so quotes from docs sites are near-verbatim. Two probes ran on an Apple M2 Max under macOS 27.0.1:

- [scripts/key-probe/](scripts/key-probe/) is a standalone crate on `rusqlite =0.40.2` (bundled SQLite 3.53.2, the same engine as `tm-store`), with WAL, `synchronous=NORMAL` and a 5 s busy timeout like `Store::open`. It runs N writer processes against one file. Built with `CARGO_TARGET_DIR=<scratch>/target cargo build --release --offline`.
- [scripts/git-probe.sh](scripts/git-probe.sh) runs git 2.56.0 against fixture repos in a scratch directory, with `GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1`.

Neither was run on Linux. Anything not quoted or measured is labeled **inferred**. Output is under Raw data, with the scratch path replaced by `$SCRATCH`.

## Question

Bead tm-u8g.5 is "Add Workspaces, repo mapping and Key assignment". Its acceptance criteria: `tm init` in a repo creates a repo-scoped Workspace and writes only `.config/tm.toml`, and concurrent creates never duplicate a Key. This note answers five questions:

1. What lives under `~/.tm`? Should tm honour `XDG_*` on Linux, and what applies on macOS? How do beads, jj, git, gh, mise and cargo split per-user and per-repo state?
2. What must `.config/tm.toml` contain so that it holds no per-machine values, and how does a machine resolve it to a store? How do mapping by remote URL and monorepo prefixes work? How is the repo root found without writes, and what must `tm init` leave alone?
3. What format may a Project key have, how is it kept unique, and is a Project recorded in the log or in config?
4. How does the local authority assign sequential Keys under concurrent writers? Where is the Key recorded so that a rebuild reproduces it and a purge never frees it? Where do provisional Keys fit?
5. Where do design.md, the ADRs and the glossary conflict, or leave a decision open?

## Sources

In-repo: [AGENTS.md](../../../AGENTS.md), [design.md](../../specs/design.md) ("Workspaces, Projects and config", "Identity", "Import and export"), [GLOSSARY.md](../../../GLOSSARY.md), ADRs [0001](../../decisions/0001-local-store-is-plain-sqlite-without-a-daemon.md), [0003](../../decisions/0003-issue-data-lives-outside-code-repos.md), [0004](../../decisions/0004-one-authority-assigns-keys.md), [0005](../../decisions/0005-operation-log-is-the-source-of-truth.md), [0006](../../decisions/0006-operation-merge-ordering-and-versioning.md) and [0007](../../decisions/0007-the-projection-does-not-depend-on-the-scheme.md), [projection-schema.md](../store/projection-schema.md), `crates/tm-store/src/{schema,lib,apply}.rs` and `crates/tm-core/src/op.rs` at `c3e2db2`.

Per-user and per-repo layout:

- gastownhall/beads @ `4a67d0d0288c1b4ee23e17fc896f47616931ba2d`
- jj-vcs/jj @ `8555ddeb8db13661d1d4759ab88f2f0cc782986f`
- cli/cli @ `fa92838bef95fe4a29c1587693885d5e9bdb6cf1`; cli/go-gh @ `7de215ca6ceb82b2944d7e8bf1ed2a1ac606a5ca`
- jdx/mise @ `c571d2cc5792b63dc116406746fec7feb6016115` (v2026.10.13)
- dirs-dev/directories-rs @ `4d76f1a5a0a979de3cac4c1ffb53c8b5a6f481a7` (v6.0.0); dirs-dev/dirs-sys-rs @ `8bcd4aa2c35990d57a2cff2953793525fc42709c`; lunacookies/etcetera @ `80592b7426676e98df57b82a5bbb028856e68281` (v0.12.0)
- XDG Base Directory spec 0.8: <https://specifications.freedesktop.org/basedir/latest/>
- Apple File System Programming Guide (archived 2018-04-09): <https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/FileSystemProgrammingGuide/FileSystemOverview/FileSystemOverview.html>
- git-config FILES: <https://git-scm.com/docs/git-config>; gitrepository-layout: <https://git-scm.com/docs/gitrepository-layout>
- Cargo home: <https://doc.rust-lang.org/cargo/guide/cargo-home.html>; Cargo config: <https://doc.rust-lang.org/cargo/reference/config.html>

Git URLs and discovery:

- git/git @ `c51408889c3b372814bc20b9431c3b74d0d268eb`: `Documentation/urls.adoc`, `config/url.adoc`, `git-remote.adoc`, `git.adoc`, `git-status.adoc`
- GitoxideLabs/gitoxide @ `ad0202e3ab396d752a56664001c134f48d834177` (gix-url 0.40.0, gix-discover 0.57.0, gix 0.89.0)
- tjtelan/git-url-parse-rs @ `4ff69ac399466c72c01144db3bb4404861c45292` (0.6.0)

Project keys and counters:

- Jira Data Center, key format: <https://confluence.atlassian.com/adminjiraserver/changing-the-project-key-format-938847081.html>; editing a key: <https://confluence.atlassian.com/adminjiraserver107/editing-a-project-key-1595114606.html>; previous keys: <https://support.atlassian.com/jira/kb/how-to-list-all-previous-project-keys-in-jira/>; counter reset: <https://support.atlassian.com/jira/kb/reset-the-project-counter-and-reuse-jira-issue-keys-after-moving-issues/>; JRACLOUD-78272: <https://jira.atlassian.com/browse/JRACLOUD-78272>
- Jira Cloud: <https://support.atlassian.com/jira-software-cloud/docs/edit-a-projects-details/>, <https://support.atlassian.com/jira-cloud-administration/docs/configure-jira-application-options/>
- makeplane/plane @ `879ce8b5c6b4fe21efff4789a7062747f46d6383`
- gitlab-org/gitlab @ `1d58eb23e25fffbf37bacc83ca32aa9f99a4c793` (GitHub mirror path)
- SQLite: <https://www.sqlite.org/lang_transaction.html>, <https://www.sqlite.org/c3ref/busy_handler.html>, <https://www.sqlite.org/autoinc.html>
- PostgreSQL sequence functions: <https://www.postgresql.org/docs/current/functions-sequence.html>

## Findings

### 1. Per-user versus per-repo state in comparable tools

| Tool | Per-user location on Linux | On macOS | Per-repo state | Repo-scoped state kept per user |
|---|---|---|---|---|
| beads | `~/.config/bd/config.yaml`; `~/.beads/shared-server/` | the same hard-coded `~/.config/bd` | `.beads/`: database, `metadata.json`, `config.yaml`, hooks | none; everything sits in `.beads/` |
| jj | `$XDG_CONFIG_HOME/jj` or `~/.config/jj` | the same (Application Support stopped being read in 0.36.0) | `.jj/` | `~/.config/jj/repos/<config-id>/config.toml`; the repo holds only the id |
| git | `$XDG_CONFIG_HOME/git/config`, `~/.gitconfig` | the same | `.git/config`, `config.worktree` | none |
| gh | `GH_CONFIG_DIR`, else `$XDG_CONFIG_HOME/gh`, else `~/.config/gh`; state and data under the `XDG_STATE_HOME` and `XDG_DATA_HOME` equivalents | `~/.config/gh` too | none | none |
| mise | `~/.config/mise`, `~/.local/share/mise`, `~/.local/state/mise`, `~/.cache/mise` | the same, except cache in `~/Library/Caches/mise` | `mise.toml`, `.config/mise.toml` and others | trust decisions in `~/.local/state/mise/trusted-configs/`, keyed by a hash of the canonical path |
| cargo | `$CARGO_HOME`, else `~/.cargo`; no XDG | the same | `.cargo/config.toml` in each ancestor directory | none |

**beads** puts per-machine values and the store inside the repo, and edits files tm promises not to touch:

- `bd init` creates `.beads/` with `metadata.json`, whose fields include `dolt_server_host` ("Server host (default: 127.0.0.1)") and `dolt_data_dir` ("absolute path") ([configfile.go#L30-L36](https://github.com/gastownhall/beads/blob/4a67d0d0288c1b4ee23e17fc896f47616931ba2d/internal/configfile/configfile.go#L30-L36)).
- It sets `core.hooksPath` with `git ... config --local core.hooksPath <hooksDir>` ([init_git_hooks.go#L433](https://github.com/gastownhall/beads/blob/4a67d0d0288c1b4ee23e17fc896f47616931ba2d/cmd/bd/init_git_hooks.go#L433)).
- It writes AGENTS.md and agent settings ([init.go#L2326](https://github.com/gastownhall/beads/blob/4a67d0d0288c1b4ee23e17fc896f47616931ba2d/cmd/bd/init.go#L2326)).
- It commits with `-c core.hooksPath= commit --no-verify -m "bd init: initialize beads issue tracking"` ([init.go#L3972](https://github.com/gastownhall/beads/blob/4a67d0d0288c1b4ee23e17fc896f47616931ba2d/cmd/bd/init.go#L3972)).

The issue prefix defaults to `filepath.Base(cwd)` ([init.go#L1034](https://github.com/gastownhall/beads/blob/4a67d0d0288c1b4ee23e17fc896f47616931ba2d/cmd/bd/init.go#L1034)). Validation disagrees between commands: `rename-prefix` accepts `^[a-z][a-z0-9-]*$`, while `bd doctor` accepts `^[a-zA-Z][a-zA-Z0-9_-]*$` up to 20 characters.

beads finds the repo by shelling out to `git rev-parse --git-dir --git-common-dir --show-toplevel` ([gitdir.go#L124](https://github.com/gastownhall/beads/blob/4a67d0d0288c1b4ee23e17fc896f47616931ba2d/internal/git/gitdir.go#L124)). It tries an in-process equivalent on Linux only, because "elsewhere path case-folding (darwin, windows) and ownership semantics … make an exact in-process answer harder to prove". It refuses `bd init` in an untracked `$HOME` unless `BEADS_DIR` is set ([init.go#L426](https://github.com/gastownhall/beads/blob/4a67d0d0288c1b4ee23e17fc896f47616931ba2d/cmd/bd/init.go#L426)). Worktrees share one database through the git common directory ([worktree_cmd.go#L52-L53](https://github.com/gastownhall/beads/blob/4a67d0d0288c1b4ee23e17fc896f47616931ba2d/cmd/bd/worktree_cmd.go#L52-L53)).

**jj moved repo config out of the repo** in 0.38.0 (2026-02-04): "Per-repo and per-workspace config is now stored outside the repo, for security reasons … `.jj/repo/config.toml` and `.jj/workspace-config.toml` should no longer be used" ([CHANGELOG.md#L1115-L1122](https://github.com/jj-vcs/jj/blob/8555ddeb8db13661d1d4759ab88f2f0cc782986f/CHANGELOG.md#L1115-L1122)). The design doc names the threat as the "zip file problem": a malicious `[fix.tools]` entry in a repo you downloaded ([secure-config.md](https://github.com/jj-vcs/jj/blob/8555ddeb8db13661d1d4759ab88f2f0cc782986f/docs/design/secure-config.md)).

- The repo stores only a random id in `.jj/repo/config-id`, which is 10 random bytes (`CONFIG_ID_BYTES = 10`, [secure_config.rs#L38-L40](https://github.com/jj-vcs/jj/blob/8555ddeb8db13661d1d4759ab88f2f0cc782986f/lib/src/secure_config.rs#L38-L40)).
- The config itself lives in `<config_dir>/jj/repos/<id>/`, beside a `metadata.binpb` that records the repo path, so jj can tell a moved repo from a copied one.
- `jj config gc` deletes entries for repos that no longer exist.

This is the same split tm needs: an opaque id in the repo, everything else under the user's directory.

**Where the tools put per-user state on macOS.** Most CLI tools use XDG paths there too:

- jj resolves its directory through `etcetera::choose_base_strategy()`, which is XDG on macOS ([cli/src/config.rs#L371-L373](https://github.com/jj-vcs/jj/blob/8555ddeb8db13661d1d4759ab88f2f0cc782986f/cli/src/config.rs#L371-L373)).
- gh falls back to `filepath.Join(home, ".config", "gh")` ([go-gh config.go#L247-L314](https://github.com/cli/go-gh/blob/7de215ca6ceb82b2944d7e8bf1ed2a1ac606a5ca/pkg/config/config.go#L247-L314)).
- mise is special-cased only for its cache ([env.rs#L73-L103](https://github.com/jdx/mise/blob/c571d2cc5792b63dc116406746fec7feb6016115/crates/mise-util/src/env.rs#L73-L103)). `mise doctor` on this machine printed `config: ~/.config/mise`, `data: ~/.local/share/mise` and `state: ~/.local/state/mise`.
- The `directories` crate is the exception. It returns `~/Library/Application Support` for both `config_dir` and `data_dir` on macOS and ignores `XDG_*` there ([mac.rs#L9-L16](https://github.com/dirs-dev/directories-rs/blob/4d76f1a5a0a979de3cac4c1ffb53c8b5a6f481a7/src/mac.rs#L9-L16)).
- etcetera's README states the split: the XDG strategy "is used by most CLI tools & some GUI tools on each platform", and the Apple strategy "by most GUI applications" ([README.md#L50-L53](https://github.com/lunacookies/etcetera/blob/80592b7426676e98df57b82a5bbb028856e68281/README.md#L50-L53)).
- Apple's guide says Application Support holds "app-created data files, configuration files, templates" in a subdirectory named after the bundle id or company. It says nothing about command-line dotfiles.

**XDG rules.** The spec defines `XDG_CONFIG_HOME` (default `$HOME/.config`), `XDG_DATA_HOME` (`$HOME/.local/share`), `XDG_STATE_HOME` (`$HOME/.local/state`) and `XDG_CACHE_HOME` (`$HOME/.cache`). It also requires: "If an implementation encounters a relative path in any of these variables it should consider the path invalid and ignore it." etcetera and dirs-sys follow that rule. gh and mise accept relative values.

**Cargo** uses one home directory: "By default, the Cargo home is located in `$HOME/.cargo/`" and "You can alter the location of the Cargo home by setting the `CARGO_HOME` environmental variable". It never consults XDG.

### 2. Repo mapping, discovery and what `tm init` touches

**Reading the remote by hand misses remotes and rewrites.** Measured with [git-probe.sh](scripts/git-probe.sh) on a fixture repo. Its `origin` is `gh:oakoss/tm.git`, rewritten by `url."git@github.com:".insteadOf = gh:`, and its `upstream` is defined in a file pulled in by `[include]`:

| Read | `origin` | `upstream` |
|---|---|---|
| hand-parsed `.git/config` | `gh:oakoss/tm.git` | (not found) |
| `git config --get remote.<name>.url` | `gh:oakoss/tm.git` | `https://github.com/Oakoss/TM` |
| `git remote get-url <name>` | `git@github.com:oakoss/tm.git` | `https://github.com/Oakoss/TM` |

The git docs agree: for `get-url`, "Configurations for `insteadOf` and `pushInsteadOf` are expanded here" ([git-remote.adoc#L142-L145](https://github.com/git/git/blob/c51408889c3b372814bc20b9431c3b74d0d268eb/Documentation/git-remote.adoc#L142-L145)). When several prefixes match, "the longest match is used" ([config/url.adoc](https://github.com/git/git/blob/c51408889c3b372814bc20b9431c3b74d0d268eb/Documentation/config/url.adoc#L1-L30)). Only `git remote get-url` sees the URL git actually uses. gh also reads remotes through git (`git remote -v`), so it sees rewritten URLs too ([client.go#L165-L166](https://github.com/cli/cli/blob/fa92838bef95fe4a29c1587693885d5e9bdb6cf1/git/client.go#L165-L166)).

**In a linked worktree, `.git` is a file.** Measured: the worktree's `.git` contains `gitdir: $SCRATCH/git/repo/.git/worktrees/wt`. `rev-parse --git-common-dir` gives the main `.git`, and `remote get-url origin` works unchanged from the worktree. gitrepository-layout calls this a "gitfile … usually managed via the git submodule and git worktree commands", and says the `commondir` file points at the shared repository. A reader that expects `.git/` to be a directory fails in every worktree and submodule. gix-discover handles gitfiles, `GIT_CEILING_DIRECTORIES` and ownership trust, but leaves `GIT_DIR` and `safe.directory` to the full `gix` crate ([types.rs#L80-L179](https://github.com/GitoxideLabs/gitoxide/blob/ad0202e3ab396d752a56664001c134f48d834177/gix-discover/src/upwards/types.rs#L80-L179)).

**Read commands write nothing, except `git status`.** Measured: `rev-parse`, `config --get`, `remote get-url`, `log` and `for-each-ref` changed no file under `.git`. After touching a tracked file, plain `git status` rewrote `.git/index`, while `GIT_OPTIONAL_LOCKS=0 git status` changed nothing. The docs say `GIT_OPTIONAL_LOCKS=0` makes git skip "any optional sub-operations that require taking a lock. For example, this will prevent `git status` from refreshing the index as a side effect" ([git.adoc#L1030-L1036](https://github.com/git/git/blob/c51408889c3b372814bc20b9431c3b74d0d268eb/Documentation/git.adoc#L1030-L1036)).

**Shelling out costs about 7 ms per call.** Measured as the mean of 20 runs each of `rev-parse --show-toplevel` and `remote get-url origin`.

**Remote URL forms.** Git accepts `ssh://[user@]host[:port]/path`, `git://`, `http[s]://`, `file://`, local paths, `<transport>::<address>`, and the scp-like `[user@]host:path`. The scp form "is only recognized if there are no slashes before the first colon" ([urls.adoc#L18-L31](https://github.com/git/git/blob/c51408889c3b372814bc20b9431c3b74d0d268eb/Documentation/urls.adoc#L18-L31)). The parsers differ:

- **gix-url** applies git's rule, lowercases DNS-like hosts, and keeps the path as written: no `.git` stripping and no owner/repo extraction ([parse.rs#L49-L89](https://github.com/GitoxideLabs/gitoxide/blob/ad0202e3ab396d752a56664001c134f48d834177/gix-url/src/parse.rs#L49-L89)).
- **gh** turns any non-URL containing `:` into `ssh://` + the string with the first `:` replaced by `/`, skipping git's slash check, and strips the port ([git/url.go#L8-L61](https://github.com/cli/cli/blob/fa92838bef95fe4a29c1587693885d5e9bdb6cf1/git/url.go#L8-L61)). Repo identity then trims `.git`, lowercases the host, drops `www.` ([ghrepo/repo.go#L26-L76](https://github.com/cli/cli/blob/fa92838bef95fe4a29c1587693885d5e9bdb6cf1/internal/ghrepo/repo.go#L26-L76)) and compares owner and repo ignoring case.
- **git-url-parse 0.6.0** extracts owner and repo and drops `.git`, but does not lowercase the host.

**Discovery by walking up.** Nothing in design.md requires git to find the repo file. The rules below are **inferred**:

- tm can walk up from the working directory, looking for `.tm.toml` or `.config/tm.toml`, and stop at the first directory that contains a `.git` entry (file or directory).
- Stopping at `.git` keeps a submodule's walk inside the submodule, and makes a linked worktree read its own checked-out copy of the file.
- Because the walk stops there, it never reaches `$HOME`. Without that stop, someone who keeps dotfiles in a git repo at `$HOME` and has an XDG-style `~/.config/tm.toml` would find it read as a repo file. beads guards `$HOME` explicitly for a related reason.
- The monorepo prefix is the working directory relative to the directory holding the file, so the mapped case needs no subprocess.

### 3. Project keys

| Tracker | Format | Unique within | After a rename | Number reuse |
|---|---|---|---|---|
| Jira Cloud | "at least two characters", "start with an uppercase letter", "only uppercase letters or numbers"; maximum length set by admin, 2 to 255, default 10 | site | "we automatically save your previous space keys … links, filters, and queries using your previous key continue working"; an old key cannot be taken by a new space | `project.pcounter`; deleted numbers not reused (community answers only, **inferred**) |
| Jira Data Center | default `[A-Z][A-Z]+`, configurable through `jira.projectkey.pattern` | instance | "REST calls that specify an issue key will work with the old issue key"; history in table `project_key` | moved-issue numbers stay reserved even after the project is deleted (JRACLOUD-78272) |
| Plane | `CharField(max_length=12)`, stored `strip().upper()`, characters `&+,:;$^}{*=?@#\|'<>.()%!-` refused | workspace, among projects not soft-deleted ([project.py#L142-L158](https://github.com/makeplane/plane/blob/879ce8b5c6b4fe21efff4789a7062747f46d6383/apps/api/plane/db/models/project.py#L142-L158)) | no table of old keys found (**inferred**: old keys stop resolving) | `IssueSequence` rows survive issue deletion: "we want to keep the sequence even if the issue is deleted" ([issue.py#L557-L571](https://github.com/makeplane/plane/blob/879ce8b5c6b4fe21efff4789a7062747f46d6383/apps/api/plane/db/models/issue.py#L557-L571)) |
| GitLab | (iid, no key) | scope and usage | n/a | `UPDATE internal_ids SET last_value = last_value + 1` returning the value; "strictly monotone … gapless" ([internal_id.rb](https://github.com/gitlab-org/gitlab/blob/1d58eb23e25fffbf37bacc83ca32aa9f99a4c793/app/models/internal_id.rb)) |
| Linear | no official documentation found | team | one community report says a renamed team re-keys its issues and old identifiers still resolve (unverified) | not found |

How each counter stays correct under concurrency:

- **Plane** serialises with `pg_advisory_xact_lock` keyed on the project, then takes `Max("sequence") + 1` from `IssueSequence` ([issue.py#L180-L214](https://github.com/makeplane/plane/blob/879ce8b5c6b4fe21efff4789a7062747f46d6383/apps/api/plane/db/models/issue.py#L180-L214)). That is max+1 under a lock, over rows that deletion never removes.
- **GitLab** keeps a counter row updated in the same transaction. Two operations can lower or reset it: `reset` rewinds the counter for a rolled-back record, and `flush_records!` deletes counter rows that are later re-seeded from `maximum(iid)`. Reading the code, the second can hand out the iids of deleted top-numbered issues again (**inferred**).
- **Jira** keeps the old keys of moved issues in `moved_issue_key`, and its counter-reset KB deletes those rows by hand.

### 4. Key assignment by the local authority

**What the store does today** (read at `c3e2db2`):

- `ops.assigned_key TEXT` exists.
- The `ops_immutable` trigger leaves `assigned_key` out of its column list, so the column stays writable.
- `Op::envelope` has no Key field, so the Key is outside `op_hash` as ADR 0006 requires.
- `insert_op` writes 15 columns and never `assigned_key`. `StoredOp::read` does not select it, and nothing in `apply.rs` writes the `keys` table.
- `apply` refuses `set_field key` as "computed".
- `keys` is in `DROP_PROJECTIONS` with the comment "Keys outlive purged Issues, so they carry no foreign key".
- `ops.project` is free text inside the envelope, and the tests store the prefix itself (`"CPL"`).

**Measured: concurrent assignment** with [key-probe](scripts/key-probe/): 50 processes × 10 creates, each create in its own transaction, no retries. The `keys` table has `key` as primary key and `UNIQUE (project, number)`. The first three strategies ran five times each and the mutation row three times:

| Strategy | Committed | Errors | Duplicates | Numbers |
|---|---|---|---|---|
| `BEGIN IMMEDIATE`, counter row (`INSERT … ON CONFLICT DO UPDATE … RETURNING`) | 500 / 500 every run | 0 | 0 | 1..500, no gaps |
| `BEGIN IMMEDIATE`, `max(number) + 1` over `keys` | 500 / 500 every run | 0 | 0 | 1..500, no gaps |
| `BEGIN` (deferred), `max(number) + 1` | 130 to 188 | 302 to 362 `SQLITE_BUSY`, 7 to 14 `SQLITE_BUSY_SNAPSHOT` | 0 | contiguous over what committed |
| `max(number) + 1` read **before** `BEGIN IMMEDIATE` (a deliberate bug) | 448 to 455 | 45 to 52 UNIQUE violations | 0 stored | contiguous |

With 200 processes × 10, both `IMMEDIATE` strategies committed 2000 / 2000 with no gaps (858 ms with the counter row, 472 ms with max+1, one run each).

Reading the results:

- A deferred transaction fails rather than duplicates. SQLite's docs describe the mechanism: an upgrade "will fail with SQLITE_BUSY" if another connection has written, and when waiting could deadlock, "it will go ahead and return SQLITE_BUSY to the application instead of invoking the busy handler".
- The last row is the mutation check. Computing the number outside the write transaction produces real collisions, and only the constraint stops them from being stored. The probe therefore catches duplicates when they occur; the `IMMEDIATE` strategies produce none.

**Measured: rollback and crash leave no gap.**

- Rolling back every third create (150 of 500) left 350 Keys numbered 1..350 with no gaps.
- In the kill test, a process took `BEGIN IMMEDIATE`, reserved `CPL-1`, printed `reserved 1` and was killed with SIGKILL. The next 10 × 10 writers committed 100 Keys numbered 1..100, with no trace of the killed transaction.
- This matches SQLite's rule for rowids: "ROWID values that were rolled back are ignored and can be reused". PostgreSQL sequences behave the opposite way: "the value obtained by `nextval` is not reclaimed for re-use if the calling transaction later aborts", so they "cannot be used to obtain 'gapless' sequences". This matters for the later Postgres server.
- So under the local authority, gaps come only from purge, and "never reused" is about committed Keys.

**Measured: a rebuild reproduces Keys only from the stored column.**

- After the 500-create run, dropping `keys` and refilling it from `ops.assigned_key` on `create` rows gave a byte-identical table. The next number after the rebuild was 501, the same as the counter row before it.
- Renumbering from another log order (`op_id`) would have changed 490 of the 500 Keys.
- Rebuild sorts by `server_seq IS NULL, server_seq, hlc, replica_id, op_id`. That order equals commit order on one Replica, but not after operations from several Replicas interleave (**inferred** from ADR 0006). Replay must therefore read the Key from the row and never recompute it.

**Measured: what a purge must keep.** Three Issues were created, `CPL-1` to `CPL-3`, and then `CPL-3` was purged two ways:

| Purge | Next number from `max(keys)` | From the counter row | From the log | After a rebuild |
|---|---|---|---|---|
| A: projection row deleted, `create` operation kept | 3 (reuse) | 4 | 4 | 4, and `CPL-3` is back in `keys` |
| B: `create` operation deleted | 3 | (dropped with the projections) | 3 | 3, so `CPL-3` is reused |

"Never reused, even after purge" therefore holds only if purge keeps the `create` row and its `assigned_key`, or if a high-water mark lives in a table that rebuild does not drop. A counter row inside the projections protects nothing across a rebuild.

### 5. Conflicts and open decisions in the docs

- **`ops.project` is the Project key, inside `op_hash`.** ADR 0006 hashes "Project" into every operation and uses it to partition the chain. The code stores the key string there. Operations are never rewritten, so a Project key could then never change. Jira and Linear both allow changing a key and keep old Keys resolving. Whether `ops.project` holds a stable Project id or the key is the hardest-to-reverse choice in this bead, and it must be made before real operations exist.
- **Purge against the append-only log.** `keys` is dropped on rebuild, replay refuses an operation whose hash does not match, and the `create` row is the only durable record of a Key. Purge is not designed (projection-schema.md lists it as open). Whatever design it gets must keep the `create` row's identity and `assigned_key`. This is a constraint on purge, and it blocks the "never reused" invariant rather than being a side question.
- **Fresh clone in Phase 1.** ADR 0003 says "a fresh clone on another machine gets its Issues through `tm serve` sync", but sync is Phase 9. A clone whose `.config/tm.toml` names a Workspace this machine lacks needs defined behaviour now.
- **Where a repo-scoped Workspace's config lives.** ADR 0003 says its "config sits in the repo's `.config/`". design.md says per-machine values "live in the Workspace or user config". The repo file can therefore hold only shareable config. Per-machine settings for a repo-scoped Workspace (sync URL, later tokens) must live under `~/.tm`, so the Workspace's config is split across two places.
- **`~/.tm` is fixed and XDG is not mentioned.** design.md and ADR 0003 give `~/.tm/` as the default. Changing the default would be a design.md edit, outside this research.
- **"global to a machine".** The glossary defines a Workspace as "global to a machine or created separately for isolation", but a synced Workspace exists on several Replicas. "Global" means "the default one on this machine", not "machine-specific".
- **Which Workspace `tm init` makes.** design.md says `tm init` "in a repo with no Workspace creates a Workspace dedicated to it". It does not say what happens when the machine already has a global Workspace that the repo could map into. The bead's acceptance asks for a repo-scoped one.
- **Trust in repo-supplied config.** ADR 0003 lets the repo file carry "Components, Scheme overrides and rules", and design.md lists "Commands run by automation rules" after v1. Once a repo file can cause commands to run, cloning a repo is jj's "zip file problem". jj moved repo config out of the repo for this reason; mise keeps a trust store.
- **Confirmed diff against agents.** AGENTS.md: tm "writes anything outside its Workspace only after a confirmed diff". `tm init` writing `.config/tm.toml` is such a write, so it needs a confirmation an agent can give without a TTY.
- **Nested Projects and keys.** design.md says Projects "nest freely" and monorepos map path prefixes to Projects. Whether a child Project has its own key and counter, or numbers under its parent's key, is not stated.
- **Several remotes.** Mapping by remote URL does not say which remote (origin, upstream, all), or whether a fork and its upstream may map to the same Project.

## Conclusions

1. **Keep one root, `~/.tm`, with `TM_HOME` as the override.** Do not split the store across `XDG_DATA_HOME`, `XDG_STATE_HOME` and so on. A Workspace is "a self-contained store", and one directory is one thing to back up, copy, or allow in a sandbox profile. Cargo is the precedent for a single home with an environment override. Most CLI tools that honour XDG also do so on macOS (jj, gh, mise); the `directories` crate's Application Support mapping would put a space in the path and differ by OS.
   - Not using `~/.config/tm` also avoids that path being mistaken for a repo file in a dotfiles repo at `$HOME`.
   - Honouring XDG on Linux is a reasonable later option, but nothing measured here needs it.
2. **Recommended layout**, with each Workspace directory named by its id:

   ```text
   $TM_HOME (default ~/.tm)
     config.toml              user config: whoami, the default Workspace, per-machine settings
     workspaces/
       <workspace-id>/        global and repo-scoped Workspaces alike
         store.db, store.db-wal, store.db-shm
         snapshot.db          read-only snapshot (ADR 0001)
         config.toml          Workspace config: defaults, remote mappings, per-machine sync settings
         attachments/<hash>
   ```

   - Resolving an id is a path join, and listing `workspaces/` is the registry, so no separate registry file is needed.
   - The store records its own Workspace id in `meta`, beside `replica_id`. The Workspace id is shared by all Replicas; the Replica id is per store.
   - This follows jj's `repos/<config-id>/` split.
3. **The repo file holds identity only.** Nothing in it varies between machines:

   ```toml
   workspace = "01929f6e-7c1a-7b3e-9d2a-4f6c1e0b8a21"
   project = "TM"

   [[paths]]
   prefix = "crates/tm-tui"
   project = "TUI"
   ```

   - Resolving it: if `$TM_HOME/workspaces/<workspace>/` exists, use that store; if not, fail with a structured error.
   - `.tm.toml` and `.config/tm.toml` both present: refuse, naming both.
   - Path prefixes match whole path segments, and the longest match wins, so `crates/tm` does not match `crates/tm-tui`.
4. **Find the repo without git, and use git only for the remote fallback.** Walk up for the repo file and stop at `.git`. When no file is found, run `git remote get-url <remote>` with `GIT_OPTIONAL_LOCKS=0`. Never hand-parse `.git/config`: it missed an included remote and an `insteadOf` rewrite in the probe.
   - Normalise remotes to `host/path`: parse with git's scp rule (gix-url's behaviour), lowercase the host, drop the user, port, scheme, leading `/` and `~`, trailing `/` and `.git`.
   - Compare the path ignoring case only for hosts known to be case-insensitive (**inferred**).
   - `file://` and local-path remotes are per-machine and never map.
   - The remote mappings live in the Workspace's `config.toml` outside the repo; a URL is never written into the repo file.
5. **Project keys:** `^[A-Z][A-Z0-9]{1,9}$`, accepted in any case and stored uppercase. This is Jira Cloud's rule with Jira's default length of 10.
   - It excludes `-`, which separates key from number and is the filter language's negation prefix, and `~`, which marks provisional Keys.
   - Keys are unique within the Workspace, retired keys included, so an old key keeps resolving and is never handed to another Project (Jira's rule).
6. **Record Project identity in the store, keyed by a stable id.** A Project's identity is its id, key, retired keys and parent. Its settings (Scheme overrides, Components, rules) stay in config, as ADR 0007 does for the Scheme.
   - Uniqueness is then a `UNIQUE` constraint checked inside the same `BEGIN IMMEDIATE` as other writes. Two concurrent `tm init`s cannot both claim `TM`; a config file has no such check without its own locking (**inferred**).
   - `ops.project` should carry the Project id, so a key change never touches a hashed operation.
   - Whether Project records are operations (synced, with history, but a new kind beyond ADR 0006's list) or a permanent non-projection table is a decision for an ADR. This note leans toward operations, because sync must carry Projects anyway.
7. **Key assignment:**
   - Within `append_all`'s `BEGIN IMMEDIATE`, a `create` takes `max(number) + 1` for its Project from `keys`, which the `UNIQUE (project, number)` index serves. It then writes `assigned_key` on the `create` row in `insert_op`.
   - `apply(create)` inserts the `keys` row from that column, `StoredOp` reads the column, and replay never recomputes a number.
   - Measured, max+1 is as safe as a counter row and needs no extra state. A counter row is only worth having as a permanent high-water mark, outside the projections, if purge is allowed to remove `create` rows.
   - A trigger should let `assigned_key` change only from `NULL`, so a local Key, once written, is as immutable as the hashed columns.
8. **Provisional Keys (sketch only, inferred; sync is Phase 9):**
   - While a synced Replica is offline, `assigned_key` stays `NULL`. The provisional Key (`CPL-~k3f`) is derived deterministically from the Issue UUID, so a rebuild reproduces it without storing it.
   - The server's Key arrives as a server effect into `assigned_key` and the chain hash (ADR 0006), and the provisional form becomes an alias row.
   - When a local-only Workspace attaches to a server, the server must take over the existing local Keys and continue after the highest. ADR 0004's `tm sync detach` is the reverse.

## Implications / actions

- **tm-u8g.5 code changes:**
  - `insert_op` takes the assigned Key.
  - Add `assigned_key` to `StoredOp` and `replay`.
  - `apply` creates the `keys` row.
  - Add `UNIQUE (project, number)` to `keys`.
  - Add a trigger freezing a non-NULL `assigned_key`.
  - Add a `workspace_id` meta row.
  - `IssueView` gains `key`.
  - Extend the rebuild property test to compare `keys`.
- **Tests for the acceptance criteria:**
  - Extend the existing 50-process test so children `create_issue`. Assert `count(*) = count(DISTINCT key) = 500` and numbers 1..500.
  - Add a single-process test that rolls back a create and checks that the next create reuses the number.
  - For `tm init`: snapshot the fixture repo's tree, including `.git/`, before and after. Assert the only difference is a new `.config/tm.toml`, that `git status --porcelain` prints exactly `?? .config/tm.toml`, and that `.git/config`, `.git/hooks/`, `.gitignore`, `AGENTS.md` and `CLAUDE.md` are byte-identical.
  - Point `TM_HOME` at a temporary directory, so the test never touches `~/.tm`.
- **`tm init` must:**
  - Create the Workspace before writing the repo file, so a failed write leaves only an unreferenced Workspace that `tm doctor` can list.
  - Write the file with create-new semantics, so a concurrent `tm init` fails and re-reads rather than overwriting.
  - Show the file as a diff and wait for confirmation. Agents need an explicit, non-interactive confirmation.
  - Never stage or commit, never touch `.gitignore` or `.git/info/exclude`, never set git config or hooks, and never edit AGENTS.md or CLAUDE.md.
- **ADR candidates:** what `ops.project` holds (Project id or key) and where Project identity lives. Both affect `op_hash` and should be settled before the first real operation is written.
- **Constraint for the purge design:** keep each purged Issue's `create` row (or at least its id, Project and `assigned_key`), so that "never reused" survives a rebuild.
- **Phase 9:** take over local Keys when a Workspace first attaches; derive provisional Keys from the Issue UUID; the Postgres server must not use sequences for Keys if Keys are to stay gapless.

## Open questions

- **Fresh clone with an unknown Workspace in Phase 1.** One option is to refuse with a structured error offering `tm import` from an export. Another is to create an empty local Workspace under the same id, but that lets two machines number the same Project, which is the collision ADR 0004 exists to prevent. This note recommends refusing until sync exists; the choice is the user's.
- **Which remote maps,** and whether forks share a Project.
- **Does a child Project number its own Issues?**
- **Is a Project key rename supported in v1?** It is cheap only if `ops.project` is an id.
- **Trust for repo-supplied rules** before any rule can run a command: a mise-style trust store, or jj's approach of keeping such config out of the repo.
- **Honour `XDG_*` on Linux later?** Nothing here needed it; revisit if users ask.
- **Linux runs.** Both probes ran only on macOS. The SQLite behaviour is engine-level, and ADR 0001's sandbox probes matched across both OSes, but these exact runs are not measured on Linux.
- **Linear's key rules and rename behaviour** were not found in official docs.

## Raw data

key-probe (`key-probe concurrent <strategy> 50 10 0 <dir>`, five rounds, first round shown, then the varying fields of the rest):

```text
strategy=counter-immediate procs=50 each=10 rollback_every=0 elapsed_ms=186 committed=500 rolled_back=0 busy=0 busy_snapshot=0 unique_violation=0 other=0 | keys_rows=500 distinct_numbers=500 min=Some(1) max=Some(500) gaps=0
strategy=max-immediate procs=50 each=10 rollback_every=0 elapsed_ms=154 committed=500 rolled_back=0 busy=0 busy_snapshot=0 unique_violation=0 other=0 | keys_rows=500 distinct_numbers=500 min=Some(1) max=Some(500) gaps=0
strategy=max-deferred procs=50 each=10 rollback_every=0 elapsed_ms=32 committed=153 rolled_back=0 busy=340 busy_snapshot=7 unique_violation=0 other=0 | keys_rows=153 distinct_numbers=153 min=Some(1) max=Some(153) gaps=0
counter-immediate elapsed_ms: 190 169 138 147 267, always committed=500 gaps=0
max-immediate     elapsed_ms: 154 181 136 174 151, always committed=500 gaps=0
max-deferred      committed/busy/busy_snapshot: 167/323/10 188/303/9 184/302/14 188/302/10 130/362/8
strategy=max-outside procs=50 each=10 rollback_every=0 elapsed_ms=173 committed=453 rolled_back=0 busy=0 busy_snapshot=0 unique_violation=47 other=0 | keys_rows=453 distinct_numbers=453 min=Some(1) max=Some(453) gaps=0
strategy=max-outside procs=50 each=10 rollback_every=0 elapsed_ms=152 committed=455 rolled_back=0 busy=0 busy_snapshot=0 unique_violation=45 other=0 | keys_rows=455 distinct_numbers=455 min=Some(1) max=Some(455) gaps=0
strategy=max-outside procs=50 each=10 rollback_every=0 elapsed_ms=141 committed=448 rolled_back=0 busy=0 busy_snapshot=0 unique_violation=52 other=0 | keys_rows=448 distinct_numbers=448 min=Some(1) max=Some(448) gaps=0
strategy=counter-immediate procs=200 each=10 rollback_every=0 elapsed_ms=858 committed=2000 rolled_back=0 busy=0 busy_snapshot=0 unique_violation=0 other=0 | keys_rows=2000 distinct_numbers=2000 min=Some(1) max=Some(2000) gaps=0
strategy=max-immediate procs=200 each=10 rollback_every=0 elapsed_ms=472 committed=2000 rolled_back=0 busy=0 busy_snapshot=0 unique_violation=0 other=0 | keys_rows=2000 distinct_numbers=2000 min=Some(1) max=Some(2000) gaps=0
strategy=counter-immediate procs=50 each=10 rollback_every=3 elapsed_ms=188 committed=350 rolled_back=150 busy=0 busy_snapshot=0 unique_violation=0 other=0 | keys_rows=350 distinct_numbers=350 min=Some(1) max=Some(350) gaps=0
holder said `reserved 1`, exited signal: 9 (SIGKILL); then 10x10 writers: committed=100 busy=0 | keys_rows=100 distinct=100 min=Some(1) max=Some(100) killed_rows_in_log=0 number_1_now_held_by_killed=false
created CPL-1..CPL-3; counter row next_number=4
purge A (projection row deleted, create op kept): next from max(keys)=3 next from counter row=4 next from log=4
  after rebuild from log: next from max(keys)=4 (CPL-3 present again: true)
purge B (create op deleted, then rebuild): next from max(keys)=3 next from log=3 -> CPL-3 would be reused
rebuild: rows before=500 after=500 identical=true counter_row_before=Some(501) next_after_rebuild=501 | renumbering by another order (op_id) would change 490 Keys
```

git-probe (`bash git-probe.sh <empty dir>`):

```text
== remote URLs
hand-parsed .git/config, origin:   gh:oakoss/tm.git
hand-parsed .git/config, upstream:
git config --get remote.origin.url:   gh:oakoss/tm.git
git remote get-url origin:            git@github.com:oakoss/tm.git
git config --get remote.upstream.url: https://github.com/Oakoss/TM
git remote get-url upstream:          https://github.com/Oakoss/TM
== root discovery from a subdirectory
rev-parse --show-toplevel: $SCRATCH/git/repo
rev-parse --show-prefix:   pkg/sub/
== linked worktree
.git in the worktree is a: file
its contents: gitdir: $SCRATCH/git/repo/.git/worktrees/wt
rev-parse --show-toplevel:  $SCRATCH/git/wt
rev-parse --git-dir:        $SCRATCH/git/repo/.git/worktrees/wt
rev-parse --git-common-dir: $SCRATCH/git/repo/.git
remote get-url origin:      git@github.com:oakoss/tm.git
== writes under .git by read commands
files changed by rev-parse/config/remote get-url/log/for-each-ref: 0
files changed by GIT_OPTIONAL_LOCKS=0 git status: 0
files changed by plain git status: > 1792265027.872815232 .git/index
== cost of shelling out (20 runs each)
rev-parse --show-toplevel: 7 ms per run; remote get-url: 7 ms per run
```
