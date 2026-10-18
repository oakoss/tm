PRAGMA foreign_keys=OFF;
BEGIN TRANSACTION;
CREATE TABLE meta (
  k TEXT PRIMARY KEY,
  v ANY
) STRICT;
INSERT INTO meta VALUES('format',1);
INSERT INTO meta VALUES('replica_id',x'01a1500a3dbf729a891801ee42288d41');
INSERT INTO meta VALUES('last_hlc',117463070204624896);
CREATE TABLE ops (
  seq INTEGER PRIMARY KEY,
  op_id BLOB NOT NULL UNIQUE,
  op_hash BLOB NOT NULL,
  v INTEGER NOT NULL,
  kind TEXT NOT NULL,
  replica_id BLOB NOT NULL,
  actor TEXT NOT NULL,
  project TEXT NOT NULL,
  issue_id BLOB NOT NULL,
  field TEXT,
  elem TEXT,
  hlc INTEGER NOT NULL,
  base_seq INTEGER NOT NULL,
  utc_ms INTEGER NOT NULL,
  offset_min INTEGER NOT NULL,
  payload TEXT NOT NULL,
  server_seq INTEGER,
  server_received_ms INTEGER,
  assigned_key TEXT
) STRICT;
INSERT INTO ops VALUES(1,x'01a1500a3dc373e2b2ed74dbf0769805',x'1483fb6e80782554c615e59401ace8a43198f559e1f37db9065306fc0d868804',1,'create_project',x'01a1500a3dbf729a891801ee42288d41','jace','01a1500a-3dc0-72af-b375-8efad705dd47',x'01a1500a3dc072afb3758efad705dd47',NULL,NULL,117463070204428288,0,1792344210883,-300,'{"kind":"create_project","key":"CPL","name":"Core"}',NULL,NULL,NULL);
INSERT INTO ops VALUES(2,x'01a1500a3dc475e896684ffa79aa774b',x'219d7c025b6153918e0d388e72965b4aa438aacf4fee65f79f442ab1a52478d7',1,'create',x'01a1500a3dbf729a891801ee42288d41','jace','01a1500a-3dc0-72af-b375-8efad705dd47',x'01a1500a3dc475e896684ff985119b7b',NULL,NULL,117463070204493824,0,1792344210884,-300,'{"kind":"create"}',NULL,NULL,'CPL-1');
INSERT INTO ops VALUES(3,x'01a1500a3dc475e896684ffb4de0d9a3',x'61cb4e73b4406a8bac65d0ca5526b9c30c4fbbc6902fd51a24e7a3eab714fe6b',1,'set_field',x'01a1500a3dbf729a891801ee42288d41','jace','01a1500a-3dc0-72af-b375-8efad705dd47',x'01a1500a3dc475e896684ff985119b7b','type',NULL,117463070204493825,0,1792344210884,-300,'{"kind":"set_field","field":"type","label":"Type","value":"task","value_label":"Task"}',NULL,NULL,NULL);
INSERT INTO ops VALUES(4,x'01a1500a3dc475e896684ffc79097229',x'9fad3ced865ae8eeb556a52378879e98e640e4677ea2bf672ef8bd4710697cd3',1,'set_field',x'01a1500a3dbf729a891801ee42288d41','jace','01a1500a-3dc0-72af-b375-8efad705dd47',x'01a1500a3dc475e896684ff985119b7b','title',NULL,117463070204493826,0,1792344210884,-300,'{"kind":"set_field","field":"title","label":"Title","value":"first"}',NULL,NULL,NULL);
INSERT INTO ops VALUES(5,x'01a1500a3dc475e896684ffd323620e6',x'a042244f410b9672fee4ebe0b3bea3589b403e295edb14c55092f0aba2c2cbd6',1,'set_field',x'01a1500a3dbf729a891801ee42288d41','jace','01a1500a-3dc0-72af-b375-8efad705dd47',x'01a1500a3dc475e896684ff985119b7b','status',NULL,117463070204493827,0,1792344210884,-300,'{"kind":"set_field","field":"status","label":"Status","value":"todo","value_label":"Todo"}',NULL,NULL,NULL);
INSERT INTO ops VALUES(6,x'01a1500a3dc475e896684fff0a4be664',x'79f31e7fd69de6905e8f22dd95ce624ad7af4cf140bb55c1347bc9688c290426',1,'create',x'01a1500a3dbf729a891801ee42288d41','jace','01a1500a-3dc0-72af-b375-8efad705dd47',x'01a1500a3dc475e896684ffed46ed52d',NULL,NULL,117463070204493828,0,1792344210884,-300,'{"kind":"create"}',NULL,NULL,'CPL-2');
INSERT INTO ops VALUES(7,x'01a1500a3dc475e8966850001c0b9426',x'd551cf28cbb30d9f49509ee045607c9a36bf66917bedf4c28f9c16f3a4689837',1,'set_field',x'01a1500a3dbf729a891801ee42288d41','jace','01a1500a-3dc0-72af-b375-8efad705dd47',x'01a1500a3dc475e896684ffed46ed52d','type',NULL,117463070204493829,0,1792344210884,-300,'{"kind":"set_field","field":"type","label":"Type","value":"bug","value_label":"Bug"}',NULL,NULL,NULL);
INSERT INTO ops VALUES(8,x'01a1500a3dc5756e83442cf11f39b062',x'9fbe74ce055a36ed08b66846bb8f7d6c41b3b27c749c0c8aeb6d640d580843df',1,'set_field',x'01a1500a3dbf729a891801ee42288d41','jace','01a1500a-3dc0-72af-b375-8efad705dd47',x'01a1500a3dc475e896684ffed46ed52d','title',NULL,117463070204493830,0,1792344210884,-300,'{"kind":"set_field","field":"title","label":"Title","value":"second"}',NULL,NULL,NULL);
INSERT INTO ops VALUES(9,x'01a1500a3dc5756e83442cf2a66efb33',x'957c4bef87441ca58c435a828d4b8129e68c1bd319fe867c1413928f766e4575',1,'set_field',x'01a1500a3dbf729a891801ee42288d41','jace','01a1500a-3dc0-72af-b375-8efad705dd47',x'01a1500a3dc475e896684ffed46ed52d','status',NULL,117463070204493831,0,1792344210884,-300,'{"kind":"set_field","field":"status","label":"Status","value":"todo","value_label":"Todo"}',NULL,NULL,NULL);
INSERT INTO ops VALUES(10,x'01a1500a3dc5756e83442cf3bbb497d3',x'a275800d6f0ff6668f313bdc300b70f0701a55b80b4bdacc2ace4f7b9fbec490',1,'link',x'01a1500a3dbf729a891801ee42288d41','jace','01a1500a-3dc0-72af-b375-8efad705dd47',x'01a1500a3dc475e896684ff985119b7b','links','blocks/01a1500a-3dc4-75e8-9668-4ffed46ed52d',117463070204559360,0,1792344210885,-300,'{"kind":"link","link_type":"blocks","target":"01a1500a-3dc4-75e8-9668-4ffed46ed52d","present":true}',NULL,NULL,NULL);
INSERT INTO ops VALUES(11,x'01a1500a3dc5756e83442cf4a7a760f0',x'a4c2bb5e44aae695aafd2e6734628e9e747d80dbb28ca6998f8b9ad1e0ffa5eb',1,'claim',x'01a1500a3dbf729a891801ee42288d41','jace','01a1500a-3dc0-72af-b375-8efad705dd47',x'01a1500a3dc475e896684ff985119b7b','claim',NULL,117463070204559361,0,1792344210885,-300,'{"kind":"claim","action":{"action":"take","agent":"claude","ttl_ms":7200000}}',NULL,NULL,NULL);
INSERT INTO ops VALUES(12,x'01a1500a3dc5756e83442cf52ae8910b',x'46742840ee805c7117414d2acc4d891873b0188a6ddd498cdcb369cadcf4ca7a',1,'comment',x'01a1500a3dbf729a891801ee42288d41','jace','01a1500a-3dc0-72af-b375-8efad705dd47',x'01a1500a3dc475e896684ff985119b7b',NULL,NULL,117463070204559362,0,1792344210885,-300,'{"kind":"comment","body":"a comment"}',NULL,NULL,NULL);
INSERT INTO ops VALUES(13,x'01a1500a3dc671cf80de2b5dfbd97c23',x'afce8a2b2154042e1c52405d29b586ccad36478ba740d62f58c753f5c5131666',1,'add',x'01a1500a3dbf729a891801ee42288d41','jace','01a1500a-3dc0-72af-b375-8efad705dd47',x'01a1500a3dc475e896684ff985119b7b','labels','core',117463070204624896,0,1792344210886,-300,'{"kind":"add","field":"labels","label":"Labels","elem":"core"}',NULL,NULL,NULL);
CREATE TRIGGER ops_immutable
BEFORE UPDATE OF op_id, op_hash, v, kind, replica_id, actor, project, issue_id, field, elem,
                 hlc, base_seq, utc_ms, offset_min, payload ON ops
