-- OpenCode v2 schema (session_v2 / session_message tables). Same usage as seed_v1.sql.
CREATE TABLE session_v2 (
  id TEXT PRIMARY KEY,
  directory TEXT NOT NULL
);

CREATE TABLE session_message (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  type TEXT NOT NULL,
  seq INTEGER NOT NULL,
  time_created INTEGER NOT NULL,
  time_updated INTEGER NOT NULL,
  data TEXT NOT NULL,
  FOREIGN KEY (session_id) REFERENCES session_v2(id)
);

INSERT INTO session_v2 (id, directory) VALUES
  ('s1', '/work/alpha'),
  ('s2', '/work/beta');

-- Two assistant messages on 2026-04-23 using opus-4.7 in session s1
INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data) VALUES
  ('m1', 's1', 'assistant', 2, 1777622400000, 1777622400000, json_object(
    'model', json_object('id', 'anthropic/claude-opus-4-7', 'providerID', 'anthropic'),
    'time', json_object('created', 1777622400000),
    'tokens', json_object(
      'input', 1000, 'output', 500, 'reasoning', 0,
      'cache', json_object('read', 200, 'write', 100)
    ),
    'cost', 0.75
  )),
  ('m2', 's1', 'assistant', 3, 1777622500000, 1777622500000, json_object(
    'model', json_object('id', 'anthropic/claude-opus-4-7', 'providerID', 'anthropic'),
    'time', json_object('created', 1777622500000),
    'tokens', json_object(
      'input', 2000, 'output', 1000, 'reasoning', 0,
      'cache', json_object('read', 400, 'write', 200)
    ),
    'cost', 1.50
  )),
  -- One assistant message on 2026-04-22 using sonnet in session s2
  ('m3', 's2', 'assistant', 2, 1777536000000, 1777536000000, json_object(
    'model', json_object('id', 'anthropic/claude-sonnet-4-6', 'providerID', 'anthropic'),
    'time', json_object('created', 1777536000000),
    'tokens', json_object(
      'input', 500, 'output', 300, 'reasoning', 0,
      'cache', json_object('read', 0, 'write', 0)
    ),
    'cost', 0.20
  )),
  -- User and idle messages (filtered out by the type predicate)
  ('m4', 's1', 'user', 1, 1777622400000, 1777622400000, json_object(
    'time', json_object('created', 1777622400000)
  )),
  ('m5', 's1', 'idle', 4, 1777622600000, 1777622600000, json_object(
    'time', json_object('created', 1777622600000)
  ));
