-- Seed mock requests into the dev proxy database.
--
-- Usage:  make dev-seed-requests
--         make dev-seed-requests SEED_COUNT=500
--
-- Inserts randomized rows spanning the last 7 days with realistic
-- provider/model/status distributions.  Safe to run multiple times
-- (rows accumulate; use 'make dev-reset' to wipe).

-- Default row count; override with:  sqlite3 dev/proxy.db ".read scripts/seed-dev-requests.sql"
-- The makefile passes SEED_COUNT via a parameter substitution below.

.param set :count 200

WITH providers(name) AS (
    VALUES ('anthropic'), ('zai'), ('openai')
),
models(name) AS (
    VALUES ('claude-3-5-sonnet-20241022'),
           ('claude-3-5-haiku-20241022'),
           ('glm-4.6'),
           ('gpt-4o'),
           ('gpt-4o-mini')
),
statuses(val) AS (
    -- 80% completed, 20% errored
    VALUES ('completed'), ('completed'), ('completed'), ('completed'), ('errored')
),
directions(val) AS (
    -- ~1/3 each: passthrough (NULL), A->O, O->A
    VALUES (NULL), ('anthropic→openai'), ('openai→anthropic')
)
INSERT INTO requests (
    id, user_id, provider, model, status,
    started_at, finished_at,
    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
    cost_usd, translation_direction
)
SELECT
    lower(hex(randomblob(16))),
    1,
    (SELECT name FROM providers ORDER BY random() LIMIT 1),
    (SELECT name FROM models  ORDER BY random() LIMIT 1),
    (SELECT val  FROM statuses ORDER BY random() LIMIT 1),
    -- started_at: random time in the last 7 days (ms epoch)
    (strftime('%s', 'now') * 1000) - (abs(random()) % (86400000 * 7)),
    -- finished_at: 0.5–3 seconds after start
    (strftime('%s', 'now') * 1000) - (abs(random()) % (86400000 * 7))
        + (abs(random()) % 2500 + 500),
    -- tokens
    abs(random() % 100000) + 100,
    abs(random() % 10000) + 50,
    abs(random() % 20000),
    abs(random() % 5000),
    -- cost_usd: $0.001 – $10.0
    abs(random() % 10000000) / 1000000.0,
    (SELECT val FROM directions ORDER BY random() LIMIT 1)
FROM (
    WITH RECURSIVE cnt(x) AS (
        SELECT 1 UNION ALL SELECT x + 1 FROM cnt WHERE x < :count
    )
    SELECT x FROM cnt
);
