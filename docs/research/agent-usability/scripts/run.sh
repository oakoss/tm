#!/usr/bin/env bash
# Runs the agent-usability tasks (../agent-usability.md) against Claude Code
# or Codex: each task in a fresh headless session over a freshly seeded
# scratch Workspace, then checks the Workspace and scores the run.
#
# usage: run.sh claude|codex [TASK...]
# TASK is one of take-next comment describe blocked finish undo handoff;
# with none, all of them. Results go to $OUT, a new temporary directory by
# default; they hold transcripts and machine paths, so keep them out of the
# repository.

set -euo pipefail

TASKS=(take-next comment describe blocked finish undo handoff)

agent=${1:-}
case $agent in
  claude | codex) shift ;;
  *)
    echo "usage: run.sh claude|codex [TASK...]" >&2
    exit 2
    ;;
esac
tasks=("$@")
[[ ${#tasks[@]} -gt 0 ]] || tasks=("${TASKS[@]}")
for task in "${tasks[@]}"; do
  [[ " ${TASKS[*]} " == *" $task "* ]] || {
    echo "unknown task: $task" >&2
    exit 2
  }
done
command -v tm >/dev/null || {
  echo "tm is not on PATH" >&2
  exit 1
}
command -v "$agent" >/dev/null || {
  echo "$agent is not on PATH" >&2
  exit 1
}
if [[ $agent == codex && ! -f $HOME/.codex/auth.json ]]; then
  echo "no Codex login at ~/.codex/auth.json" >&2
  exit 1
fi

OUT=${OUT:-$(mktemp -d "${TMPDIR:-/tmp}/tm-usability.XXXXXX")}
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)

# Never writes ~/.codex: a file Codex left in the link's place may be a
# partial token refresh, so it is kept outside the task directories, which a
# rerun deletes, for the person to check.
auth_link=
auth_kept=
release_auth() {
  [[ -n $auth_link ]] || return 0
  if [[ -e $auth_link && ! -L $auth_link ]]; then
    auth_kept=$(mktemp "$OUT/codex-auth.json.left-by-codex.$task.XXXXXX" 2>/dev/null) ||
      auth_kept="$OUT/codex-auth.json.left-by-codex.$task"
    chmod 600 "$auth_link" 2>/dev/null || true
    mv -f "$auth_link" "$auth_kept" 2>/dev/null || true
    echo "run.sh: Codex replaced its auth.json link, perhaps refreshing its login;" \
      "check $auth_kept and, if it holds a valid login, copy it to ~/.codex/auth.json" |
      tee -a "$OUT/$agent-scores.txt" >&2 || true
  fi
  rm -f "$auth_link" 2>/dev/null || true
  auth_link=
}
task=none
on_exit() {
  local status=$?
  [[ $status -eq 0 ]] || echo "run.sh: stopped at task $task" >&2
  echo "results: $OUT" >&2
  release_auth || true
}
trap on_exit EXIT
trap 'exit 130' INT
trap 'exit 143' TERM HUP

# Variables a harness, tm or git reads for identity: the session running
# this script must not lend its own to the Workspace or the Agent it starts.
scrubbed=()
while IFS='=' read -r name _; do
  case $name in
    CLAUDE* | CODEX* | TM_* | GIT_* | GEMINI_CLI | OPENCODE | CURSOR_AGENT) scrubbed+=(-u "$name") ;;
  esac
done < <(env)

# Runs a command against the task's Workspace without this session's
# variables: as the human, or as the Agent its harness names.
in_workspace() {
  # macOS bash 3.2 reads an empty array as unset under set -u.
  env ${scrubbed[@]+"${scrubbed[@]}"} \
    GIT_CONFIG_GLOBAL="$dir/gitconfig" GIT_CONFIG_NOSYSTEM=1 \
    TM_HOME="$dir/tm-home" "$@"
}

tm_() { (cd "$dir/repo" && in_workspace tm "$@" >/dev/null); }
json() { (cd "$dir/repo" && in_workspace tm --json "$@"); }

PREAMBLE="You are working in the git repository in your current directory. \
Its work is tracked with the tm command-line tool; run tm --help to learn it. \
Do not change any file other than through tm unless the task says so."

seed() {
  case $1 in
    take-next)
      tm_ create --type task --title "Tidy the README" --set priority=low
      tm_ create --type bug --title "Fix the parser crash on empty input" --set priority=high
      tm_ create --type task --title "Add a --version flag" --set priority=medium
      ;;
    comment | finish | undo | handoff)
      local extra=()
      [[ $1 == finish ]] && extra=(--set status=in_progress --set assignee=jace@example.com)
      [[ $1 == undo ]] && extra=(--set priority=high)
      tm_ create --type bug --title "Fix the parser crash on empty input" ${extra[@]+"${extra[@]}"}
      ;;
    describe)
      tm_ create --type task --title "Add a --version flag"
      cat >"$dir/plan.expected" <<'EOF'
