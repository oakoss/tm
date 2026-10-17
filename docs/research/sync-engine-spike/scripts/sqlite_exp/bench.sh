#!/bin/bash
# Mean wall time over 20 runs of a read-only count query, per runtime.
t() { local s=$(python3 -c 'import time;print(time.time())'); for i in $(seq 20); do "$@" >/dev/null; done; python3 -c "import time;print('%s: %.1f ms/run' % ('$1', (time.time()-$s)*1000/20))"; }
t /opt/homebrew/opt/sqlite/bin/sqlite3 -readonly big.db 'select count(*) from issue'
t node -e "const {DatabaseSync}=require('node:sqlite');new DatabaseSync('big.db',{readOnly:true}).prepare('select count(*) from issue').get()"
t bun -e "import {Database} from 'bun:sqlite';new Database('big.db',{readonly:true}).query('select count(*) from issue').get()"
