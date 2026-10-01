//! Beat workers for catalogue-promotion denormalization.
//!
//! Django reference (`saleor/product/tasks.py`, both on a ~30s beat):
//! - `update_variant_relations_for_active_promotion_rules_task` — for dirty
//!   rules of ACTIVE catalogue promos, recompute the rule↔variant M2M
//!   (`fetch_variants_for_promotion_rules`), mark affected products' channel
//!   listings dirty, clear `variants_dirty` (batched, re-queues while dirty
//!   rules remain).
//! - `recalculate_discounted_price_for_products_task` — for listings with
//!   `discounted_price_dirty`, recompute every variant listing's
//!   `discounted_price_amount` (best applicable rule), refresh the
//!   listing↔rule link rows, set the product listing to the min, clear the
//!   dirty flag, emit `PRODUCT_VARIANT_DISCOUNTED_PRICE_UPDATED`.
//!
//! Why this matters here (not just dashboard cosmetics): the storefront
//! pricing resolvers AND the Custom API materialized views read the
//! DENORMALIZED `discounted_price_amount` columns, and nothing else in this
//! codebase writes promo-adjusted values into them (`upsert_variant_listing`
//! stores `price`). Without these workers catalogue promotions are silently
//! invisible on every shelf.
//!
//! Honest deltas: no `PRODUCT_VARIANT_DISCOUNTED_PRICE_UPDATED` emission
//! (the webhook outbox has no price-update emitter; dashboards poll anyway);
//! per-rule transactions instead of one big atomic block (beat-safe, and a
//! crash leaves `variants_dirty` set so the next tick resumes).

use std::collections::{HashMap, HashSet};

use rust_decimal::Decimal;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set, Statement, TransactionTrait,
};

use crate::promotions;
use crate::{DbError, Result};

const RULE_BATCH: u64 = 100;
const LISTING_BATCH: u64 = 500;

/// Dirty rules of ACTIVE catalogue promos, oldest first (Django's
/// `order_by("id")[:PROMOTION_RULE_BATCH_SIZE]` + active-promotion filter).
async fn dirty_rules(
    db: &impl ConnectionTrait,
) -> Result<Vec<crate::entities::discount_promotionrule::Model>> {
    use crate::entities::discount_promotionrule::{Column as RCol, Entity as REnt};
    let now = chrono::Utc::now();
    let rules = REnt::find()
        .filter(RCol::VariantsDirty.eq(true))
        .order_by_asc(RCol::Id)
        .limit(RULE_BATCH)
        .all(db)
        .await
        .map_err(DbError::SeaOrm)?;
    let mut out = Vec::new();
    for r in rules {
        // Django's exclusion: null/zero reward or empty predicate never
        // match anything (and would poison the M2M with junk).
        let reward = r.reward_value.unwrap_or(Decimal::ZERO);
        let pred_empty = r
            .catalogue_predicate
            .as_object()
            .map(|o| o.is_empty())
            .unwrap_or(true);
        if reward <= Decimal::ZERO || pred_empty {
            // Still clear the flag — upstream would exclude these from the
            // batch but never revisit them either.
            let mut am: crate::entities::discount_promotionrule::ActiveModel = r.clone().into();
            am.variants_dirty = Set(Some(false));
            am.update(db).await.map_err(DbError::SeaOrm)?;
            continue;
        }
        // Parent must be an ACTIVE catalogue promotion.
        use crate::entities::discount_promotion::{Column as PCol, Entity as PEnt};
        let active = PEnt::find_by_id(r.promotion_id)
            .filter(PCol::Type.eq("catalogue"))
            .filter(PCol::StartDate.lte(now))
            .filter(PCol::EndDate.is_null().or(PCol::EndDate.gt(now)))
            .one(db)
            .await
            .map_err(DbError::SeaOrm)?
            .is_some();
        if active {
            out.push(r);
        } else {
            let mut am: crate::entities::discount_promotionrule::ActiveModel = r.clone().into();
            am.variants_dirty = Set(Some(false));
            am.update(db).await.map_err(DbError::SeaOrm)?;
        }
    }
    Ok(out)
}

