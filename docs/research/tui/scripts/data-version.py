"""Whether PRAGMA data_version sees another process's commit, and what a poll costs.

Usage: python3 -I data-version.py COPY_OF_A_STORE.db  (writes to the file it is given)
"""

import sqlite3
import subprocess
import sys
import time

path = sys.argv[1]
reader = sqlite3.connect(path, isolation_level=None)


def version():
    return reader.execute("PRAGMA data_version").fetchone()[0]


print("journal:", reader.execute("PRAGMA journal_mode").fetchone()[0])
before = version()
reader.execute("INSERT OR REPLACE INTO meta(k, v) VALUES ('probe-self', 1)")
print("own write moves it:", version() != before)
before = version()
subprocess.run(
    ["sqlite3", path, "INSERT OR REPLACE INTO meta(k, v) VALUES ('probe-other', 2)"],
    check=True,
)
print("other process write moves it:", version() != before, before, "->", version())
before = version()
print("no write, unchanged:", version() == before)
fresh = sqlite3.connect(path, isolation_level=None)
print("a new connection starts at:", fresh.execute("PRAGMA data_version").fetchone()[0])
n = 100_000
start = time.perf_counter()
for _ in range(n):
    version()
print(f"poll cost: {(time.perf_counter() - start) / n * 1e6:.2f} us/call")
