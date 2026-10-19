"""Count a beads backlog the way the switch milestone checks it.

Usage: python3 -I beads_counts.py EXPORT.jsonl LIST.json

EXPORT.jsonl is `bd export` output (one issue per line); LIST.json is
`bd list --json --all --limit 0` output. Prints the counts the beads adapter
must reproduce (statuses, types, priorities, dependency types), which fields
the data carries, and whether the two bd outputs agree. People are printed as
counts and shapes only, never as names.
"""

import collections
import json
import sys


def load(export_path, list_path):
    with open(export_path, encoding="utf-8") as f:
        rows = [json.loads(line) for line in f if line.strip()]
    with open(list_path, encoding="utf-8") as f:
        listed = json.load(f)
    return rows, listed


def deps(issue):
    """(issue_id, depends_on_id, type) per edge; `bd show` nests the target issue instead."""
    out = []
    for dep in issue.get("dependencies") or []:
        if "depends_on_id" in dep:
            out.append((dep["issue_id"], dep["depends_on_id"], dep["type"]))
        else:
            out.append((issue["id"], dep["id"], dep.get("dependency_type")))
    return out


def tally(title, counter):
    print(f"{title}:")
    for key, count in sorted(counter.items(), key=lambda kv: (-kv[1], str(kv[0]))):
        print(f"  {key}: {count}")


def main():
    rows, listed = load(sys.argv[1], sys.argv[2])
    ids = {row["id"] for row in rows}
    print(f"export rows: {len(rows)}; list rows: {len(listed)}")
    print(f"record kinds (_type): {dict(collections.Counter(r.get('_type') for r in rows))}")
    tally("status", collections.Counter(r.get("status") for r in rows))
    tally("issue_type", collections.Counter(r.get("issue_type") for r in rows))
    tally("priority", collections.Counter(r.get("priority") for r in rows))

    edges = [e for r in rows for e in deps(r)]
    tally("dependency type", collections.Counter(e[2] for e in edges))
    dangling = [e for e in edges if e[1] not in ids]
    print(f"dependency edges: {len(edges)}; target outside the export: {len(dangling)}")
    wrong_side = [r["id"] for r in rows for e in deps(r) if e[0] != r["id"]]
    print(f"edges filed under another issue: {len(wrong_side)}")

    # `bd export` carries no `parent`; `bd list` derives it.
    parents = {r["id"]: r.get("parent") for r in listed if r.get("parent")}
    pc = {e[0]: e[1] for e in edges if e[2] == "parent-child"}
    print(f"list's parent field set: {len(parents)}; parent-child edges: {len(pc)}; agree: {parents == pc}")
    open_blockers = collections.Counter(
        next(r for r in rows if r["id"] == e[1]).get("status") for e in edges if e[2] == "blocks")
    print(f"blocks edges by the blocker's status: {dict(open_blockers)}")
    multi = collections.Counter(e[0] for e in edges if e[2] == "parent-child")
    print(f"issues with more than one parent-child edge: {sum(1 for c in multi.values() if c > 1)}")

    fields = collections.Counter(k for r in rows for k, v in r.items() if v not in (None, "", [], {}))
    tally("non-empty fields (issues carrying each)", fields)
    print(f"comments: {sum(len(r.get('comments') or []) for r in rows)}; "
          f"comment_count total: {sum(r.get('comment_count', 0) for r in rows)}")
    print(f"labels: {sum(len(r.get('labels') or []) for r in rows)}")
    dep_meta = collections.Counter(
        d.get("metadata") for r in rows for d in r.get("dependencies") or [])
    print(f"dependency metadata values: {dict(dep_meta)}")

    stamps = [r[k] for r in rows for k in ("created_at", "updated_at", "closed_at", "started_at") if r.get(k)]
    print(f"timestamps: {len(stamps)}; with fractional seconds: {sum('.' in s for s in stamps)}; "
          f"ending in Z: {sum(s.endswith('Z') for s in stamps)}")
    closed = [r for r in rows if r.get("status") == "closed"]
    print(f"closed: {len(closed)}; with closed_at: {sum(bool(r.get('closed_at')) for r in closed)}; "
          f"with close_reason: {sum(bool(r.get('close_reason')) for r in closed)}")
    for who in ("owner", "created_by", "assignee"):
        values = {r[who] for r in rows if r.get(who)}
        print(f"{who}: {sum(bool(r.get(who)) for r in rows)} issues, {len(values)} distinct, "
              f"{sum('@' in v for v in values)} look like emails")
    owner_pairs = {(r.get("owner"), r.get("created_by")) for r in rows}
    print(f"distinct (owner, created_by) pairs: {len(owner_pairs)}")

    by_id = {r["id"]: r for r in listed}
    same = set(by_id) == ids
    print(f"list and export name the same issues: {same}")
    for key in ("status", "priority", "issue_type", "title", "updated_at"):
        differ = [i for i in ids & set(by_id) if by_id[i].get(key) != next(r for r in rows if r["id"] == i).get(key)]
        print(f"  {key} differs on: {len(differ)}")
    list_edges = sorted(e for r in listed for e in deps(r))
    print(f"  dependency edges agree: {list_edges == sorted(edges)}")
    only_export = sorted(set().union(*(r.keys() for r in rows)) - set().union(*(r.keys() for r in listed)))
    only_list = sorted(set().union(*(r.keys() for r in listed)) - set().union(*(r.keys() for r in rows)))
    print(f"  keys only in export: {only_export}; only in list: {only_list}")


if __name__ == "__main__":
    main()