Print the version from Cargo.toml with `--version` and `-V`.

- Read it with env!("CARGO_PKG_VERSION"); no build script.
- Exit 0 and print nothing else, so `$(tool -V)` is just the number.
- Test: "tool --version" prints "tool 1.2.3".
EOF
      cp "$dir/plan.expected" "$dir/repo/plan.md"
      ;;
    blocked)
      tm_ create --type task --title "Design the config file"
      tm_ create --type task --title "Choose a TOML crate"
      tm_ create --type task --title "Load the config file"
      tm_ create --type task --title "Validate the config file"
      tm_ create --type task --title "Write the changelog"
      tm_ link EV-1 blocks EV-3
      tm_ link EV-2 blocks EV-4
      ;;
  esac
}

prompt() {
  case $1 in
    take-next) echo "Take the next piece of work tm has for you, and start it. Do not write any code." ;;
    comment) echo "Record on EV-1 that the crash comes from the parser's handling of empty input, and that you will fix it next." ;;
    describe) echo "Replace EV-1's description with the plan in plan.md, exactly as written there." ;;
    blocked) echo "Which Issues are blocked right now, and by what? End your answer with one line per blocked Issue, written as: BLOCKED <key> by <key>" ;;
    finish) echo "The fix for EV-1 has landed. Mark EV-1 finished in tm." ;;
    undo) echo "Set EV-1's priority to low. Then: that was a mistake, and EV-1 should not have changed. Undo it so EV-1 is as it was before." ;;
    handoff) echo "Start EV-1. Then hand it off to $(other) to finish: the empty-input case is fixed, and only the error message is left." ;;
  esac
}

# The Agent a handoff goes to: the one not under test.
other() {
  [[ $agent == claude ]] && echo codex || echo claude
}

# Prints a line per failed condition; none means the task was done.
check() {
  local show
  case $1 in
    take-next)
      show=$(json show EV-2)
      jq -e '.data.fields.status == "in_progress"' <<<"$show" >/dev/null || echo "EV-2, the most urgent, is not started"
      jq -e '.data.claim.agent != null' <<<"$show" >/dev/null || echo "EV-2 holds no Claim"
      ;;
    comment)
      json show EV-1 | jq -e '[.data.comments[].body | ascii_downcase | select(contains("empty input"))] | length > 0' >/dev/null ||
        echo "EV-1 has no comment naming the empty input"
      ;;
    describe)
      [[ $(json show EV-1 | jq -r '.data.fields.description // ""') == "$(cat "$dir/plan.expected")" ]] ||
        echo "EV-1's description differs from the plan seeded in plan.md"
      ;;
    blocked)
      # Every BLOCKED line the answer gives, which must be exactly these two.
      local named
      named=$(grep -oE 'BLOCKED EV-[0-9]+ by EV-[0-9]+' "$dir/answer.txt" | sort -u | tr '\n' ';' || true)
      [[ $named == "BLOCKED EV-3 by EV-1;BLOCKED EV-4 by EV-2;" ]] ||
        echo "the answer's BLOCKED lines are ${named:-none}, not EV-3 by EV-1 and EV-4 by EV-2"
      ;;
    finish)
      [[ $(json show EV-1 | jq -r '.data.fields.status') == "done" ]] || echo "EV-1 is not done"
      ;;
    undo)
      json history EV-1 | jq -e '[.data.writes[] | select(.actor != "jace@example.com") | .changes[].change
        | select(.field == "priority" and .value == "low")] | length > 0' >/dev/null ||
        echo "the Agent never set EV-1's priority to low"
      [[ $(json show EV-1 | jq -r '.data.fields.priority') == high ]] || echo "EV-1's priority is not high again"
      ;;
    handoff)
      show=$(json show EV-1)
      jq -e --arg to "$(other)" '.data.claim.handed == true and .data.claim.agent == $to' <<<"$show" >/dev/null ||
        echo "EV-1's Claim is not handed to $(other)"
      jq -e '[.data.comments[].body | ascii_downcase | select(contains("error message"))] | length > 0' <<<"$show" >/dev/null ||
        echo "EV-1 has no note naming the error message"
      ;;
  esac
}