/// Recompute rule↔variant links for dirty rules; returns rules processed.
/// Affected products' listings are marked `discounted_price_dirty` for the
/// price pass.
pub async fn refresh_rule_variant_links(db: &sea_orm::DatabaseConnection) -> Result<usize> {
    use crate::entities::discount_promotionrule_variants::{
        Column as MVCol, Entity as MVEnt,
    };
    use crate::entities::product_productvariant::{Column as VCol, Entity as VEnt};
    let rules = dirty_rules(db).await?;
    if rules.is_empty() {
        return Ok(0);
    }
    // All variants with their product (beat-scale table scan; variants are
    // in the low thousands per shop).
    let variants: Vec<(i32, i32)> = VEnt::find()
        .select_only()
        .column(VCol::Id)
        .column(VCol::ProductId)
        .into_tuple()
        .all(db)
        .await
        .map_err(DbError::SeaOrm)?;
    let mut ctx_cache: HashMap<i32, promotions::VariantContext> = HashMap::new();
    let mut done = 0;
    for r in rules {
        let rule_view = promotions::ActiveRule {
            rule_id: r.id,
            reward_value_type: r.reward_value_type.clone().unwrap_or_default(),
            reward_value: r.reward_value.unwrap_or(Decimal::ZERO),
            promotion_name: String::new(),
            rule_name: r.name.clone().unwrap_or_default(),
            predicate: r.catalogue_predicate.clone(),
            promotion_end: None,
            reward_type: r.reward_type.clone().unwrap_or_default(),
        };
        let mut wanted: HashSet<i32> = HashSet::new();
        for (vid, pid) in &variants {
            let ctx = match ctx_cache.get(pid) {
                Some(c) => c.clone(),
                None => {
                    let c = promotions::variant_context(db, *pid).await?;
                    ctx_cache.insert(*pid, c.clone());
                    c
                }
            };
            if promotions::rule_matches_variant(db, &rule_view, *vid, &ctx).await? {
                wanted.insert(*vid);
            }
        }
        let existing: HashSet<i32> = MVEnt::find()
            .select_only()
            .column(MVCol::ProductvariantId)
            .filter(MVCol::PromotionruleId.eq(r.id))
            .into_tuple()
            .all(db)
            .await
            .map_err(DbError::SeaOrm)?
            .into_iter()
            .collect();
        let txn = db.begin().await.map_err(DbError::SeaOrm)?;
        for vid in wanted.difference(&existing) {
            let am = crate::entities::discount_promotionrule_variants::ActiveModel {
                promotionrule_id: Set(r.id),
                productvariant_id: Set(*vid),
                ..Default::default()
            };
            // Concurrent beat ticks may race; ignore conflicts (Django uses
            // `ignore_conflicts` on the same insert path).
            let _ = am.insert(&txn).await;
        }
        for vid in existing.difference(&wanted) {
            MVEnt::delete_many()
                .filter(MVCol::PromotionruleId.eq(r.id))
                .filter(MVCol::ProductvariantId.eq(*vid))
                .exec(&txn)
                .await
                .map_err(DbError::SeaOrm)?;
        }
        // Products entering OR leaving the rule get their listings dirtied.
        let mut affected_pids: HashSet<i32> = HashSet::new();
        for vid in wanted.symmetric_difference(&existing) {
            if let Some((_, pid)) = variants.iter().find(|(v, _)| v == vid) {
                affected_pids.insert(*pid);
            }
        }
        if !affected_pids.is_empty() {
            let ids: Vec<sea_orm::Value> = affected_pids.into_iter().map(|i| i.into()).collect();
            let list = (1..=ids.len())
                .map(|i| format!("${i}"))
                .collect::<Vec<_>>()
                .join(", ");
            txn.execute(Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                &format!(
                    "UPDATE product_productchannellisting SET discounted_price_dirty = TRUE WHERE product_id IN ({list})"
                ),
                ids,
            ))
            .await
            .map_err(DbError::SeaOrm)?;
        }
        let mut am: crate::entities::discount_promotionrule::ActiveModel = r.into();
        am.variants_dirty = Set(Some(false));
        am.update(&txn).await.map_err(DbError::SeaOrm)?;
        txn.commit().await.map_err(DbError::SeaOrm)?;
        done += 1;
    }
    Ok(done)
}

