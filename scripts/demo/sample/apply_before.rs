use sqlx::PgPool;

use crate::payments::{Payment, Refund};

/// Credits the merchant's balance and records the ledger entry.
pub async fn apply_payment(db: &PgPool, payment: &Payment) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO ledger_entries (account_id, amount_cents, currency, source) VALUES ($1, $2, $3, 'payment')",
        payment.account_id,
        payment.amount_cents,
        payment.currency,
    )
    .execute(db)
    .await?;
    sqlx::query!("UPDATE balances SET available_cents = available_cents + $2 WHERE account_id = $1", payment.account_id, payment.amount_cents)
        .execute(db)
        .await?;
    Ok(())
}

/// Debits the refunded amount.
pub async fn apply_refund(db: &PgPool, refund: &Refund) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO ledger_entries (account_id, amount_cents, currency, source) VALUES ($1, -$2, $3, 'refund')",
        refund.account_id,
        refund.amount_cents,
        refund.currency,
    )
    .execute(db)
    .await?;
    sqlx::query!("UPDATE balances SET available_cents = available_cents - $2 WHERE account_id = $1", refund.account_id, refund.amount_cents)
        .execute(db)
        .await?;
    Ok(())
}