# Output that marks a tm misstep, wherever it falls in a chained call:
# clap's usage error, tm's text error (not prime's heading or a warning), or
# tm's JSON error, whose keys are sorted and may be pretty-printed.
ERROR_OUTPUT='(^|\n)(error: |tm: (?!Project |warning: ))|"error": ?"[a-z_]+"'

# The transcript's JSON events; a line that is not JSON, such as a CLI's
# notice, is skipped rather than ending the run.
events() { jq -c -R 'fromjson? // empty' "$dir/transcript.jsonl"; }

# Each run_ function leaves why the Agent failed in $why, empty when it ran
# to a final answer, and its commands in commands.txt: tm-error when the
# output carries an error, fail when it only exited non-zero.
run_claude() {
  local rc=0
  (cd "$dir/repo" && in_workspace claude -p "$PREAMBLE

$(prompt "$task")" \
    --setting-sources "" --strict-mcp-config --no-session-persistence \
    --allowedTools Bash Read Write Edit Glob Grep \
    --output-format stream-json --verbose) </dev/null >"$dir/transcript.jsonl" 2>"$dir/stderr.txt" || rc=$?
  events | jq -r 'select(.type == "result") | .result // ""' >"$dir/answer.txt" || true
  if [[ $rc -ne 0 ]]; then
    why="exit $rc: $(reason)"
  elif ! events | jq -e -s 'any(.type == "result" and .subtype == "success" and (.is_error | not))' >/dev/null; then
    why="no successful result: $(reason)"
  fi
  events | jq -rs --arg err "$ERROR_OUTPUT" '
    def text: if type == "array" then map(.text? // "") | join("") else (. // "" | tostring) end;
    . as $all
    | ([$all[] | select(.type == "user") | .message.content | arrays | .[] | select(.type? == "tool_result")
        | {key: .tool_use_id,
           value: (if (.content | text) | test($err) then "tm-error"
                   elif .is_error then "fail    " else "ok      " end)}]
       | from_entries) as $marks
    | $all[] | select(.type == "assistant") | .message.content | arrays | .[]
    | select(.type? == "tool_use" and .name == "Bash")
    | "\($marks[.id] // "no-result") \(.input.command // "" | gsub("\n"; " ⏎ "))"' \
    >"$dir/commands.txt" || touch "$dir/unreadable"
}

run_codex() {
  local rc=0
  mkdir -p "$dir/codex-home"
  auth_link="$dir/codex-home/auth.json"
  ln -s "$HOME/.codex/auth.json" "$auth_link"
  # A subshell, so an interrupt's lines reach the terminal, not stderr.txt.
  (in_workspace env CODEX_HOME="$dir/codex-home" codex exec \
    --ephemeral --skip-git-repo-check --sandbox workspace-write \
    -C "$dir/repo" --add-dir "$dir/tm-home" --json -o "$dir/answer.txt" \
    "$PREAMBLE

$(prompt "$task")") </dev/null >"$dir/transcript.jsonl" 2>"$dir/stderr.txt" || rc=$?
  release_auth
  if [[ $rc -ne 0 ]]; then
    why="exit $rc: $(reason)"
  elif [[ ! -s $dir/answer.txt ]]; then
    why="no final answer: $(reason)"
  fi
  [[ -f $dir/answer.txt ]] || : >"$dir/answer.txt"
  events | jq -r --arg err "$ERROR_OUTPUT" '
    select(.type == "item.completed" and .item.type? == "command_execution")
    | (if (.item.aggregated_output // "" | tostring) | test($err) then "tm-error"
       elif .item.exit_code != 0 then "fail    " else "ok      " end) as $mark
    | "\($mark) \(.item.command // "" | gsub("\n"; " ⏎ "))"' \
    >"$dir/commands.txt" || touch "$dir/unreadable"
}

# Lines that name tm; its help, version or a lookup of it alone do not.
tm_calls() {
  sed -E "s/(which|command -v|type) tm//g; s/tm (--help|-h|--version|-V)([ '\";|)&]|$)/\2/g" |
    grep -cE "(^|[ '\"(;&|])([^ '\"]*/)?tm( |$)" || true
}

# The first line the Agent's CLI gave on stderr, else its final answer.
reason() {
  local line
  line=$(grep -m1 . "$dir/stderr.txt" || grep -m1 . "$dir/answer.txt" || true)
  echo "${line:-no output}"
}

# Not mise's shim: every build reports 0.0.0, so the binary's build time tells runs apart.
tm_bin=$(mise which tm 2>/dev/null || command -v tm)
{
  echo "date: $(date -u +%Y-%m-%dT%H:%MZ)"
  echo "tm: $(tm --version) at $tm_bin, built $(date -u -r "$tm_bin" +%Y-%m-%dT%H:%MZ)"
  echo "repo: $(git -C "$(dirname "$0")" rev-parse --short HEAD 2>/dev/null || echo unknown)"
  echo "agent: $agent $($agent --version 2>/dev/null | head -1)"
} >"$OUT/$agent-versions.txt"
echo "# $(date -u +%Y-%m-%dT%H:%MZ)" >>"$OUT/$agent-scores.txt"

for task in "${tasks[@]}"; do
  dir="$OUT/$agent-$task"
  rm -rf "$dir"
  mkdir -p "$dir/repo" "$dir/tm-home"
  printf '[user]\n\temail = jace@example.com\n\tname = Jace\n' >"$dir/gitconfig"
  (cd "$dir/repo" && in_workspace git init -q && in_workspace tm init --yes --key EV --name eval </dev/null >/dev/null)
  seed "$task"
  why=
  "run_$agent"
  failed=
  # A failed call that ran tm counts even with no error shown: stderr may be discarded.
  misses=$(($(grep -c '^tm-error' "$dir/commands.txt" || true) + $( (grep '^fail' "$dir/commands.txt" || true) | tm_calls)))
  ran_tm=$(tm_calls <"$dir/commands.txt")
  if [[ -n $why ]]; then
    score="agent failed: $why"
  elif ! (cd "$dir/repo" && in_workspace tm show EV-1 >/dev/null 2>"$dir/check-error.txt"); then
    score="unscored: the harness could not read the Workspace: $(head -1 "$dir/check-error.txt")"
  elif [[ -f $dir/unreadable ]]; then
    score="unscored: the transcript could not be read"
  else
    failed=$(check "$task") || true
    if [[ -n $failed ]]; then
      score="not done"
    elif [[ $ran_tm -eq 0 ]]; then
      score="unscored: no tm command found in the transcript"
    elif [[ $misses -gt 0 ]]; then
      score="done after $misses tm error(s)"
    else
      score="done unaided"
    fi
  fi
  printf '%s  %-9s  %s\n' "$agent" "$task" "$score" | tee -a "$OUT/$agent-scores.txt"
  [[ -z $failed ]] || awk '{ print "    " $0 }' <<<"$failed" | tee -a "$OUT/$agent-scores.txt"
  # The next session would link a login the refresh may have retired.
  if [[ -n $auth_kept ]]; then
    echo "run.sh: stopped after $task; restore ~/.codex/auth.json, then rerun the rest" |
      tee -a "$OUT/$agent-scores.txt" >&2
    exit 1
  fi
done
task=none
