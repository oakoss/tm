// usage: node sync_mp.mjs <tursodb> <workdir> <port> [multiprocess_wal]
// Parent opens a sync replica and holds it; a child process opens the same replica path and writes.
import { connect } from "@tursodatabase/sync";
import { spawn, execFileSync } from "node:child_process";
import { mkdirSync, rmSync } from "node:fs";

const [tursodb, dir, port, feat] = process.argv.slice(2);
const url = `http://127.0.0.1:${port}`;
const experimental = feat ? [feat] : [];
const path = `${dir}/a/local.db`;

if (process.env.CHILD) {
  try {
    const db = await connect({ path, url, clientName: "child", experimental });
    await db.exec(`INSERT INTO t VALUES ('child')`);
    console.log("  child: wrote ok; rows =", JSON.stringify(await (await db.prepare("SELECT count(*) c FROM t")).all()));
    await db.close();
  } catch (e) { console.log("  child: ERROR", e.message.split("\n")[0]); }
  process.exit(0);
}

rmSync(dir, { recursive: true, force: true });
mkdirSync(`${dir}/a`, { recursive: true }); mkdirSync(`${dir}/server`, { recursive: true });
const server = spawn(tursodb, ["--sync-server", `127.0.0.1:${port}`, `${dir}/server/server.db`], { stdio: "ignore" });
await new Promise((r) => setTimeout(r, 700));
try {
  const db = await connect({ path, url, clientName: "parent", experimental });
  await db.exec(`CREATE TABLE IF NOT EXISTS t(x TEXT)`);
  await db.exec(`INSERT INTO t VALUES ('parent')`);
  console.log(`  parent (experimental=${JSON.stringify(experimental)}): open, holding`);
  console.log(execFileSync(process.execPath, [process.argv[1], ...process.argv.slice(2)], { env: { ...process.env, CHILD: "1" }, encoding: "utf8" }).trimEnd());
  console.log("  parent sees rows =", JSON.stringify(await (await db.prepare("SELECT x FROM t")).all()));
  await db.close();
} catch (e) { console.log("  parent: ERROR", e.message.split("\n")[0]); }
server.kill("SIGTERM");
