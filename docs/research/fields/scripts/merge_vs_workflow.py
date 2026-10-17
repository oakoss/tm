"""Replays concurrent writes under ADR 0006's per-field rule and checks the
result against a Workflow. The rule mirrors `Version::beats` in
crates/tm-store/src/apply.rs for acknowledged operations.

Run: python3 -I merge_vs_workflow.py
"""

from dataclasses import dataclass

TRANSITIONS = {
    "todo": {"in_progress", "cancelled"},
    "in_progress": {"todo", "in_review"},
    "in_review": {"in_progress", "done"},
    "done": set(),
    "cancelled": {"todo"},
}
REQUIRED_TO_ENTER = {"in_progress": {"area"}, "in_review": {"area"}}


@dataclass
class Op:
    replica: str
    field: str
    value: object
    hlc: int
    base_seq: int
    server_seq: int = 0


def beats(new: Op, cur: Op) -> bool:
    if new.base_seq >= cur.server_seq:
        return True
    if cur.base_seq >= new.server_seq:
        return False
    return (new.hlc, new.replica) > (cur.hlc, cur.replica)


def replay(ops):
    heads, state = {}, {}
    for op in sorted(ops, key=lambda o: o.server_seq):
        cur = heads.get(op.field)
        if cur is None or beats(op, cur):
            heads[op.field] = op
            state[op.field] = op.value
    return state


def check_write(state, field, value):
    """The write-time check a Replica runs against its own projection."""
    if field != "status":
        if value is None and field in REQUIRED_TO_ENTER.get(state["status"], set()):
            return f"refused: {field} required in {state['status']}"
        return "allowed"
    if value not in TRANSITIONS[state["status"]]:
        return f"refused: {state['status']} -> {value} not a transition"
    missing = [f for f in REQUIRED_TO_ENTER.get(value, ()) if state.get(f) is None]
    return f"refused: missing {missing}" if missing else "allowed"


def violations(state):
    return [f for f in REQUIRED_TO_ENTER.get(state["status"], ()) if state.get(f) is None]


def scenario(name, base, local_a, local_b, order):
    print(f"--- {name}")
    shared = replay(base)
    print("shared state:", shared)
    last = {}
    for label, ops in (("A", local_a), ("B", local_b)):
        st = dict(shared)
        for op in ops:
            print(f"  {label} writes {op.field}={op.value!r}: {check_write(st, op.field, op.value)}")
            st[op.field] = op.value
        last[label] = st["status"]
    seq = len(base)
    for op in order:
        seq += 1
        op.server_seq = seq
    merged = replay(base + order)
    print("merged state:", merged, "| missing required:", violations(merged))
    for label, before in last.items():
        if before != merged["status"]:
            ok = merged["status"] in TRANSITIONS[before]
            print(f"  {label} sees {before} -> {merged['status']}: {'a transition' if ok else 'not a transition'}")


def base_ops():
    return [
        Op("A", "status", "todo", hlc=1, base_seq=0, server_seq=1),
        Op("A", "area", "cli", hlc=2, base_seq=1, server_seq=2),
    ]


a = Op("A", "status", "in_progress", hlc=10, base_seq=2)
b = Op("B", "area", None, hlc=11, base_seq=2)
scenario("1: transition and field clear meet", base_ops(), [a], [b], [a, b])

a1 = Op("A", "status", "in_progress", hlc=10, base_seq=2)
a2 = Op("A", "status", "in_review", hlc=12, base_seq=2)
b1 = Op("B", "status", "cancelled", hlc=11, base_seq=2)
scenario("2a: two status paths, A's later clock", base_ops(), [a1, a2], [b1], [a1, b1, a2])

a1 = Op("A", "status", "in_progress", hlc=10, base_seq=2)
a2 = Op("A", "status", "in_review", hlc=12, base_seq=2)
b1 = Op("B", "status", "cancelled", hlc=13, base_seq=2)
scenario("2b: two status paths, B's later clock", base_ops(), [a1, a2], [b1], [a1, a2, b1])
