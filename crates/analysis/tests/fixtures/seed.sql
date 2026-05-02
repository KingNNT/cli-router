CREATE TABLE session (
  id TEXT PRIMARY KEY,
  directory TEXT NOT NULL
);

CREATE TABLE message (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  data TEXT NOT NULL,
  FOREIGN KEY (session_id) REFERENCES session(id)
);

INSERT INTO session (id, directory) VALUES
  ('s1', '/work/alpha'),
  ('s2', '/work/beta');

-- Two assistant messages on 2026-04-23 using opus-4.7 in session s1
INSERT INTO message (id, session_id, data) VALUES
  ('m1', 's1', json_object(
    'role', 'assistant',
    'modelID', 'anthropic/claude-opus-4-7',
    'providerID', 'anthropic',
    'time', json_object('created', 1777622400000),
    'tokens', json_object(
      'input', 1000, 'output', 500, 'reasoning', 0,
      'cache', json_object('read', 200, 'write', 100)
    ),
    'cost', 0.75
  )),
  ('m2', 's1', json_object(
    'role', 'assistant',
    'modelID', 'anthropic/claude-opus-4-7',
    'providerID', 'anthropic',
    'time', json_object('created', 1777622500000),
    'tokens', json_object(
      'input', 2000, 'output', 1000, 'reasoning', 0,
      'cache', json_object('read', 400, 'write', 200)
    ),
    'cost', 1.50
  )),
  -- One assistant message on 2026-04-22 using sonnet in session s2
  ('m3', 's2', json_object(
    'role', 'assistant',
    'modelID', 'anthropic/claude-sonnet-4-6',
    'providerID', 'anthropic',
    'time', json_object('created', 1777536000000),
    'tokens', json_object(
      'input', 500, 'output', 300, 'reasoning', 0,
      'cache', json_object('read', 0, 'write', 0)
    ),
    'cost', 0.20
  )),
  -- One user message (should be filtered out by role predicate)
  ('m4', 's1', json_object(
    'role', 'user',
    'time', json_object('created', 1777622400000)
  ));
