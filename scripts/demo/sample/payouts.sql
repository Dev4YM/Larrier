CREATE TABLE payouts (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id    UUID NOT NULL REFERENCES accounts (id),
    amount_cents  BIGINT NOT NULL CHECK (amount_cents > 0),
    currency      CHAR(3) NOT NULL,
    status        TEXT NOT NULL DEFAULT 'pending',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX payouts_account_created ON payouts (account_id, created_at DESC);
