-- Stripe retries deliveries: each event is applied once, keyed by its id.
CREATE TABLE processed_events (
    event_id     TEXT PRIMARY KEY,
    kind         TEXT NOT NULL,
    processed_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Rows older than Stripe's 30-day retry window are pruned nightly.
CREATE INDEX processed_events_processed_at ON processed_events (processed_at);
