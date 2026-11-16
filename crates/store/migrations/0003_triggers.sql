ALTER TABLE runs DROP CONSTRAINT runs_trigger_check;
ALTER TABLE runs ADD CONSTRAINT runs_trigger_check
    CHECK (trigger IN ('ui', 'mcp', 'api', 'webhook', 'schedule'));

CREATE TABLE triggers (
    id uuid PRIMARY KEY,
    owner_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    workflow_id uuid NOT NULL REFERENCES workflows (id) ON DELETE CASCADE,
    name text NOT NULL,
    kind text NOT NULL CHECK (kind IN ('webhook', 'schedule')),
    token_hash bytea UNIQUE CHECK (length(token_hash) = 32),
    secret_nonce bytea,
    secret_ciphertext bytea,
    cron text,
    next_run_at timestamptz,
    inputs jsonb NOT NULL DEFAULT '{}',
    enabled boolean NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now(),
    last_fired_at timestamptz,
    last_error text,
    CHECK ((kind = 'webhook') = (token_hash IS NOT NULL)),
    CHECK ((kind = 'schedule') = (cron IS NOT NULL))
);

CREATE INDEX triggers_by_workflow ON triggers (owner_id, workflow_id, created_at);
CREATE INDEX triggers_due ON triggers (next_run_at) WHERE kind = 'schedule' AND enabled;
