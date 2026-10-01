//! Scheduled lifecycle transitions (daily beat).
//!
//! Django reference:
//! - `saleor/discount/tasks.py::handle_promotion_toggle` (+
//!   `get_starting/ending_promotions`): promos whose start/end crossed since
//!   the last notification → fan-out webhooks (skipped here: no emitter),
//!   mark products dirty, stamp `last_notification_scheduled_at`.
//!   REQUIRED for correctness of `promo_recalc`: future promos get their
//!   `variants_dirty` cleared while inactive, so without this tick a promo
//!   would never activate in M2M/listings when its start arrives.
//! - `saleor/giftcard/tasks.py::deactivate_expired_cards_task`: past-expiry
//!   active cards → `is_active=FALSE` (+ event, skipped: no giftcard-event
//!   emitter; the flag is what checkout enforcement reads).
//! - `saleor/product/tasks.py::deactivate_preorder_for_variants_task`:
//!   past-end preorder variants → flags cleared. The allocation-conversion
//!   half (`warehouse/management.py::deactivate_preorder_for_variant`) is
//!   N/A by construction — this engine never creates preorder allocations
//!   (nothing reads `is_preorder` in checkout), so there is nothing to
//!   convert; documented, not silent.

use sea_orm::{ConnectionTrait, DatabaseBackend, EntityTrait, Statement};
use uuid::Uuid;

use crate::{DbError, Result};

const BATCH: u64 = 100;

/// Promotion start/end transitions; returns (started, ended).
pub async fn promotion_toggle_tick(db: &sea_orm::DatabaseConnection) -> Result<(usize, usize)> {
    use crate::entities::discount_promotion::{Column as PCol, Entity as PEnt};
    use sea_orm::{ColumnTrait, QueryFilter, QuerySelect};
    // Starting: start passed and never (or pre-start) notified.
    let starting: Vec<Uuid> = PEnt::find()
        .select_only()
        .column(PCol::Id)
        .filter(PCol::StartDate.lte(chrono::Utc::now()))
        .filter(sea_orm::sea_query::Expr::cust(
            "last_notification_scheduled_at IS NULL OR last_notification_scheduled_at < start_date",
        ))
        .limit(BATCH)
        .into_tuple()
        .all(db)
        .await
        .map_err(DbError::SeaOrm)?;
    // Ending: end passed and never (or pre-end) notified.
    let ending: Vec<Uuid> = PEnt::find()
        .select_only()
        .column(PCol::Id)
        .filter(PCol::EndDate.is_not_null())
        .filter(PCol::EndDate.lte(chrono::Utc::now()))
        .filter(sea_orm::sea_query::Expr::cust(
            "last_notification_scheduled_at IS NULL OR last_notification_scheduled_at < end_date",
        ))
        .limit(BATCH)
        .into_tuple()
        .all(db)
        .await
        .map_err(DbError::SeaOrm)?;
    for id in &starting {
        crate::promo_recalc::mark_promotion_rules_dirty(db, *id).await?;
        stamp_notification(db, *id).await?;
    }
    for id in &ending {
        // Links must clear and prices restore → dirty both levels first.
        crate::promo_recalc::mark_promotion_products_dirty(db, *id).await?;
        crate::promo_recalc::mark_promotion_rules_dirty(db, *id).await?;
        stamp_notification(db, *id).await?;
    }
    Ok((starting.len(), ending.len()))
}

async fn stamp_notification(db: &impl ConnectionTrait, id: Uuid) -> Result<()> {
    use crate::entities::discount_promotion::{Column as PCol, Entity as PEnt};
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    PEnt::update_many()
        .col_expr(
            PCol::LastNotificationScheduledAt,
            sea_orm::sea_query::Expr::value(Some(chrono::Utc::now())),
        )
        .filter(PCol::Id.eq(id))
        .exec(db)
        .await
        .map_err(DbError::SeaOrm)?;
    Ok(())
}

/// Deactivate past-expiry gift cards; returns cards deactivated.
pub async fn deactivate_expired_gift_cards(db: &impl ConnectionTrait) -> Result<u64> {
    let n = db
        .execute(Statement::from_string(
            DatabaseBackend::Postgres,
            "UPDATE giftcard_giftcard SET is_active = FALSE WHERE is_active = TRUE AND expiry_date < CURRENT_DATE".to_string(),
        ))
        .await
        .map_err(DbError::SeaOrm)?
        .rows_affected();
    Ok(n)
}

/// Clear preorder flags past their end date; returns variants cleared.
/// (No allocation conversion — see module docs.)
pub async fn deactivate_past_preorders(db: &impl ConnectionTrait) -> Result<u64> {
    let n = db
        .execute(Statement::from_string(
            DatabaseBackend::Postgres,
            "UPDATE product_productvariant SET is_preorder = FALSE, preorder_end_date = NULL WHERE is_preorder = TRUE AND preorder_end_date < now()".to_string(),
        ))
        .await
        .map_err(DbError::SeaOrm)?
        .rows_affected();
    Ok(n)
}
