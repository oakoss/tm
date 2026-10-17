#!/usr/bin/env bash
# Which git reads find a repo's root and remote, which miss them, and which
# write under .git. Usage: git-probe.sh <empty scratch dir>
set -euo pipefail

scratch=${1:?usage: git-probe.sh <empty scratch dir>}
cd "$scratch" > /dev/null

# Fixture git sees no user or system config.
g() { GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 git -c user.name=probe -c user.email=probe@example.invalid "$@"; }
r() { GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 git "$@"; }

# Every file under .git with its mtime in nanoseconds, for before/after diffs.
if stat -f '%Fm' / > /dev/null 2>&1; then
  snap() { find "$1" -type f -exec stat -f '%Fm %N' {} + | sort -k2; }
else
  snap() { find "$1" -type f -exec stat -c '%.9Y %n' {} + | sort -k2; }
fi

g init -q -b main repo
cd repo
mkdir -p pkg/sub
echo a > pkg/sub/file.txt
g add -A
g commit -qm init

# A remote whose URL is rewritten by insteadOf, and one defined in an included file.
cat >> .git/config <<'EOF'
[url "git@github.com:"]
	insteadOf = gh:
[remote "origin"]
	url = gh:oakoss/tm.git
	fetch = +refs/heads/*:refs/remotes/origin/*
[include]
	path = ../../shared.gitconfig
EOF
cat > ../shared.gitconfig <<'EOF'
[remote "upstream"]
	url = https://github.com/Oakoss/TM
EOF

echo "== remote URLs"
echo "hand-parsed .git/config, origin:   $(awk '/\[remote "origin"\]/{f=1;next} /^\[/{f=0} f && /url =/{print $3}' .git/config)"
echo "hand-parsed .git/config, upstream: $(awk '/\[remote "upstream"\]/{f=1;next} /^\[/{f=0} f && /url =/{print $3}' .git/config)"
echo "git config --get remote.origin.url:   $(r config --get remote.origin.url)"
echo "git remote get-url origin:            $(r remote get-url origin)"
echo "git config --get remote.upstream.url: $(r config --get remote.upstream.url)"
echo "git remote get-url upstream:          $(r remote get-url upstream)"

echo "== root discovery from a subdirectory"
(cd pkg/sub && echo "rev-parse --show-toplevel: $(r rev-parse --show-toplevel)" && echo "rev-parse --show-prefix:   $(r rev-parse --show-prefix)")

echo "== linked worktree"
g worktree add -q ../wt -b wt
echo ".git in the worktree is a: $(test -f ../wt/.git && echo file || echo directory)"
echo "its contents: $(cat ../wt/.git)"
(cd ../wt/pkg && echo "rev-parse --show-toplevel:  $(r rev-parse --show-toplevel)" \
  && echo "rev-parse --git-dir:        $(r rev-parse --git-dir)" \
  && echo "rev-parse --git-common-dir: $(r rev-parse --git-common-dir)" \
  && echo "remote get-url origin:      $(r remote get-url origin)")

echo "== writes under .git by read commands"
snap .git > ../before.txt
for cmd in "rev-parse --show-toplevel" "rev-parse --git-common-dir" "config --get remote.origin.url" "remote get-url origin" "log -1 --format=%H" "for-each-ref refs/heads"; do
  # shellcheck disable=SC2086
  r $cmd > /dev/null
done
snap .git > ../after.txt
echo "files changed by rev-parse/config/remote get-url/log/for-each-ref: $(diff ../before.txt ../after.txt | grep -c '^>' || true)"

# Make the index stat-dirty: same content, newer mtime.
sleep 1
touch pkg/sub/file.txt
snap .git > ../before.txt
GIT_OPTIONAL_LOCKS=0 r status --porcelain > /dev/null
snap .git > ../after.txt
echo "files changed by GIT_OPTIONAL_LOCKS=0 git status: $(diff ../before.txt ../after.txt | grep -c '^>' || true)"
r status --porcelain > /dev/null
snap .git > ../after2.txt
echo "files changed by plain git status: $(diff ../after.txt ../after2.txt | grep '^>' | sed 's/^> [0-9]* //' | tr '\n' ' ')"

echo "== cost of shelling out (20 runs each)"
t0=$(perl -MTime::HiRes=time -e 'printf "%.0f", time*1000')
for _ in $(seq 20); do r rev-parse --show-toplevel > /dev/null; done
t1=$(perl -MTime::HiRes=time -e 'printf "%.0f", time*1000')
for _ in $(seq 20); do r remote get-url origin > /dev/null; done
t2=$(perl -MTime::HiRes=time -e 'printf "%.0f", time*1000')
echo "rev-parse --show-toplevel: $(( (t1 - t0) / 20 )) ms per run; remote get-url: $(( (t2 - t1) / 20 )) ms per run"