/// Recompute denormalized promo prices for dirty product listings; returns
/// listings processed.
pub async fn recalculate_dirty_listings(db: &sea_orm::DatabaseConnection) -> Result<usize> {
    use crate::entities::product_productchannellisting::{
        Column as PCol, Entity as PEnt,
    };
    use crate::entities::product_productvariantchannellisting::{
        Column as VCol, Entity as VEnt,
    };
    let dirty: Vec<(i32, i32, i32)> = PEnt::find()
        .select_only()
        .column(PCol::Id)
        .column(PCol::ProductId)
        .column(PCol::ChannelId)
        .filter(PCol::DiscountedPriceDirty.eq(true))
        .order_by_asc(PCol::Id)
        .limit(LISTING_BATCH)
        .into_tuple()
        .all(db)
        .await
        .map_err(DbError::SeaOrm)?;
    if dirty.is_empty() {
        return Ok(0);
    }
    let mut rules_cache: HashMap<i32, Vec<promotions::ActiveRule>> = HashMap::new();
    let mut ctx_cache: HashMap<i32, promotions::VariantContext> = HashMap::new();
    let mut done = 0;
    for (lid, pid, ch) in dirty {
        use crate::entities::product_productvariant::{Column as PVCol, Entity as PVEnt};
        let vids: Vec<i32> = PVEnt::find()
            .select_only()
            .column(PVCol::Id)
            .filter(PVCol::ProductId.eq(pid))
            .into_tuple()
            .all(db)
            .await
            .map_err(DbError::SeaOrm)?;
        let vlistings: Vec<(i32, Option<Decimal>, String)> = if vids.is_empty() {
            Vec::new()
        } else {
            VEnt::find()
                .select_only()
                .column(VCol::Id)
                .column(VCol::PriceAmount)
                .column(VCol::Currency)
                .filter(VCol::VariantId.is_in(vids))
                .filter(VCol::ChannelId.eq(ch))
                .into_tuple()
                .all(db)
                .await
                .map_err(DbError::SeaOrm)?
        };
        let rules = match rules_cache.get(&ch) {
            Some(r) => r.clone(),
            None => {
                let r = promotions::active_catalogue_rules(db, ch).await?;
                rules_cache.insert(ch, r.clone());
                r
            }
        };
        let ctx = match ctx_cache.get(&pid) {
            Some(c) => c.clone(),
            None => {
                let c = promotions::variant_context(db, pid).await?;
                ctx_cache.insert(pid, c.clone());
                c
            }
        };
        let txn = db.begin().await.map_err(DbError::SeaOrm)?;
        let mut min_price: Option<Decimal> = None;
        for (vlid, price_opt, currency) in vlistings {
            let price = price_opt.unwrap_or(Decimal::ZERO);
            // Best applicable rule, same math as the checkout line path.
            let mut best: Option<(uuid::Uuid, Decimal)> = None;
            for rule in &rules {
                if !promotions::rule_matches_variant(&txn, rule, variant_of(&txn, vlid).await?, &ctx).await? {
                    continue;
                }
                let saving = saleor_rustify_core::discount::saving_for_rule(
                    price,
                    &rule.reward_value_type,
                    rule.reward_value,
                    &currency,
                );
                if saving > Decimal::ZERO && best.map(|(_, s)| saving > s).unwrap_or(true) {
                    best = Some((rule.rule_id, saving));
                }
            }
            let (discounted, link) = match best {
                Some((rid, saving)) => {
                    let d = (price - saving).max(Decimal::ZERO);
                    (d, Some((rid, saving)))
                }
                None => (price, None),
            };
            txn.execute(Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                "UPDATE product_productvariantchannellisting SET discounted_price_amount = $2 WHERE id = $1",
                vec![vlid.into(), discounted.into()],
            ))
            .await
            .map_err(DbError::SeaOrm)?;
            // Refresh the listing↔rule link rows (delete + insert current).
            use crate::entities::product_variantchannellistingpromotionrule::{
                Column as LCol, Entity as LEnt,
            };
            LEnt::delete_many()
                .filter(LCol::VariantChannelListingId.eq(vlid))
                .exec(&txn)
                .await
                .map_err(DbError::SeaOrm)?;
            if let Some((rid, saving)) = link {
                let am =
                    crate::entities::product_variantchannellistingpromotionrule::ActiveModel {
                        discount_amount: Set(saving),
                        currency: Set(currency.clone()),
                        promotion_rule_id: Set(rid),
                        variant_channel_listing_id: Set(vlid),
                        ..Default::default()
                    };
                let _ = am.insert(&txn).await;
            }
            min_price = Some(min_price.map(|m: Decimal| m.min(discounted)).unwrap_or(discounted));
        }
        let min = min_price.unwrap_or(Decimal::ZERO);
        txn.execute(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "UPDATE product_productchannellisting SET discounted_price_amount = $2, discounted_price_dirty = FALSE WHERE id = $1",
            vec![lid.into(), min.into()],
        ))
        .await
        .map_err(DbError::SeaOrm)?;
        txn.commit().await.map_err(DbError::SeaOrm)?;
        done += 1;
    }
    Ok(done)
}

