# Filter language: first-cut semantics, parser and SQL shape

- Date: 2026-10-17
- Author: Claude (research pass for Jace Babin)
- Scope: What should the first cut of the filter language (tm-u8g.7) mean, how should it be parsed and reported, and what SQL should it compile to over the projection layout that projection-schema.md recommends?

Method note: sources were read on 2026-10-17. The GitHub, Linear and Plane docs pages and the GitHub engineering blog came through a summarising fetch, so their quotes are near-verbatim. The Atlassian quotes and all source-code quotes are exact. SQL timings ran through `rusqlite 0.40.2` with its bundled SQLite (`sqlite_version() = 3.53.2`, the build `tm` ships), on an Apple M2 Max (`sysctl -n machdep.cpu.brand_string`), as 7 timed runs after one warm-up; the tables report medians. One probe (`nocase-and-null.sql`) ran on the macOS `sqlite3` (`3.54.0`) instead and says so. Build times used `cargo 1.99.0` with the registry already cached, so they measure compilation, not download. Anything not quoted or measured is labeled **inferred**. Recommendations here are recommendations, not decisions.

Scripts:

- [scripts/filter-sql/](scripts/filter-sql/): `setup.sql` (schema and data), `queries.sql` (every probe query with the filter it stands for), `runner/` (the Rust program that builds 10k and 100k stores and prints plans and timings), `nocase-and-null.sql`. Run: `cargo run --release --manifest-path runner/Cargo.toml -- setup.sql queries.sql <scratch-dir> 10000 100000`.
- [scripts/parser-probes/](scripts/parser-probes/): the same first-cut grammar in four parsers (`hand`, `winnow`, `chumsky`, `logos`) sharing an AST and twelve inputs (`ast`), plus `suggest` for "did you mean" rules. Run: `cargo run -p probe-<name>` from that directory.

Both are standalone Cargo workspaces with their lockfiles; copy them outside the repo to build, as the measurements were.

## Question

1. How do GitHub issue search, Linear, Jira JQL and Plane treat repeated qualifiers, negated lists, empty values, quoting, case, multi-value fields, `@me`, sort and bare words?
2. How do they report unknown fields and values; what suggestion thresholds are established; what should a structured error carry for agents?
3. Which Rust parsing approach fits a small qualifier grammar that will later gain comparisons, `OR` and parentheses?
4. What SQL should a filter compile to over built-in columns, `field_values` (EAV) and `set_members`, and how does it perform at 10k and 100k Issues?
5. What in design.md or the build plan conflicts with the findings?

## Sources

Vendor semantics:

