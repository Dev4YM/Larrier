use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use bytes::Bytes;
use sqlx::PgPool;

use crate::{ledger, AppState};

use super::signature::{verify, Event};

/// Stripe calls this for every subscribed event, and retries with
/// exponential backoff until it gets a 2xx. A retry can arrive while the
/// first delivery is still running, so each event is applied once, keyed
/// by its id.
pub async fn handle(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> StatusCode {
    let Some(sig) = headers.get("stripe-signature").and_then(|v| v.to_str().ok()) else {
        return StatusCode::BAD_REQUEST;
    };
    let event = match verify(&body, sig, &state.config.stripe_webhook_secret) {
        Ok(event) => event,
        Err(err) => {
            tracing::warn!(%err, "rejected webhook");
            return StatusCode::BAD_REQUEST;
        }
    };
    match apply_once(&state.db, event).await {
        Ok(Applied::New) => StatusCode::OK,
        Ok(Applied::Duplicate) => {
            metrics::counter!("webhooks_duplicate_total").increment(1);
            StatusCode::OK
        }
        Err(err) => {
            tracing::error!(%err, "failed to apply webhook");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

pub enum Applied {
    New,
    Duplicate,
}

/// Claims the event id and applies the event in one transaction. A
/// concurrent delivery of the same event blocks on the row lock, then sees
/// the committed row and skips.
async fn apply_once(db: &PgPool, event: Event) -> sqlx::Result<Applied> {
    let mut tx = db.begin().await?;
    let claimed = sqlx::query!(
        "INSERT INTO processed_events (event_id, kind) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        event.id,
        event.kind,
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if claimed == 0 {
        return Ok(Applied::Duplicate);
    }
    match event.kind.as_str() {
        "payment_intent.succeeded" => ledger::apply_payment(&mut tx, &event.data.object.into_payment()).await?,
        "charge.refunded" => ledger::apply_refund(&mut tx, &event.data.object.into_refund()).await?,
        other => tracing::debug!(kind = other, "ignored webhook"),
    }
    tx.commit().await?;
    Ok(Applied::New)
}
