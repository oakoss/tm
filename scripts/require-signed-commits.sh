#!/usr/bin/env bash
# Refuse a push whose commit objects have no gpgsig header.
#
# `%G?` needs allowed_signers. The header is present if signing ran, and missing
# if it did not, even with a blank GIT_CONFIG_GLOBAL.
set -euo pipefail
# Pack transfer ignores replace refs; cat-file does not (git-replace(1)).
export GIT_NO_REPLACE_OBJECTS=1

zero=0000000000000000000000000000000000000000
remote=${1-}
status=0

# awk reads the whole object so a large message cannot SIGPIPE the pipeline
# into a false reject. Matches only before the first blank line so a body
# line that looks like a header cannot pass.
has_gpgsig() {
  git cat-file -p "$1" | awk '
    /^$/ { body = 1 }
    !body && /^gpgsig(-sha256)? / { found = 1 }
    END { exit !found }
  '
}

while read -r local_ref local_sha _remote_ref remote_sha; do
  [[ -n "${local_ref:-}" ]] || continue
  if [[ "$local_sha" == "$zero" ]]; then
    continue
  fi
  if [[ "$remote_sha" == "$zero" ]]; then
    # Only the destination remote: an unsigned commit already on another
    # remote-tracking ref is still new to this push.
    if [[ -n "$remote" ]]; then
      commits=$(git rev-list "$local_sha" --not --remotes="$remote")
    else
      commits=$(git rev-list "$local_sha" --not --remotes)
    fi
  else
    if ! git cat-file -e "$remote_sha^{commit}" 2>/dev/null; then
      echo "require-signed-commits: $_remote_ref is at $remote_sha, which is not in this clone; run git fetch and push again" >&2
      status=1
      continue
    fi
    commits=$(git rev-list "$remote_sha..$local_sha")
  fi
  # `rev-list` prints nothing when the range is empty; the loop must not
  # treat that as a single empty sha.
  [[ -n "$commits" ]] || continue
  while read -r sha; do
    [[ "$(git cat-file -t "$sha")" == commit ]] || continue
    if ! has_gpgsig "$sha"; then
      echo "require-signed-commits: $sha on $local_ref has no gpgsig header" >&2
      status=1
    fi
  done <<<"$commits"
done

exit "$status"
