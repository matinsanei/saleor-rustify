//! Order expiry + expired-order cleanup.
//!
//! Django reference (`saleor/order/tasks.py`, 5-min + daily beats):
//! - `expire_orders_task` → `_expire_orders`: UNCONFIRMED orders with NO
//!   transaction/payment, older than the channel's `expire_orders_after`
//!   (minutes) → `EXPIRED` + `expired_at`, voucher usage released,
//!   `ORDER_EXPIRED` event, stock deallocated.
//! - `delete_expired_orders_task`: EXPIRED orders older than the channel's
//!   `delete_expired_orders_after` (interval), still payment-free → deleted
//!   (Django collector cascades relations; our FKs are NoAction so children
//!   go first, same rows).
//!
//! Scope notes: the Custom API reconciler owns ZarinPal payment states, but
//! UNCONFIRMED-order expiry is a different lifecycle (no money moved yet) —
//! it is NOT covered there, so it lives here. Webhook fan-out for expiry
//! (`_call_expired_order_events`) is skipped: the outbox has no order-expired
//! emitter and dashboards poll order status.

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ConnectionTrait, DatabaseBackend, EntityTrait, Set, Statement,
    TransactionTrait,
};
use uuid::Uuid;

use crate::{DbError, Result};

const BATCH: u64 = 100;

/// Expire stale UNCONFIRMED orders; returns orders expired.
pub async fn expire_unconfirmed_orders(db: &sea_orm::DatabaseConnection) -> Result<usize> {
    let ids: Vec<Uuid> = db
        .query_all(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT o.id FROM order_order o \
             JOIN channel_channel ch ON ch.id = o.channel_id \
             WHERE o.status = 'unconfirmed' \
               AND NOT EXISTS (SELECT 1 FROM payment_transactionitem t WHERE t.order_id = o.id) \
               AND NOT EXISTS (SELECT 1 FROM payment_payment p WHERE p.order_id = o.id) \
               AND ch.expire_orders_after IS NOT NULL \
               AND ch.expire_orders_after > 0 \
               AND o.created_at < now() - (ch.expire_orders_after || ' minutes')::interval \
             ORDER BY o.created_at LIMIT $1",
            vec![BATCH.into()],
        ))
        .await
        .map_err(DbError::SeaOrm)?
        .into_iter()
        .filter_map(|r| r.try_get::<Uuid>("", "id").ok())
        .collect();
    let mut done = 0;
    for id in ids {
        let txn = db.begin().await.map_err(DbError::SeaOrm)?;
        use crate::entities::order_order::Entity as OEnt;
        let Some(o) = OEnt::find_by_id(id).one(&txn).await.map_err(DbError::SeaOrm)? else {
            continue;
        };
        if o.status != "unconfirmed" {
            continue; // raced a payment; leave it.
        }
        // 1. Status flip (Django's bulk update, per-order here for the
        //    status re-check above).
        txn.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE order_order SET status = 'expired', expired_at = now() WHERE id = $1",
            vec![id.into()],
        ))
        .await
        .map_err(DbError::SeaOrm)?;
        // 2. Voucher usage release (Django `_bulk_release_voucher_usage`:
        //    one use back per expired order, floored at zero — and only for
        //    vouchers WITH a usage limit, like upstream).
        if let Some(code) = o.voucher_code.clone() {
            txn.execute(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE discount_vouchercode c SET used = GREATEST(used - 1, 0) \
                 FROM discount_voucher v \
                 WHERE c.voucher_id = v.id AND v.usage_limit IS NOT NULL AND c.code = $1",
                vec![code.clone().into()],
            ))
            .await
            .map_err(DbError::SeaOrm)?;
            // Guest orders only (Django keeps rows tied to a user account).
            if o.user_id.is_none() {
                txn.execute(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    "DELETE FROM discount_vouchercustomer vc USING discount_vouchercode c \
                     WHERE vc.voucher_code_id = c.id AND c.code = $1 AND vc.customer_email = $2",
                    vec![code.into(), o.user_email.clone().into()],
                ))
                .await
                .map_err(DbError::SeaOrm)?;
            }
        }
        // 3. Stock deallocate (Django `deallocate_stock_for_orders`).
        txn.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "DELETE FROM warehouse_allocation WHERE order_line_id IN \
             (SELECT id FROM order_orderline WHERE order_id = $1)",
            vec![id.into()],
        ))
        .await
        .map_err(DbError::SeaOrm)?;
        // 4. Audit event (Django `_order_expired_events`, type "expired").
        use crate::entities::order_orderevent::ActiveModel as EvAm;
        use sea_orm::ActiveModelTrait;
        EvAm {
            date: Set(Utc::now().into()),
            r#type: Set("expired".to_string()),
            user_id: Set(None),
            parameters: Set(serde_json::json!({})),
            order_id: Set(id),
            ..Default::default()
        }
        .insert(&txn)
        .await
        .map_err(DbError::SeaOrm)?;
        txn.commit().await.map_err(DbError::SeaOrm)?;
        done += 1;
    }
    Ok(done)
}

/// Delete long-expired payment-free orders; returns orders deleted.
pub async fn delete_expired_orders(db: &sea_orm::DatabaseConnection) -> Result<usize> {
    let ids: Vec<Uuid> = db
        .query_all(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT o.id FROM order_order o \
             JOIN channel_channel ch ON ch.id = o.channel_id \
             WHERE o.status = 'expired' \
               AND o.expired_at IS NOT NULL \
               AND NOT EXISTS (SELECT 1 FROM payment_transactionitem t WHERE t.order_id = o.id) \
               AND NOT EXISTS (SELECT 1 FROM payment_payment p WHERE p.order_id = o.id) \
               AND ch.delete_expired_orders_after::interval > interval '0 seconds' \
               AND o.expired_at < now() - ch.delete_expired_orders_after::interval \
             ORDER BY o.expired_at LIMIT $1",
            vec![BATCH.into()],
        ))
        .await
        .map_err(DbError::SeaOrm)?
        .into_iter()
        .filter_map(|r| r.try_get::<Uuid>("", "id").ok())
        .collect();
    let mut done = 0;
    for id in ids {
        let txn = db.begin().await.map_err(DbError::SeaOrm)?;
        // NoAction FKs → children first (Django collector order).
        for (table, col) in [
            ("warehouse_allocation", "order_line_id IN (SELECT id FROM order_orderline WHERE order_id = $1)"),
            ("order_orderline", "order_id = $1"),
            ("order_orderevent", "order_id = $1"),
        ] {
            txn.execute(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                &format!("DELETE FROM {table} WHERE {col}"),
                vec![id.into()],
            ))
            .await
            .map_err(DbError::SeaOrm)?;
        }
        let n = txn
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "DELETE FROM order_order WHERE id = $1",
                vec![id.into()],
            ))
            .await
            .map_err(DbError::SeaOrm)?
            .rows_affected();
        txn.commit().await.map_err(DbError::SeaOrm)?;
        if n > 0 {
            done += 1;
        }
    }
    Ok(done)
}
