use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use bytes::Bytes;

use crate::{ledger, AppState};

use super::signature::verify;

/// Stripe calls this for every subscribed event, and retries with
/// exponential backoff until it gets a 2xx.
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

    match event.kind.as_str() {
        "payment_intent.succeeded" => {
            let payment = event.data.object.into_payment();
            if let Err(err) = ledger::apply_payment(&state.db, &payment).await {
                tracing::error!(%err, event = %event.id, "failed to apply payment");
                return StatusCode::INTERNAL_SERVER_ERROR;
            }
        }
        "charge.refunded" => {
            let refund = event.data.object.into_refund();
            if let Err(err) = ledger::apply_refund(&state.db, &refund).await {
                tracing::error!(%err, event = %event.id, "failed to apply refund");
                return StatusCode::INTERNAL_SERVER_ERROR;
            }
        }
        other => tracing::debug!(kind = other, "ignored webhook"),
    }
    StatusCode::OK
}
