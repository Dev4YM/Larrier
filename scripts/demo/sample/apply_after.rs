use sqlx::PgConnection;

use crate::payments::{Payment, Refund};

/// Credits the merchant's balance and records the ledger entry, inside the
/// caller's transaction (see `webhooks::stripe::apply_once`).
pub async fn apply_payment(tx: &mut PgConnection, payment: &Payment) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO ledger_entries (account_id, amount_cents, currency, source) VALUES ($1, $2, $3, 'payment')",
        payment.account_id,
        payment.amount_cents,
        payment.currency,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!("UPDATE balances SET available_cents = available_cents + $2 WHERE account_id = $1", payment.account_id, payment.amount_cents)
        .execute(&mut *tx)
        .await?;
    Ok(())
}

/// Debits the refunded amount, inside the caller's transaction.
pub async fn apply_refund(tx: &mut PgConnection, refund: &Refund) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO ledger_entries (account_id, amount_cents, currency, source) VALUES ($1, -$2, $3, 'refund')",
        refund.account_id,
        refund.amount_cents,
        refund.currency,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!("UPDATE balances SET available_cents = available_cents - $2 WHERE account_id = $1", refund.account_id, refund.amount_cents)
        .execute(&mut *tx)
        .await?;
    Ok(())
}