BEGIN
  SELECT RAISE(ABORT, 'stored operations are immutable');
END;
CREATE TRIGGER ops_position_frozen
BEFORE UPDATE ON ops WHEN NEW.seq IS NOT OLD.seq
BEGIN
  SELECT RAISE(ABORT, 'stored operations are immutable');
END;
CREATE TRIGGER ops_append_only
BEFORE DELETE ON ops
BEGIN
  SELECT RAISE(ABORT, 'the operation log is append-only');
END;
CREATE TRIGGER ops_effects_set_once
BEFORE UPDATE OF server_seq, server_received_ms, assigned_key ON ops
WHEN (OLD.server_seq IS NOT NULL AND NEW.server_seq IS NOT OLD.server_seq)
  OR (OLD.server_received_ms IS NOT NULL
      AND NEW.server_received_ms IS NOT OLD.server_received_ms)
  OR (OLD.assigned_key IS NOT NULL AND NEW.assigned_key IS NOT OLD.assigned_key)
BEGIN
  SELECT RAISE(ABORT, 'server effects are set once');
END;
CREATE TRIGGER ops_key_on_create_insert
BEFORE INSERT ON ops
WHEN NEW.assigned_key IS NOT NULL AND NEW.kind <> 'create'
BEGIN
  SELECT RAISE(ABORT, 'only a create operation is given a Key');
END;
CREATE TRIGGER ops_key_on_create_update
BEFORE UPDATE OF assigned_key ON ops
WHEN NEW.assigned_key IS NOT NULL AND NEW.kind <> 'create'
BEGIN
  SELECT RAISE(ABORT, 'only a create operation is given a Key');
END;
CREATE INDEX ops_target ON ops (issue_id, field, elem);
CREATE UNIQUE INDEX ops_server_position ON ops (project, server_seq);
COMMIT;