- GitHub: [Searching issues and pull requests](https://docs.github.com/en/search-github/searching-on-github/searching-issues-and-pull-requests) (GH-S), [Filtering and searching issues](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/filtering-and-searching-issues-and-pull-requests) (GH-F), [Understanding the search syntax](https://docs.github.com/en/search-github/getting-started-with-searching-on-github/understanding-the-search-syntax) (GH-SYN), [Sorting search results](https://docs.github.com/en/search-github/getting-started-with-searching-on-github/sorting-search-results) (GH-SORT), [REST search](https://docs.github.com/en/rest/search/search) (GH-REST), [Troubleshooting search queries](https://docs.github.com/en/search-github/getting-started-with-searching-on-github/troubleshooting-search-queries), [engineering blog on nested queries](https://github.blog/developer-skills/application-development/github-issues-search-now-supports-nested-queries-and-boolean-operators-heres-how-we-rebuilt-it/) (GH-BLOG), [2025-03-06 changelog](https://github.blog/changelog/2025-03-06-github-issues-projects-api-support-for-issues-advanced-search-and-more/) (GH-CL)
- Linear: [API filtering](https://linear.app/developers/filtering) (LIN-API), [UI filters](https://linear.app/docs/filters) (LIN-UI), [GraphQL errors](https://linear.app/developers/graphql), SDK schema @ `9dd2ed80f93ae596f4f7fa21aa7cf80895c58514`: [schema.graphql](https://github.com/linear/linear/blob/9dd2ed80f93ae596f4f7fa21aa7cf80895c58514/packages/sdk/src/schema.graphql) (LIN-SCHEMA)
- Jira: [JQL operators](https://support.atlassian.com/jira-software-cloud/docs/jql-operators/) (JQL-OP), [keywords](https://support.atlassian.com/jira-software-cloud/docs/jql-keywords/) (JQL-KW), [fields](https://support.atlassian.com/jira-software-cloud/docs/jql-fields/) (JQL-FLD), [functions](https://support.atlassian.com/jira-software-cloud/docs/jql-functions/) (JQL-FN), [advanced search](https://support.atlassian.com/jira-software-cloud/docs/what-is-advanced-search-in-jira-cloud/) (JQL-ADV), KB pages on [unknown custom fields](https://support.atlassian.com/jira/kb/unable-to-search-using-a-custom-field-when-executing-a-jql/), [unknown values](https://support.atlassian.com/jira/kb/jql-search-returns-the-value-does-not-exist-for-the-field-fixversion/) and [values inside OR](https://support.atlassian.com/atlassian-knowledge-base/kb/jql-with-or-results-in-error/); [JRACLOUD-24907](https://jira.atlassian.com/browse/JRACLOUD-24907)
- Plane: [Plane Query Language](https://docs.plane.so/core-concepts/issues/plane-query-language) (PL-PQL), [filters](https://docs.plane.so/core-concepts/issues/visualise_filter) (PL-UI), [display options](https://docs.plane.so/core-concepts/issues/display-options); source @ `879ce8b5c6b4fe21efff4789a7062747f46d6383`: [issue_filters.py](https://github.com/makeplane/plane/blob/879ce8b5c6b4fe21efff4789a7062747f46d6383/apps/api/plane/utils/issue_filters.py) (PL-IF), [filter_backend.py](https://github.com/makeplane/plane/blob/879ce8b5c6b4fe21efff4789a7062747f46d6383/apps/api/plane/utils/filters/filter_backend.py) (PL-FB)

Suggestions:

- clap @ `ccc89b83875474399e9d9d70841163f6319d1a41`: [clap_builder/src/parser/features/suggestions.rs](https://github.com/clap-rs/clap/blob/ccc89b83875474399e9d9d70841163f6319d1a41/clap_builder/src/parser/features/suggestions.rs)
- rust @ `2834d57549e77dbd7e9fa42f51aa349f8a00368b`: [compiler/rustc_span/src/edit_distance.rs](https://github.com/rust-lang/rust/blob/2834d57549e77dbd7e9fa42f51aa349f8a00368b/compiler/rustc_span/src/edit_distance.rs)

Crates (versions and licenses from `cargo info`): winnow 1.0.4 (MIT), chumsky 0.12.0 (MIT; 1.0.0-alpha.8 is the newest release), logos 0.16.1 (MIT OR Apache-2.0), strsim 0.11.1 (MIT), toml 1.1.8 (MIT OR Apache-2.0). Repo inputs: [design.md](../../specs/design.md), [build-plan.md](../../contributing/build-plan.md), [projection-schema.md](../store/projection-schema.md), `crates/tm-store/src/schema.rs`, `deny.toml`.

## Findings

### 1. Semantics across trackers

| Question | GitHub | Linear | Jira JQL | Plane |
|---|---|---|---|---|
| Repeated qualifier | AND: `label:bug label:resolved` "matches issues with the labels 'bug' and 'resolved'" (GH-S). Not uniform historically: without advanced search "a space is treated as an `OR` operator" for `repo`, `org`, `user` (GH-CL) | All fields "need to be matched" (LIN-API) | Written as explicit `AND` | AND (separate filter keys) |
| List | OR: `label:bug,resolved` "matches … 'bug' or … 'resolved'" (GH-S) | `in`: "Value is in the given collection" | `IN (a, b)` "is the same as … `= a OR = b`" (JQL-OP) | "is any of" (PL-UI); comma list becomes `labels__in` (PL-IF L147-159) |
| Negated list | **Not documented.** Only single-value `-label:bug` (GH-S); the hyphen excludes "all results that are matched by a qualifier" (GH-SYN), so none-of is **inferred** | `nin`; null handling not documented | `NOT IN` is `!= a AND != b` and "will not match a field that has no value" (JQL-OP) | `NOT IN` "Matches none of the values" (PL-PQL) |
| Negation and empty fields | Not documented | Not documented | Excludes empty: "`component != fred` will only match work items that have a component" (JQL-OP); workaround `labels not in ("x") or labels is EMPTY` (JQL-FLD) | Not documented |
| Empty or missing | `no:label`, `no:assignee`; "cannot be combined with the minus (hyphen) symbol" (GH-S); `assignee:*` within one repository; `-label:*` not documented | `null: true/false` (LIN-API) | `IS EMPTY`, `IS NOT EMPTY` (JQL-OP) | "is empty" (PL-UI); `IS NULL`, `hasNoLabel()` (PL-PQL) |
| Quoting | `label:"bug fix"`; quoted items in a list, `label:"bug","wip"` (GH-SYN, GH-F); escapes not documented | JSON, none needed | Single or double quotes; some characters need `\\` (JQL-ADV) | Not documented |
| Case | "Search is not case sensitive" (GH-S) | `eq` vs `eqIgnoreCase` (LIN-API), so `eq` is case-sensitive (**inferred**) | Not documented; labels may differ only by case (JRACLOUD-24907) | Not documented |
| Multi-value match | `label:a` matches an Issue with a and b (**inferred** from the AND example) | Default `some`: "at least one label that matches"; `every` for all (LIN-API, LIN-SCHEMA) | `labels = x` finds Issues having x (JQL-FLD) | `labels__in` |
| `@me` | `author:@me` (GH-S); resolution for Apps or tokens not documented | `isMe`: "the currently authenticated user" (LIN-SCHEMA L30875) | `currentUser()`, only `=` and `!=` (JQL-FN) | `currentUser()`: "The person running the query" (PL-PQL) |
| Sort | `sort:created` or `sort:created-desc` newest first, `-asc` oldest (GH-SORT); REST default best match, `order` default `desc` (GH-REST) | `PrioritySort` with `noPriorityFirst` (default false) and `nulls: last` (LIN-SCHEMA L33805) | `ORDER BY` uses "the field's own sorting order" unless `asc`/`desc` (JQL-KW); comparisons on versions use admin-set order, "not a numeric or alphabetic order" (JQL-FLD) | `ASC` default (PL-PQL) |
| Bare words and precedence | Space is AND (GH-F); AND binds tighter than OR (grammar in GH-BLOG: root `or_operation` built from `and_operation`); nesting "up to five levels deep" (GH-F); at most five operators, 256 characters (GH-REST) | Explicit JSON tree | "evaluated left-to-right" without parentheses (JQL-ADV) | Nesting capped at `default_max_depth = 5` (PL-FB L29) |

Two notes the table compresses:

- Plane's legacy `issue_filters()` dispatches only keys in its `ISSUE_FILTER` map, so unknown keys are skipped without an error, and `filter_valid_uuids` drops invalid IDs silently (read from PL-IF, not run). This is the failure mode to avoid.
- Whether JQL `ORDER BY priority` sorts by rank or alphabetically is not stated in Atlassian docs; community threads report alphabetical. Unresolved.

### 2. Error UX and suggestions

**What each tracker reports.**

| | Unknown field | Unknown value | Position |
|---|---|---|---|
| GitHub | Not documented; GH-F says only that it will "warn when there is a problem with your filter" | Not documented | Not documented |
| Jira | "Field 'xyz' does not exist or you do not have permission to view it." | "The value 'Version 1.2.3' does not exist for the field 'fixVersion'." Errors even inside `OR`, since searching for a missing value "is equivalent to looking for NULL" (KB) | Yes: "the line and character number of the invalid text" (JQL-ADV) |
| Linear | GraphQL `errors` array with `path` and `extensions` "such as error codes or validation details" | Not documented | `path` only |
| Plane (new backend) | `"Filtering on field '{field}' is not allowed"`, code `invalid_filter_field` (PL-FB L120-127) | `invalid_filterset` with per-field `errors` | No |

None lists the valid options in the error. Jira gives the position; Plane gives a machine-readable code.

**Suggestion thresholds in source.**

- clap: `let confidence = strsim::jaro(v, pv.as_ref()); if confidence > 0.7 { … }`, candidates sorted by confidence, with the comment "using `jaro` because `jaro_winkler` implementation in `strsim-rs` is wrong" (suggestions.rs L17-21).
- rustc: first "Exact case insensitive match", then edit distance with `let mut dist = dist.unwrap_or_else(|| cmp::max(lookup_len, 3) / 3);` (edit_distance.rs L213-224), then sorted-word match.

Measured with `probe-suggest` against 18 field names and 7 statuses (output verbatim in Raw data):

| Input | clap rule (Jaro > 0.7) | rustc rule (≤ max(len,3)/3 edits) |
|---|---|---|
| `statu` | status 0.94, estimate 0.77 | status (d=1) |
| `prio` | priority 0.83, project 0.73 | none |
| `tpye` | type 0.92 | none |
| `test` | testing 0.86 | none |
| `Status` | status 0.89 | status (d=0, case-insensitive match) |
| `in-progress` | in_progress 0.94 | in_progress (d=1) |
| `xyz` | none | none |

Jaro catches prefixes and transpositions that the edit-distance limit misses on short words; it also offers a second, weaker candidate (`estimate` for `statu`). rustc's case-insensitive exact match first is what turns `Status` and `Done` into a silent canonicalisation rather than an error.

### 3. Parser approaches, measured

All four probes implement the same first-cut grammar (terms separated by whitespace; `-` negation; `field:value,value`; quoted values with `\` escapes; `@name` functions; `sort:field[-asc|-desc]`) and run the same twelve inputs. Winnow and chumsky were written with their default error types (`ContextError`, `Rich`); both can produce better messages with custom error types, but that custom code is roughly what the hand-written parser already is.

**Error output on the failing inputs** (spans are byte ranges; full output in Raw data):

| Input | hand-written | winnow 1.0.4 | chumsky 0.12.0 | logos 0.16.1 + hand parser |
|---|---|---|---|---|
| `status:` | `expected_value` 7..8 (one past the end) | 7..7 "expected a value" | 7..7 "found end of input expected value" | `expected_value` 7..7 |
| `label:"open` | `unterminated_quote` 6..11 (whole value) | 11..11 "invalid quoted value / expected closing `\"`, a value" | 11..11 "expected something else, '\\', or '\"'" | `lex` 6..11 "unrecognised input" |
| `status:review,` | `expected_value` 14..15 | 13..14, empty message | 14..14 "expected value" | `expected_value` 14..14 |
| `:foo` | `expected_field` 0..1 | 0..1, empty message | 0..1 "expected '-', something else, or end of input" | `unexpected` 0..1 |
| `fix login` | `bare_word` 0..3 "full-text search is not supported yet" | 0..1, empty message | 3..4 "found ' ' expected something else, or ':'" | `bare_word` 0..3 |
| `status:review OR …` | `unsupported` 14..16 "`OR` is not supported yet" | 14..15, empty message | 16..17 | `unsupported` 14..16 |
| `(status:review)` | `unsupported` 0..1 "parentheses …" | 0..1, empty message | 0..1 "found '('" | `unexpected` 0..1 |
| `priority:>=high` | `unsupported` 9..10 "comparisons …" | 9..10 "expected a value" | 9..10 "found '>' expected value" | **accepted** as the text `>=high` |
| `iteration:@next(cli)` | `unsupported` 10..16 "function arguments …" | 15..16, empty message | 15..16 | `unexpected` 15..16 |

The hand-written parser was the only one that named the construct in its error without extra work; the logos lexer silently accepted `>=high` as a value because its word token admits `>`, which a later comparison operator would then reinterpret.

**Cost** (`cargo tree -e normal,build`, clean `cargo build`, release binary with the probe's default profile):

"Crates in graph" counts what the probe pulls in a standalone workspace; "new in tm's lock" counts `Cargo.lock` entries added when the crate joins `tm-core` (from the deny run below).

| Approach | Crates in graph | New in tm's lock | Clean dev build | Clean release build | Release binary |
|---|---|---|---|---|---|
| hand-written | 0 | 0 | 0.41 s | 0.53 s | 472,896 B |
| winnow 1.0.4 | 1 (`winnow`) | 0 (already locked) | 0.88 s | 1.15 s | 481,728 B |
| logos 0.16.1 | 12 (`logos`, `logos-codegen`, `logos-derive`, `regex-automata`, `regex-syntax`, `aho-corasick`, `fnv`, `memchr`, `syn` 2, `proc-macro2`, `quote`, `unicode-ident`) | 5 | 3.20 s | 3.35 s | 474,128 B |
| chumsky 0.12.0 | 17 (`stacker`, `psm`, `cc`, `ar_archive_writer`, `object`, `hashbrown` 0.15, `foldhash` 0.1, `unicode-segmentation`, `libc`, …) | 12 | 4.62 s | 5.07 s | 518,208 B |

**Repo policy.** Each crate was added to `tm-core` in a scratch copy of the repo and checked with the repo's `deny.toml` (`cargo-deny 0.20.2 check licenses bans`), against today's lockfile:

| Added | Result |
|---|---|
| `winnow = "=1.0.4"` | `bans ok, licenses ok` |
| `chumsky = "=0.12.0"` | licenses rejected: `ar_archive_writer v0.5.3` (`Apache-2.0 WITH LLVM-exception`) via `psm` ← `stacker` ← `chumsky`; bans: duplicate `hashbrown` (0.15.5 vs rusqlite's 0.17.1) and `foldhash` (0.1.5 vs 0.2.0) |
| `chumsky`, `default-features = false, features = ["std"]` | licenses ok; bans still fail on `hashbrown` and `foldhash` |
| `logos = "=0.16.1"` | bans: duplicate `syn` (2.0.119 via `logos-codegen` vs 3.0.9 via `serde_derive`, `thiserror-impl`) |
| `toml = "=1.1.8"` | `bans ok, licenses ok`; `cargo tree -i winnow` shows `toml` and `toml_parser` already pull `winnow v1.0.4` |
| `strsim = "=0.11.1"` | `bans ok, licenses ok`, one crate |

`winnow` appears in today's `Cargo.lock` but not in the build graph (`cargo tree -i winnow --target all` prints nothing), so it is not free yet; it becomes free when `toml` arrives for the config layer. winnow 1.0 also ships `combinator::expression` (a Pratt parser) for later operator precedence. chumsky 1.0.0-alpha.8 was not tested.

**Grammar reservations a first cut must make.** Characters that the later language needs cannot be legal in first-cut bare values, or adding the feature changes what an accepted query means:

- `-` negates only at the start of a term; inside a value it is text (`in-progress`, `@today-7d`).
- `(` and `)` serve both grouping and function arguments; `,` serves lists and arguments.
- `<`, `>`, `=` at the start of a value are comparison operators; `..` is worth reserving for ranges.
- `OR`, `AND`, `NOT` as standalone upper-case words are operators.
- `:` inside a value needs quotes.
- Bare words are full-text, which arrives in Phase 7.

The hand-written probe rejects each of these with a structured `unsupported` or `bare_word` error, so no input accepted today changes meaning later.

### 4. SQL over the projection layout, measured

`setup.sql` builds the layout projection-schema.md recommends: `issues` with built-in columns (`status`, `type`, `priority` as an integer, nullable `assignee`, `archived`) and an index on `status`; `field_values (issue_id, field, elem, txt, num, ref_uuid)` `WITHOUT ROWID` with indexes `(field, txt, issue_id)` and `(field, num, issue_id)`; `set_members (issue_id, field, elem)` `WITHOUT ROWID` with `(field, elem)`, as in today's `schema.rs`. Keys are 16-byte BLOBs. Data: a Scheme select `area` on 80% of Issues, a date `due` on 30%, 20 labels each on about 10% (bug and ui co-occur on about 1%), 5% archived. Every query has `archived = 0` and bound parameters. 10k Issues gives 10,993 `field_values` and 19,833 `set_members` rows; 100k gives 109,991 and 199,819.

**(a) Multi-qualifier filter** `area:cli label:bug -type:spike status:review,testing`:

| Form | 10k ms | 100k ms | 100k plan |
|---|---|---|---|
| `EXISTS` | 0.58 | 11.8 | `SEARCH i USING INDEX issues_status`, then each `EXISTS` through the covering indexes |
| `JOIN` | 0.59 | 10.7 | same order |
| `IN (subquery)` | 0.74 | 11.4 | lists built from both indexes, `CREATE BLOOM FILTER`, then `SEARCH i … (id=?)` |

When a built-in column narrows first, the three forms are equivalent. Without it (`area:cli label:bug` only), `EXISTS` scans every Issue:

| Form | 10k ms | 100k ms | 100k plan |
|---|---|---|---|
| `EXISTS` | 4.27 | 60.0 | `SCAN i` + two correlated probes per row |
| `JOIN` | 0.41 | 6.0 | `SEARCH s … (field=? AND elem=?)`, then `f`, then `i` |
| `IN` | 0.73 | 10.9 | two lists from the indexes, Bloom filter |

**(b) Set membership.** Any-of `label:bug,ui` and all-of `label:bug label:ui`:

| Form | Rows (100k) | 10k ms | 100k ms |
|---|---|---|---|
| any-of `EXISTS … elem IN (?, ?)` | 18,057 | 7.97 | 104.9 |
| any-of `JOIN` + `DISTINCT` | 18,057 | 1.18 | 29.7 |
| any-of `JOIN` without `DISTINCT` | **19,015** (wrong) | 0.84 | 25.6 |
| any-of `IN (subquery)` | 18,057 | 1.18 | 28.7 |
| all-of two `EXISTS` | 958 | 4.34 | 60.1 |
| all-of two `JOIN`s | 958 | 0.78 | 17.5 |
| all-of two `IN`s | 958 | 0.60 | 9.2 |
| all-of `IN (… GROUP BY issue_id HAVING count(*) = 2)` | 958 | 0.43 | 6.1 |

A join on a multi-value field returns an Issue once per matching element: 958 Issues carry both labels, which is exactly the 19,015 − 18,057 surplus.

**(c) Negation.**

| Filter and form | Rows (100k) | 10k ms | 100k ms | 100k plan |
|---|---|---|---|---|
| `-label:bug` `NOT EXISTS` | 85,492 | 4.23 | 119.2 | `SCAN i`, correlated `SEARCH s USING PRIMARY KEY` |
| `-label:bug` `LEFT JOIN … IS NULL` | 85,492 | 4.49 | 57.3 | `SCAN i`, covering index probe |
| `-label:bug` `NOT IN (subquery)` | 85,492 | 2.51 | 35.6 | `SCAN i`, list from the index, Bloom filter |
| `-label:bug,ui` `NOT EXISTS … IN (?, ?)` | 76,948 | 8.45 | 111.5 | `SCAN i`, correlated covering probe |
| `-area:cli` `NOT EXISTS` (no area matches) | 82,334 | 4.18 | 57.7 | `SCAN i`, covering `field_values_txt` probe |
| `-area:cli` `JOIN … txt <> ?` (no area does not match) | 63,338 | 3.51 | 106.2 | `SCAN i`, primary-key probe |
| `no:area` `NOT EXISTS` on the field | 18,996 | 3.48 | 101.4 | `SCAN i`, primary-key probe |

The planner probed `set_members` by primary key for `NOT EXISTS` (119 ms) but `field_values` by the `(field, txt, issue_id)` index (58 ms). The likely reason is that the index range for one value is small and contiguous while primary-key probes jump across the table (**inferred**, not isolated). `NOT IN` avoids that choice. It is correct only because `issue_id` is `NOT NULL` in both tables: one NULL in the subquery would make every `NOT IN` false.

For a nullable built-in column, `<>` drops empty rows. Measured on the macOS `sqlite3` (`nocase-and-null.sql`): `SELECT NULL <> 'x', NULL IS NOT 'x', 'jace' IS NOT 'x'` printed `|1|1`, so `assignee <> ?` excludes unassigned Issues and `assignee IS NOT ?` keeps them.

**(d) Sort.**

| Filter and form | 10k ms | 100k ms | Plan |
|---|---|---|---|
| `status:todo sort:due`, `LEFT JOIN`, `LIMIT 50` | 0.90 | 20.8 | `issues_status`, PK probe, `USE TEMP B-TREE FOR ORDER BY` |
| same, every row (18,999) | 1.12 | 22.9 | same |
| same, sort key as correlated scalar subquery | 1.53 | 28.6 | two subqueries per row, temp B-tree |
| `sort:due` alone, `LEFT JOIN`, `LIMIT 50` | 3.67 | 103.9 | `SCAN i`, temp B-tree |
| `sort:due` alone, walking `field_values_num` | 0.03 | 0.045 | `SEARCH f USING COVERING INDEX field_values_num (field=?)`, no sort step; omits Issues without `due` |
| `sort:due` alone, index walk `UNION ALL` the Issues without `due` | 0.04 | 0.052 | compound of two co-routines; the timing shows the second arm (a full scan) did not run, since the first filled the page; a page it cannot fill would pay for the scan (**inferred**) |
| `status:todo sort:area`, option order as `CASE txt WHEN ?…` | 1.01 | 21.8 | temp B-tree |
| same, option order as `json_each(?)` | 1.63 | 28.8 | adds `SCAN o VIRTUAL TABLE` |
| `status:todo sort:priority`, `ORDER BY priority = 0, priority` | 0.39 | 5.6 | temp B-tree |

Sorting by a select needs the Scheme's option order, and a `CASE` built from it costs no more than sorting by a number. Priority needs an integer rank: sorting the five labels as text gives `high < low < medium < none < urgent` (`nocase-and-null.sql`). With Linear's numbering (0 none, 1 urgent … 4 low) "none last" is one more sort term.

**(e) Case-insensitive matching.** `f.txt = ? COLLATE NOCASE` against the plain index used only `(field=?)` and scanned every `area` row (20.7 ms at 100k) where the canonical value used `(field=? AND txt=?)` (17.8 ms); the gap grows with the number of rows a field has. For labels, `CREATE INDEX … ON set_members (field, elem COLLATE NOCASE)` made `elem = 'BUG' COLLATE NOCASE` use `(field=? AND elem=?)` and count 10,009 rows, while plain `elem = 'BUG'` counted 0 (macOS `sqlite3`).

**(f) OR, for later.** `area:cli OR label:bug`:

| Form | 10k ms | 100k ms | Plan |
|---|---|---|---|
| `id IN (…) OR id IN (…)` | 1.36 | 35.3 | `MULTI-INDEX OR` over both lists |
| `EXISTS (…) OR EXISTS (…)` | 7.60 | 163.0 | `SCAN i` |
| `id IN (… UNION …)` | 1.33 | 32.2 | `MERGE (UNION)` of both indexes |

### 5. Conflicts with design.md and earlier research

- **projection-schema.md finding 6** says the emitter "must generate joins, not `EXISTS`". The measurements refine this: joins win only for positive AND terms, return duplicates for lists (19,015 rows for 18,057 Issues), and cannot express negation or `OR`. `IN` subqueries are the one shape that composes.
- **design.md: "Unknown fields or values fail with the valid options."** Only enumerated fields have options; free text, numbers and dates can fail only with a type error.
- **design.md's examples `priority:>=high` and `iteration:@current`** need an order (priority rank, the Iteration list) that the first cut must already model, since `sort:priority` needs the same rank. Today `schema.rs` has no built-in columns at all (every value is in `issues.fields` JSON), and projection-schema.md probe (a) showed JSON paths are indexed only when spelled exactly as the expression index, so .7 depends on .6's columns.
- **design.md lists "bare words for full-text search"** as part of the one language; the build plan puts full-text in Phase 7. The first cut must reject bare words rather than ignore them.
- **design.md has no syntax for empty values** (`no:`, `is:empty`) and does not say whether negation includes them; both are first-cut questions (Conclusions, Open questions).
- **`@me` and `@agent`** are listed as separate functions, but design.md does not say which identity `@me` is when an Agent runs the filter.

## Conclusions

### Semantics (recommendations)

1. **Repeated qualifier is AND; a comma list is OR** (GitHub, JQL `IN`, Plane).
2. **`-field:a,b` means none of a, b.** No vendor documents it; it follows from the comma being OR (**inferred**).
3. **Negation means complement: Issues with no value match.** `-area:cli` and `-label:bug` include Issues without an area or labels, and `-assignee:jace` includes unassigned ones, using `IS NOT` for nullable built-in columns. Jira does the opposite and documents a workaround users must remember; complement is what `NOT IN`/`NOT EXISTS` give for EAV and sets without extra terms.
4. **Multi-value fields match if any element matches**; all-of is the repeated qualifier.
5. **Case-insensitive for field names and enumerated values, resolved before SQL.** Status, type, priority, select options and field names are matched case-insensitively against the Scheme and replaced by their canonical spelling, so SQL compares exactly and keeps the full index (e). GitHub's "not case sensitive" is the precedent. Labels are free-form, so either labels are canonicalised the same way against the distinct existing labels at resolve time, or `set_members` gets a `COLLATE NOCASE` index; the first keeps one index and also gives "did you mean" for labels, so it is the recommendation.
6. **`@me` resolves to the human** (the Assignee identity from `tm whoami`), and `@agent` to the calling Agent. Vendors document only "the authenticated user"; none says what happens when a bot acts for a person, so this needs a decision (see Open questions).
7. **`sort:field` with `-asc`/`-desc` suffixes**, GitHub's form; several `sort:` terms sort by each in turn. Selects sort by option order, priority by rank with none last, as Linear does by default. Empty values sort last in both directions.
8. **Bare words, `OR`/`AND`/`NOT`, parentheses, comparison prefixes and function arguments are rejected with a structured error in the first cut**, never treated as text or ignored. This keeps every query accepted today meaning the same once Phase 7 adds them.
9. **When `OR` arrives, AND binds tighter than OR** (GitHub's grammar), not JQL's left-to-right.

### Errors (recommendations)

Two passes, two error kinds:

- **Syntax**, from the parser, with a span and no knowledge of the Scheme (`expected_value`, `unterminated_quote`, `bare_word`, `unsupported`).
- **Resolution**, against the Scheme of the Projects in scope: `unknown_field` with every valid field, `unknown_value` with every valid option. This is the pass the bead's acceptance ("an unknown field returns a structured error listing valid fields") tests, so the parser alone cannot meet it; the resolver needs the Scheme as input.

Only enumerated fields can list valid values: status, type, priority, select and multi-select options, Issue types, and Actors for person fields. Text, number, date and URL fields can only report a type error (`invalid_value` with the expected type).

A shape that extends design.md's existing `missing_fields` example:

```json
{"error":"unknown_value","field":"status","value":"reveiw","span":[7,13],
 "valid":["triage","todo","in_progress","review","testing","done","cancelled"],
 "did_you_mean":["review"],"fix":"tm list status:review"}
```

- `span` is a byte offset pair into the filter string, as the probes produce. The web UI counts UTF-16 units, so either the JSON documents bytes and the UI converts, or the error carries both; bytes are the Rust-native choice.
- `did_you_mean`: case-insensitive exact match first (silently canonicalised, not an error), then Jaro > 0.7 as clap does, best first, at most three. `strsim` is one MIT crate and passes `cargo deny`. Writing Jaro by hand is about 40 lines if the dependency is unwanted.
- Several errors in one query: resolution can report every unknown field at once; the parser stops at the first syntax error.

### Parser (recommendation)

**Hand-written recursive descent, with a Pratt or precedence-climbing layer added when `OR` and parentheses arrive.** It adds no crates, builds in 0.4 s, and was the only probe whose errors named the construct and spanned it usefully without custom error types. The grammar is small enough that a combinator library's main benefit, composing many productions, does not pay for itself.

winnow is the fallback if the grammar grows past what reads well by hand: it passes `cargo deny`, costs nothing once `toml` is in the tree, and has a Pratt combinator. chumsky 0.12 and logos 0.16 are blocked by the repo's `deny.toml` today (license and duplicate versions), whatever their merits.

### SQL (recommendation)

**Compile every EAV and set term to `issues.id IN (subquery)` or `issues.id NOT IN (subquery)`, and built-in terms to plain column predicates.** `IN` was within 2x of the fastest form in every positive case (worst: 10.9 ms vs 6.0 ms for a JOIN at 100k), never duplicates rows, was the fastest negation (35.6 ms vs 57–119 ms), and composes under `OR` (`MULTI-INDEX OR`, 35 ms vs 163 ms for `EXISTS`). A JOIN is faster only for positive AND terms, needs `DISTINCT` for lists, and cannot express negation or `OR`; `EXISTS` scans every Issue whenever no built-in column narrows first.

The emitter therefore stays one shape per term, which is what OR and parentheses will need.

Sort: `LEFT JOIN field_values … ORDER BY num IS NULL, num` (or `CASE txt …` for selects), with `created_ms` as the final tie-break for stable pages. The index-walk-plus-`UNION ALL` form is a later optimisation for unfiltered sorts (0.05 ms vs 104 ms at 100k), not first-cut work.

At 100k Issues every first-cut probe finished in under 125 ms and most under 30 ms; at 10k, under 9 ms.

## Implications / actions

- **tm-u8g.7**: parser in `tm-core` (no I/O, no Scheme), resolver taking the Scheme and producing typed terms, emitter in `tm-store` producing parameterised SQL with generated placeholders for lists. Parser tests cover each construct and each reserved construct's `unsupported` error; resolver tests cover `unknown_field` with the valid list.
- **tm-u8g.6 (built-in columns and Scheme fields) must provide** what this emitter assumes: built-in columns on `issues` (today `schema.rs` keeps every value in `issues.fields` JSON), `priority` as an integer rank, `assignee` nullable, `field_values` with the two `(field, txt|num, issue_id)` indexes, and `issue_id NOT NULL` in `field_values` and `set_members`, the invariant `NOT IN` relies on.
- **Indexes on built-in columns** beyond `status` (type, assignee, `status_category`) are worth adding only when a measured filter needs them; the probes show the planner using `issues_status` well.
- **ADR candidate**: once .6 and .7 land, "EAV with typed columns, filters compiled to `IN` subqueries" is the choice projection-schema.md said should become an ADR.

## Open questions

- **`@me` when an Agent runs the filter.** design.md lists `@me` and `@agent` separately and records Agent writes as "agent, on behalf of human". Does `assignee:@me` from an Agent mean the human (recommended above, since the Assignee is always a human) or the Agent? Needs the user's decision.
- **Empty-value syntax.** design.md has none. Options: GitHub's `no:area` (cannot be negated, per GH-S), `area:none` (collides with a select option or label called `none`, and with priority's `none` level), or a reserved function `area:@empty` with `-area:@empty` for "has a value". The function form fits the grammar's existing `@` namespace and negates naturally; needs a decision.
- **Default sort direction.** GitHub defaults `sort:created` to descending; JQL and Plane default to ascending. Per-field defaults (dates descending, others ascending) or one global rule?
- **Default filter terms.** design.md says Triage Issues are left out of default Views; whether `tm list` adds `-status_category:triage` and `archived = 0` implicitly, and how a filter opts back in (`archived:true`?), is not specified.
- **Label case.** If labels are canonicalised against existing labels, what happens when two labels differ only by case (JRACLOUD-24907 shows Jira allows it)? Refusing such labels at write time would make the question moot.
- **Span units** for the web UI (bytes vs UTF-16), decided before the JSON error schema is versioned.

## Raw data

### SQL probe output (bundled SQLite 3.53.2), medians in ms

| Probe | 10k rows | 10k ms | 100k rows | 100k ms |
|---|---|---|---|---|
| `a_exists` | 34 | 0.583 | 340 | 11.797 |
| `a_join` | 34 | 0.593 | 340 | 10.708 |
| `a_in` | 34 | 0.735 | 340 | 11.409 |
| `a2_exists` | 73 | 4.265 | 699 | 59.974 |
| `a2_join` | 73 | 0.408 | 699 | 5.988 |
| `a2_in` | 73 | 0.729 | 699 | 10.871 |
| `b_any_exists` | 1809 | 7.974 | 18057 | 104.933 |
| `b_any_join_distinct` | 1809 | 1.180 | 18057 | 29.730 |
| `b_any_join_no_distinct` | 1905 | 0.836 | 19015 | 25.649 |
| `b_any_in` | 1809 | 1.175 | 18057 | 28.690 |
| `b_all_exists` | 96 | 4.340 | 958 | 60.131 |
| `b_all_join` | 96 | 0.781 | 958 | 17.532 |
| `b_all_in` | 96 | 0.603 | 958 | 9.231 |
| `b_all_group` | 96 | 0.431 | 958 | 6.088 |
| `c_not_exists` | 8556 | 4.227 | 85492 | 119.197 |
| `c_left_join_null` | 8556 | 4.492 | 85492 | 57.254 |
| `c_not_in` | 8556 | 2.505 | 85492 | 35.635 |
| `c_none_of_not_exists` | 7694 | 8.448 | 76948 | 111.488 |
| `c_select_not_exists` | 8200 | 4.178 | 82334 | 57.673 |
| `c_select_join_ne` | 6307 | 3.513 | 63338 | 106.186 |
| `c_no_area` | 1893 | 3.482 | 18996 | 101.354 |
| `d_due_left_join_limit` | 50 | 0.899 | 50 | 20.834 |
| `d_due_left_join_all` | 1902 | 1.120 | 18999 | 22.895 |
| `d_due_scalar_subquery_limit` | 50 | 1.529 | 50 | 28.630 |
| `d_due_unfiltered_left_join_limit` | 50 | 3.671 | 50 | 103.943 |
| `d_due_unfiltered_index_walk_limit` | 50 | 0.032 | 50 | 0.045 |
| `d_due_unfiltered_union_limit` | 50 | 0.037 | 50 | 0.052 |
| `d_area_case_limit` | 50 | 1.011 | 50 | 21.789 |
| `d_area_json_each_limit` | 50 | 1.627 | 50 | 28.829 |
| `d_priority_rank_limit` | 50 | 0.390 | 50 | 5.630 |
| `e_nocase_compare` | 1303 | 0.784 | 12671 | 20.718 |
| `e_canonical_compare` | 1303 | 0.562 | 12671 | 17.823 |
| `f_or_in` | 2177 | 1.364 | 21485 | 35.335 |
| `f_or_exists` | 2177 | 7.598 | 21485 | 163.034 |
| `f_or_union` | 2177 | 1.328 | 21485 | 32.246 |

Selected plans at 100k, verbatim from the runner:

```text
--- a2_exists [area:cli label:bug]
    SCAN i
    SEARCH f EXISTS USING COVERING INDEX field_values_txt (field=? AND txt=? AND issue_id=?)
    SEARCH s EXISTS USING COVERING INDEX set_members_by_elem (field=? AND elem=? AND issue_id=?)
--- a2_join [area:cli label:bug]
    SEARCH s USING COVERING INDEX set_members_by_elem (field=? AND elem=?)
    SEARCH f USING COVERING INDEX field_values_txt (field=? AND txt=? AND issue_id=?)
    SEARCH i USING INDEX sqlite_autoindex_issues_1 (id=?)
--- a2_in [area:cli label:bug]
    SEARCH i USING INDEX sqlite_autoindex_issues_1 (id=?)
    LIST SUBQUERY 1
      SEARCH field_values USING COVERING INDEX field_values_txt (field=? AND txt=?)
    LIST SUBQUERY 2
      SEARCH set_members USING COVERING INDEX set_members_by_elem (field=? AND elem=?)
      CREATE BLOOM FILTER
--- c_not_exists [-label:bug]
    SCAN i
    CORRELATED SCALAR SUBQUERY 1
      SEARCH s USING PRIMARY KEY (issue_id=? AND field=? AND elem=?)
--- c_not_in [-label:bug]
    SCAN i
    LIST SUBQUERY 1
      SEARCH set_members USING COVERING INDEX set_members_by_elem (field=? AND elem=?)
      CREATE BLOOM FILTER
--- d_due_unfiltered_index_walk_limit [sort:due (first page, Issues with a due date only, driven by the index)]
    SEARCH f USING COVERING INDEX field_values_num (field=?)
    SEARCH i USING INDEX sqlite_autoindex_issues_1 (id=?)
--- e_nocase_compare [area:CLI (matched with COLLATE NOCASE in SQL)]
    SEARCH f USING COVERING INDEX field_values_txt (field=?)
    SEARCH i USING INDEX sqlite_autoindex_issues_1 (id=?)
--- f_or_in [area:cli OR label:bug (later grammar; checks the IN form composes)]
    MULTI-INDEX OR
      INDEX 1
        LIST SUBQUERY 1
          SEARCH field_values USING COVERING INDEX field_values_txt (field=? AND txt=?)
        SEARCH i USING INDEX sqlite_autoindex_issues_1 (id=?)
      INDEX 2
        LIST SUBQUERY 2
          SEARCH set_members USING COVERING INDEX set_members_by_elem (field=? AND elem=?)
        SEARCH i USING INDEX sqlite_autoindex_issues_1 (id=?)
```

`nocase-and-null.sql` on macOS `sqlite3` 3.54.0, against a copy of the 100k store:

```text
3.54.0
|1|1
high < low < medium < none < urgent
QUERY PLAN
`--SEARCH set_members USING COVERING INDEX set_members_by_elem (field=?)
QUERY PLAN
`--SEARCH set_members USING COVERING INDEX set_members_nocase (field=? AND elem=?)
QUERY PLAN
`--SEARCH set_members USING COVERING INDEX set_members_by_elem (field=? AND elem=?)
10009
0
```

### Parser probe output (hand-written)

```text
ok   status:review,testing -type:spike                       terms [status=Text("review")|Text("testing"), -type=Text("spike")] sort []
ok   label:"good first issue" assignee:@me sort:updated-desc terms [label=Text("good first issue"), assignee=Function("me")] sort [updated desc]
ok   area:cli label:bug -label:wontfix                       terms [area=Text("cli"), label=Text("bug"), -label=Text("wontfix")] sort []
err  status:                                                 expected_value at 7..8: expected a value
err  label:"open                                             unterminated_quote at 6..11: missing closing `"`
err  status:review,                                          expected_value at 14..15: expected a value
err  :foo                                                    expected_field at 0..1: expected a field name before `:`
err  fix login                                               bare_word at 0..3: full-text search is not supported yet; use field:value
err  status:review OR status:testing                         unsupported at 14..16: `OR` is not supported yet
err  (status:review)                                         unsupported at 0..1: parentheses are not supported yet
err  priority:>=high                                         unsupported at 9..10: comparisons are not supported yet
err  iteration:@next(cli)                                    unsupported at 10..16: function arguments are not supported yet
```

### Suggestion probe output

```text
statu        clap(jaro>0.7): [status 0.94, estimate 0.77]  rustc(lev<=len/3): status (d=1)
stauts       clap(jaro>0.7): [status 0.94, estimate 0.72]  rustc(lev<=len/3): status (d=2)
asignee      clap(jaro>0.7): [assignee 0.96]  rustc(lev<=len/3): assignee (d=1)
labels       clap(jaro>0.7): [label 0.94]  rustc(lev<=len/3): label (d=1)
prio         clap(jaro>0.7): [priority 0.83, project 0.73]  rustc(lev<=len/3): -
Status       clap(jaro>0.7): [status 0.89]  rustc(lev<=len/3): status (d=0)
tpye         clap(jaro>0.7): [type 0.92]  rustc(lev<=len/3): -
xyz          clap(jaro>0.7): []  rustc(lev<=len/3): -
opn          clap(jaro>0.7): [done 0.72]  rustc(lev<=len/3): -
reveiw       clap(jaro>0.7): [review 0.94]  rustc(lev<=len/3): review (d=2)
in-progress  clap(jaro>0.7): [in_progress 0.94]  rustc(lev<=len/3): in_progress (d=1)
inprogress   clap(jaro>0.7): [in_progress 0.97]  rustc(lev<=len/3): in_progress (d=1)
test         clap(jaro>0.7): [testing 0.86]  rustc(lev<=len/3): -
Done         clap(jaro>0.7): [done 0.83]  rustc(lev<=len/3): done (d=0)
```

`opn` against statuses with no `open` shows the cost of a loose threshold: Jaro offers `done`, an unrelated status.
