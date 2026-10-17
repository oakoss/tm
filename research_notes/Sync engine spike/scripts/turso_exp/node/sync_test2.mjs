// usage: node sync_test2.mjs <tursodb> <workdir> <port>
// Schema and sequence trigger created directly on the server DB before any client bootstraps.
// Then: offline inserts on both replicas, disjoint-column edits of one row, push, pull, compare.
import { connect } from "@tursodatabase/sync";
import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, rmSync } from "node:fs";

const [tursodb, dir, port] = process.argv.slice(2);
const url = `http://127.0.0.1:${port}`;
rmSync(dir, { recursive: true, force: true });
for (const d of ["server", "a", "b"]) mkdirSync(`${dir}/${d}`, { recursive: true });
const sdb = `${dir}/server/server.db`;
const sql = (q) => spawnSync(tursodb, ["-q", "-m", "list", sdb, q], { encoding: "utf8" });

let server;
const start = async () => { server = spawn(tursodb, ["--sync-server", `127.0.0.1:${port}`, sdb], { stdio: "ignore" }); await new Promise((r) => setTimeout(r, 700)); };
const stop = async () => { server.kill("SIGTERM"); await new Promise((r) => server.on("exit", r)); };
const rows = async (db, label) => console.log(`  ${label}:`, JSON.stringify(await (await db.prepare("SELECT id,title,status,seq,origin FROM issues ORDER BY id")).all()));
const step = async (label, fn) => { try { await fn(); console.log(`  ${label}: ok`); } catch (e) { console.log(`  ${label}: ERROR ${e.message.split("\n")[0]}`); } };
// Raw Hrana JSON to the sync server's /v2/pipeline, executed on the server's own connection (no credentials sent).
const pipeline = async (stmts) => {
  const res = await fetch(`${url}/v2/pipeline`, { method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ requests: stmts.map((s) => ({ type: "execute", stmt: { sql: s } })) }) });
  return { status: res.status, body: (await res.text()).slice(0, 400) };
};

await start();
console.log("server DDL via /v2/pipeline:", JSON.stringify(await pipeline([
  `CREATE TABLE issues(id TEXT PRIMARY KEY, title TEXT, status TEXT, seq INTEGER, origin TEXT)`,
  `CREATE TRIGGER assign_seq AFTER INSERT ON issues WHEN NEW.seq IS NULL BEGIN UPDATE issues SET seq = (SELECT coalesce(max(seq),0)+1 FROM issues) WHERE id = NEW.id; END`,
  `INSERT INTO issues(id,title,status,origin) VALUES ('i1','first','open','server')`,
  `SELECT id, seq FROM issues`,
])));
const a = await connect({ path: `${dir}/a/local.db`, url, clientName: "A" });
const b = await connect({ path: `${dir}/b/local.db`, url, clientName: "B" });
console.log("  B schema:", JSON.stringify(await (await b.prepare("SELECT type,name FROM sqlite_schema WHERE type='trigger'")).all()));
await rows(a, "A after bootstrap");

console.log("offline writes");
await stop();
await a.exec(`INSERT INTO issues(id,title,status,origin) VALUES ('iA1','A one','open','A')`);
await a.exec(`INSERT INTO issues(id,title,status,origin) VALUES ('iA2','A two','open','A')`);
await b.exec(`INSERT INTO issues(id,title,status,origin) VALUES ('iB1','B one','open','B')`);
await a.exec(`UPDATE issues SET status='closed-by-A' WHERE id='i1'`);
await b.exec(`UPDATE issues SET title='retitled-by-B' WHERE id='i1'`);
await rows(a, "A local"); await rows(b, "B local");

await start();
if (process.env.B_FIRST) {
  await step("B push", () => b.push());
  await step("A push", () => a.push());
} else {
  await step("A push", () => a.push());
  await step("B push", () => b.push());
}
await step("A pull", () => a.pull());
await step("B pull", () => b.pull());
await rows(a, "A final"); await rows(b, "B final");
await a.close(); await b.close(); await stop();
console.log("server final:\n" + sql("SELECT id,title,status,seq,origin FROM issues ORDER BY id").stdout.trim());
