CREATE TABLE users (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    display_name text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE workflows (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name text NOT NULL,
    document json NOT NULL,
    document_hash bytea NOT NULL CHECK (length(document_hash) = 32),
    version integer NOT NULL DEFAULT 1,
    updated_via text NOT NULL CHECK (updated_via IN ('ui', 'mcp', 'api')),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX workflows_by_owner ON workflows (owner_id, updated_at DESC, id DESC);

CREATE TABLE approvals (
    workflow_id uuid NOT NULL REFERENCES workflows (id) ON DELETE CASCADE,
    document_hash bytea NOT NULL,
    approved_by uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    approved_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workflow_id, document_hash)
);

CREATE TABLE node_definitions (
    owner_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    type_name text NOT NULL,
    definition json NOT NULL,
    content_hash bytea NOT NULL CHECK (length(content_hash) = 32),
    version integer NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (owner_id, type_name)
);

CREATE TABLE secrets (
    owner_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name text NOT NULL,
    nonce bytea NOT NULL CHECK (length(nonce) = 12),
    ciphertext bytea NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (owner_id, name)
);

CREATE TABLE runs (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    workflow_id uuid REFERENCES workflows (id) ON DELETE SET NULL,
    workflow_name text NOT NULL,
    snapshot json NOT NULL,
    document_hash bytea NOT NULL CHECK (length(document_hash) = 32),
    inputs jsonb NOT NULL DEFAULT '{}',
    status text NOT NULL DEFAULT 'queued'
        CHECK (status IN ('queued', 'running', 'succeeded', 'failed', 'cancelled')),
    trigger text NOT NULL CHECK (trigger IN ('ui', 'mcp', 'api')),
    attempts integer NOT NULL DEFAULT 0,
    claimed_by uuid,
    heartbeat_at timestamptz,
    cancel_requested boolean NOT NULL DEFAULT false,
    outputs jsonb NOT NULL DEFAULT '{}',
    errors jsonb NOT NULL DEFAULT '[]',
    created_at timestamptz NOT NULL DEFAULT now(),
    started_at timestamptz,
    finished_at timestamptz
);

CREATE INDEX runs_queued ON runs (created_at, id) WHERE status = 'queued';
CREATE INDEX runs_running ON runs (heartbeat_at) WHERE status = 'running';
CREATE INDEX runs_by_owner ON runs (owner_id, created_at DESC, id DESC);
CREATE INDEX runs_by_workflow ON runs (workflow_id, created_at DESC, id DESC);

CREATE SEQUENCE run_event_seq;

CREATE TABLE run_steps (
    run_id uuid NOT NULL REFERENCES runs (id) ON DELETE CASCADE,
    path text NOT NULL,
    status text NOT NULL
        CHECK (status IN ('running', 'retrying', 'succeeded', 'failed', 'skipped', 'cancelled')),
    attempt integer NOT NULL,
    message text,
    outputs jsonb NOT NULL DEFAULT '{}',
    duration_ms bigint,
    seq bigint NOT NULL DEFAULT nextval('run_event_seq'),
    PRIMARY KEY (run_id, path)
);

CREATE INDEX run_steps_by_seq ON run_steps (run_id, seq);

CREATE TABLE run_logs (
    seq bigint PRIMARY KEY DEFAULT nextval('run_event_seq'),
    run_id uuid NOT NULL REFERENCES runs (id) ON DELETE CASCADE,
    path text NOT NULL,
    message text NOT NULL,
    at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX run_logs_by_run ON run_logs (run_id, seq);
