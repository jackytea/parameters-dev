ALTER TABLE users ADD COLUMN email text;
CREATE UNIQUE INDEX users_email ON users (lower(email));

CREATE TABLE identities (
    provider text NOT NULL,
    provider_user_id text NOT NULL,
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (provider, provider_user_id)
);

CREATE TABLE login_tokens (
    token_hash bytea PRIMARY KEY CHECK (length(token_hash) = 32),
    email text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    consumed_at timestamptz
);

CREATE INDEX login_tokens_expiry ON login_tokens (expires_at);

CREATE TABLE sessions (
    token_hash bytea PRIMARY KEY CHECK (length(token_hash) = 32),
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    last_seen_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL
);

CREATE INDEX sessions_by_user ON sessions (user_id);
CREATE INDEX sessions_expiry ON sessions (expires_at);

CREATE TABLE api_tokens (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name text NOT NULL,
    token_hash bytea NOT NULL UNIQUE CHECK (length(token_hash) = 32),
    scopes text[] NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz,
    last_used_at timestamptz,
    revoked_at timestamptz
);

CREATE INDEX api_tokens_by_user ON api_tokens (user_id, created_at DESC);
