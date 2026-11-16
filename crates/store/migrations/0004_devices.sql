ALTER TABLE login_tokens ADD COLUMN next text;

CREATE TABLE device_codes (
    device_hash bytea PRIMARY KEY CHECK (length(device_hash) = 32),
    user_code text NOT NULL UNIQUE,
    name text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    approved_by uuid REFERENCES users (id) ON DELETE CASCADE,
    consumed_at timestamptz
);

CREATE INDEX device_codes_expiry ON device_codes (expires_at);
