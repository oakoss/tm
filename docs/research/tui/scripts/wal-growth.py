"""How the WAL grows while a reader holds a transaction across writes and
PASSIVE checkpoints, against a reader that opens a fresh one each round.

Usage: python3 -I wal-growth.py STORE.db SCRATCH_DIR  (copies STORE.db into SCRATCH_DIR)
"""

import os
import sqlite3
import sys

source, scratch = sys.argv[1], sys.argv[2]


def run(hold):
    path = os.path.join(scratch, f"wal-{hold}.db")
    for suffix in ("", "-wal", "-shm"):
        try:
            os.remove(path + suffix)
        except FileNotFoundError:
            pass
    # A file copy would miss commits still in the source's WAL.
    with sqlite3.connect(source) as src, sqlite3.connect(path) as dst:
        src.backup(dst)
    reader = sqlite3.connect(path, isolation_level=None)
    writer = sqlite3.connect(path, isolation_level=None)
    writer.execute("PRAGMA journal_mode=WAL")
    if hold:
        reader.execute("BEGIN")
        reader.execute("SELECT count(*) FROM ops").fetchone()
    sizes = []
    for _ in range(5):
        for j in range(200):
            writer.execute(
                "INSERT OR REPLACE INTO meta(k, v) VALUES (?, ?)", (f"p{j}", "x" * 2000)
            )
        if not hold:
            reader.execute("SELECT count(*) FROM ops").fetchone()
        writer.execute("PRAGMA wal_checkpoint(PASSIVE)").fetchone()
        sizes.append(os.path.getsize(path + "-wal") // 1024)
    print(f"reader holds a transaction={hold}: WAL KiB after each round {sizes}")


run(True)
run(False)
