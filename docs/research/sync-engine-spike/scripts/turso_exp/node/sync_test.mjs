// usage: node sync_test.mjs <tursodb> <workdir> <port>
// Two replicas of one sync-server DB: bootstrap, go offline, write on both, reconnect, push/pull, compare.
import { connect } from "@tursodatabase/sync";
import { spawn } from "node:child_process";
import { mkdirSync, rmSync, readdirSync } from "node:fs";

const [tursodb, dir, port] = process.argv.slice(2);
const url = `http://127.0.0.1:${port}`;
rmSync(dir, { recursive: true, force: true });
for (const d of ["server", "a", "b"]) mkdirSync(`${dir}/${d}`, { recursive: true });

let server;
async function startServer() {
  server = spawn(tursodb, ["--sync-server", `127.0.0.1:${port}`, `${dir}/server/server.db`], { stdio: "ignore" });
  await new Promise((r) => setTimeout(r, 700));
}
async function stopServer() {
  server.kill("SIGTERM");
  await new Promise((r) => server.on("exit", r));
}
const rows = async (db, label) => {
  const r = await (await db.prepare("SELECT id, title, status, seq, origin FROM issues ORDER BY id")).all();
  console.log(`  ${label}:`, JSON.stringify(r));
};
const step = async (label, fn) => {
  try { const v = await fn(); console.log(`  ${label}: ok${v === undefined ? "" : " -> " + JSON.stringify(v)}`); }
  catch (e) { console.log(`  ${label}: ERROR ${e.message.split("\n")[0]}`); }
};

await startServer();
console.log("1. bootstrap");
const a = await connect({ path: `${dir}/a/local.db`, url, clientName: "A" });
await a.exec(`CREATE TABLE issues(id TEXT PRIMARY KEY, title TEXT, status TEXT, seq INTEGER, origin TEXT)`);
// Trigger meant to act as "server assigns sequence numbers"; it replicates to every replica with the schema.
await a.exec(`CREATE TRIGGER assign_seq AFTER INSERT ON issues WHEN NEW.seq IS NULL BEGIN
  UPDATE issues SET seq = (SELECT coalesce(max(seq),0)+1 FROM issues) WHERE id = NEW.id; END`);
await a.exec(`INSERT INTO issues(id,title,status,origin) VALUES ('i1','first','open','A')`);
await step("A push", () => a.push());
const b = await connect({ path: `${dir}/b/local.db`, url, clientName: "B" });
await step("B pull", () => b.pull());
await rows(a, "A"); await rows(b, "B");

console.log("2. server down; offline writes on both replicas");
await stopServer();
await a.exec(`UPDATE issues SET status='A-closed' WHERE id='i1'`);
await a.exec(`INSERT INTO issues(id,title,status,origin) VALUES ('iA','from A','open','A')`);
await b.exec(`UPDATE issues SET status='B-wip', title='first (edited by B)' WHERE id='i1'`);
await b.exec(`INSERT INTO issues(id,title,status,origin) VALUES ('iB','from B','open','B')`);
await step("A push while offline", () => a.push());
await rows(a, "A local"); await rows(b, "B local");
console.log("  A stats:", JSON.stringify(await a.stats()));

console.log("3. server back; A pushes first, then B, then both pull");
await startServer();
await step("A push", () => a.push());
await step("B push", () => b.push());
await step("A pull", () => a.pull());
await step("B pull", () => b.pull());
await rows(a, "A final"); await rows(b, "B final");
console.log("  B stats:", JSON.stringify(await b.stats()));
await a.close(); await b.close();
await stopServer();
console.log("  sidecars a/:", readdirSync(`${dir}/a`).join(" "));
console.log("  server/:", readdirSync(`${dir}/server`).join(" "));
const s = spawn(tursodb, ["-q", "-m", "list", `${dir}/server/server.db`, "SELECT id,title,status,seq,origin FROM issues ORDER BY id"], { stdio: ["ignore", "pipe", "pipe"] });
let out = ""; s.stdout.on("data", (d) => (out += d)); s.stderr.on("data", (d) => (out += d));
await new Promise((r) => s.on("exit", r));
console.log("  server final:\n" + out.trim().split("\n").map((l) => "    " + l).join("\n"));
