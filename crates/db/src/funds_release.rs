//! Abandoned-checkout fund release scan.
//!
//! Django reference (`saleor/payment/tasks.py:transaction_release_funds_for_checkout_task`,
//! 10-min beat): find transactions on TTL-expired auto-refundable checkouts
//! (authorized/charged, no order) and release them via the PSP gateway
//! (`request_cancelation_action` / `request_refund_action`).
//!
//! Deliberate scope cut — NO gateway calls here: this deployment's money
//! movement belongs to the Custom API (ZarinPal verify/capture + its own
//! reconciler); the Rust engine holds no ZarinPal adapter, and a second
//! actor releasing funds would double-act against the reconciler. This worker
//! is the DETECTOR: it runs Django's eligibility query verbatim and surfaces
//! every hit at WARN for ops/reconciler pickup. Silent skip would be the bug;
//! duplicate release would be worse.
//!
//! Eligibility (Django-verbatim, including the 365-day lookback, the channel
//! TTL/flag/cut-off gates and the `last_refund_success` guard). The channel
//! TTL (`checkout_ttl_before_releasing_funds`, default "6 hours") is cast to
//! a Postgres interval in-SQL, so all Django interval spellings work; a
//! malformed TTL fails THIS scan loudly (warn) instead of releasing wrongly.

use rust_decimal::Decimal;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

use crate::{DbError, Result};

/// Django `TRANSACTION_BATCH_FOR_RELEASING_FUNDS` default.
const BATCH: u64 = 60;

pub struct Releasable {
    pub transaction_id: i32,
    pub checkout_id: Option<uuid::Uuid>,
    pub authorized: Decimal,
    pub charged: Decimal,
    pub currency: String,
}

/// Django `transactions_to_release_funds()` as one query. Returns the batch;
/// callers decide (log/alert — never auto-release, see module docs).
pub async fn scan_releasable_funds(
    db: &impl ConnectionTrait,
) -> Result<Vec<Releasable>> {
    let rows = db
        .query_all(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT t.id, t.checkout_id, t.authorized_value, t.charged_value, t.currency \
             FROM payment_transactionitem t \
             JOIN checkout_checkout c ON c.token = t.checkout_id \
             JOIN channel_channel ch ON ch.id = c.channel_id \
             WHERE t.order_id IS NULL \
               AND t.last_refund_success = TRUE \
               AND (t.authorized_value > 0 OR t.charged_value > 0) \
               AND c.automatically_refundable = TRUE \
               AND c.authorize_status IN ('partial', 'full') \
               AND c.created_at > now() - interval '365 days' \
               AND ch.release_funds_for_expired_checkouts = TRUE \
               AND c.last_change < now() - ch.checkout_ttl_before_releasing_funds::interval \
               AND (c.last_transaction_modified_at IS NULL \
                    OR c.last_transaction_modified_at < now() - ch.checkout_ttl_before_releasing_funds::interval) \
               AND (ch.checkout_release_funds_cut_off_date IS NULL \
                    OR ch.checkout_release_funds_cut_off_date < c.created_at) \
             ORDER BY t.created_at LIMIT $1",
            vec![BATCH.into()],
        ))
        .await
        .map_err(DbError::SeaOrm)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(Releasable {
            transaction_id: r.try_get::<i32>("", "id").map_err(DbError::SeaOrm)?,
            checkout_id: r
                .try_get::<Option<uuid::Uuid>>("", "checkout_id")
                .map_err(DbError::SeaOrm)?,
            authorized: r
                .try_get::<Decimal>("", "authorized_value")
                .map_err(DbError::SeaOrm)?,
            charged: r
                .try_get::<Decimal>("", "charged_value")
                .map_err(DbError::SeaOrm)?,
            currency: r.try_get::<String>("", "currency").map_err(DbError::SeaOrm)?,
        });
    }
    Ok(out)
}
