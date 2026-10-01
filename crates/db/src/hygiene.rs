//! Warehouse + event/app hygiene (daily beat).
//!
//! Django reference:
//! - `saleor/warehouse/tasks.py::update_stocks_quantity_allocated_task` —
//!   recompute `stock.quantity_allocated` from live allocations where it
//!   drifted (drift → oversell display; the write path is otherwise trusted).
//! - `delete_empty_allocations_task` — drop zero-quantity allocations.
//! - `saleor/core/tasks.py::delete_event_payloads_task` — delete payloads
//!   unreferenced by deliveries newer than `EVENT_PAYLOAD_DELETE_PERIOD`
//!   (14d default), batched.
//! - `saleor/app/tasks.py::remove_apps_task` — apps past `DELETE_APP_TTL`
//!   (1d default) go away with their webhooks/deliveries/attempts/payloads
//!   (batched per app upstream; single pass here — app counts are tiny).

use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

use crate::{DbError, Result};

/// Fix drifted `quantity_allocated`; returns stocks corrected.
pub async fn recompute_stock_allocated(db: &impl ConnectionTrait) -> Result<u64> {
    let n = db
        .execute(Statement::from_string(
            DatabaseBackend::Postgres,
            "UPDATE warehouse_stock s SET quantity_allocated = COALESCE(a.total, 0) \
             FROM (SELECT stock_id, SUM(quantity_allocated) AS total FROM warehouse_allocation GROUP BY stock_id) a \
             WHERE a.stock_id = s.id AND s.quantity_allocated IS DISTINCT FROM COALESCE(a.total, 0)".to_string(),
        ))
        .await
        .map_err(DbError::SeaOrm)?
        .rows_affected();
    // Stocks whose allocations ALL vanished keep a stale positive count —
    // the join above misses them (no group row).
    let m = db
        .execute(Statement::from_string(
            DatabaseBackend::Postgres,
            "UPDATE warehouse_stock SET quantity_allocated = 0 WHERE quantity_allocated <> 0 \
             AND NOT EXISTS (SELECT 1 FROM warehouse_allocation WHERE stock_id = warehouse_stock.id)".to_string(),
        ))
        .await
        .map_err(DbError::SeaOrm)?
        .rows_affected();
    Ok(n + m)
}

/// Drop zero-quantity allocations; returns rows deleted.
pub async fn delete_empty_allocations(db: &impl ConnectionTrait) -> Result<u64> {
    let n = db
        .execute(Statement::from_string(
            DatabaseBackend::Postgres,
            "DELETE FROM warehouse_allocation WHERE quantity_allocated = 0".to_string(),
        ))
        .await
        .map_err(DbError::SeaOrm)?
        .rows_affected();
    Ok(n)
}

/// Delete event payloads unreferenced by recent deliveries; returns rows.
/// Period mirrors `EVENT_PAYLOAD_DELETE_PERIOD` (14 days).
pub async fn prune_event_payloads(db: &impl ConnectionTrait) -> Result<u64> {
    let n = db
        .execute(Statement::from_string(
            DatabaseBackend::Postgres,
            "DELETE FROM core_eventpayload WHERE id IN \
             (SELECT p.id FROM core_eventpayload p \
              WHERE NOT EXISTS (SELECT 1 FROM core_eventdelivery d \
                                WHERE d.payload_id = p.id AND d.created_at > now() - interval '14 days') \
              ORDER BY p.id DESC LIMIT 500)".to_string(),
        ))
        .await
        .map_err(DbError::SeaOrm)?
        .rows_affected();
    Ok(n)
}

/// Remove apps past the delete TTL with their delivery tree; returns apps.
/// TTL mirrors `DELETE_APP_TTL` (1 day).
pub async fn remove_deleted_apps(db: &sea_orm::DatabaseConnection) -> Result<u64> {
    let ids: Vec<i32> = db
        .query_all(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT id FROM app_app WHERE removed_at IS NOT NULL AND removed_at < now() - interval '1 day' LIMIT 100".to_string(),
        ))
        .await
        .map_err(DbError::SeaOrm)?
        .into_iter()
        .filter_map(|r| r.try_get::<i32>("", "id").ok())
        .collect();
    let mut done = 0u64;
    for id in ids {
        let txn = db.begin().await.map_err(DbError::SeaOrm)?;
        use sea_orm::TransactionTrait;
        // Attempts → deliveries → payloads (payload rows may be shared;
        // delete only orphans), then webhooks, tokens, perms, app.
        txn.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "DELETE FROM core_eventdeliveryattempt WHERE delivery_id IN \
             (SELECT d.id FROM core_eventdelivery d JOIN webhook_webhook w ON w.id = d.webhook_id WHERE w.app_id = $1)",
            vec![id.into()],
        ))
        .await
        .map_err(DbError::SeaOrm)?;
        txn.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "DELETE FROM core_eventdelivery WHERE webhook_id IN (SELECT id FROM webhook_webhook WHERE app_id = $1)",
            vec![id.into()],
        ))
        .await
        .map_err(DbError::SeaOrm)?;
        txn.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "DELETE FROM core_eventpayload p WHERE NOT EXISTS (SELECT 1 FROM core_eventdelivery d WHERE d.payload_id = p.id)",
            vec![],
        ))
        .await
        .map_err(DbError::SeaOrm)?;
        txn.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "DELETE FROM webhook_webhook WHERE app_id = $1",
            vec![id.into()],
        ))
        .await
        .map_err(DbError::SeaOrm)?;
        crate::apps::delete_app(&txn, id).await?;
        txn.commit().await.map_err(DbError::SeaOrm)?;
        done += 1;
    }
    Ok(done)
}
