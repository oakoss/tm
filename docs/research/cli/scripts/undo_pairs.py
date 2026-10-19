"""Random writes and undoes on one field under ADR 0010's conflict rule and
ADR 0012's narrowed ones. After each sequence, the field must equal a replay
of the writes still in effect: those not undone, in log order.

Run: python3 -I undo_pairs.py
"""

import random

RULES = ("adr-0010", "any-undone-pair", "later-undone-pair")


def run(rule, rng, steps=12):
    log = []  # (batch, value, undid or None)
    initial = "v0"

    def value_before(i):
        return log[i - 1][1] if i else initial

    def undone(i):
        return any(u[2] == i and not undone(j) for j, u in enumerate(log) if u[2] is not None)

    def later(i, target):
        return rule == "any-undone-pair" or i > target

    def ignored(i, target):
        if undone(i):
            return later(i, target)
        undid = log[i][2]
        return undid is not None and later(undid, target)

    for step in range(steps):
        if not log or rng.random() < 0.5:
            log.append((step, f"v{step + 1}", None))
            continue
        target = rng.randrange(len(log))
        since = range(target + 1, len(log))
        if rule == "adr-0010":
            conflict = bool(since)
        else:
            conflict = any(not ignored(i, target) for i in since)
        prior = value_before(target)
        if conflict or log[-1][1] == prior:
            continue
        log.append((step, prior, target))

    in_effect = [i for i, b in enumerate(log) if b[2] is None and not undone(i)]
    expected = log[in_effect[-1]][1] if in_effect else initial
    actual = log[-1][1] if log else initial
    undoes = sum(b[2] is not None for b in log)
    return actual == expected, undoes


def main():
    for rule in RULES:
        rng = random.Random(12)
        results = [run(rule, rng) for _ in range(20_000)]
        wrong = sum(not ok for ok, _ in results)
        undoes = sum(n for _, n in results)
        print(f"{rule:18} wrong {wrong:5} of 20000, undoes applied {undoes}")


main()