async fn variant_of(txn: &sea_orm::DatabaseTransaction, vlid: i32) -> Result<i32> {    use crate::entities::product_productvariantchannellisting::{Column as VCol, Entity as VEnt};
    VEnt::find_by_id(vlid)
        .select_only()
        .column(VCol::VariantId)
        .into_tuple::<i32>()
        .one(txn)
        .await
        .map_err(DbError::SeaOrm)?
        .ok_or_else(|| DbError::Catalog("variant listing vanished".into()))
}

// ---------------------------------------------------------------------------
// Write-path invalidation (Django `post_save` signals on PromotionRule /
// Promotion). The beat passes above do the heavy recompute; these only set
// the flags so dashboard edits take effect within one beat interval.
// ---------------------------------------------------------------------------

/// Mark all rules of a promotion dirty (date/type edits change activity).
pub async fn mark_promotion_rules_dirty(
    db: &impl ConnectionTrait,
    promo_id: uuid::Uuid,
) -> Result<()> {
    use crate::entities::discount_promotionrule::{Column as RCol, Entity as REnt};
    REnt::update_many()
        .col_expr(RCol::VariantsDirty, sea_orm::sea_query::Expr::value(true))
        .filter(RCol::PromotionId.eq(promo_id))
        .exec(db)
        .await
        .map_err(DbError::SeaOrm)?;
    Ok(())
}

/// Mark listings dirty for every product linked to one rule (rule delete).
pub async fn mark_rule_products_dirty(
    db: &impl ConnectionTrait,
    rule_id: uuid::Uuid,
) -> Result<()> {
    db.execute(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "UPDATE product_productchannellisting SET discounted_price_dirty = TRUE \
         WHERE product_id IN (SELECT product_id FROM product_productvariant WHERE id IN \
         (SELECT productvariant_id FROM discount_promotionrule_variants WHERE promotionrule_id = $1))",
        vec![rule_id.into()],
    ))
    .await
    .map_err(DbError::SeaOrm)?;
    Ok(())
}

/// Mark listings dirty for every product linked to any rule of a promotion
/// (promotion delete — call BEFORE the M2M rows go away).
pub async fn mark_promotion_products_dirty(
    db: &impl ConnectionTrait,
    promo_id: uuid::Uuid,
) -> Result<()> {
    db.execute(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "UPDATE product_productchannellisting SET discounted_price_dirty = TRUE \
         WHERE product_id IN (SELECT product_id FROM product_productvariant WHERE id IN \
         (SELECT productvariant_id FROM discount_promotionrule_variants WHERE promotionrule_id IN \
         (SELECT id FROM discount_promotionrule WHERE promotion_id = $1)))",
        vec![promo_id.into()],
    ))
    .await
    .map_err(DbError::SeaOrm)?;
    Ok(())
}
