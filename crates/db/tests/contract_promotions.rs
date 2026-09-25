//! Promotion engine contract vs Django's tables and Django-written rows.
//! Mirrors `saleor/discount/tests/` catalogue-promotion and voucher flows:
//! evaluation writes the same discount rows Django writes, totals drop by
//! the same amounts, voucher usage increments on completion.

use rust_decimal::Decimal;
use saleor_rustify_db::{catalog, checkout_store, database_url, promotions};
use sea_orm::{ConnectionTrait, DatabaseConnection};

async fn db() -> DatabaseConnection {
    saleor_rustify_db::connect(&database_url())
        .await
        .expect("saleor postgres must be up (localhost:5434)")
}

/// A variant covered by a LIVE catalogue percentage rule + its real
/// channel price (populatedb assigns rules to arbitrary variants, so the
/// covered set drifts between seeds — discover, never hardcode variant ids).
/// Min price 50 keeps the fixed-25 voucher math in range.
async fn live_pct_variant(db: &DatabaseConnection) -> (i32, i32, Decimal, Decimal) {
    use saleor_rustify_db::entities::{
        discount_promotion, discount_promotionrule, discount_promotionrule_channels,
    };
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
    let now = chrono::Utc::now();
    let promos: Vec<uuid::Uuid> = discount_promotion::Entity::find()
        .select_only()
        .column(discount_promotion::Column::Id)
        .filter(discount_promotion::Column::Type.eq("catalogue"))
        .filter(discount_promotion::Column::StartDate.lte(now))
        .filter(
            discount_promotion::Column::EndDate
                .is_null()
                .or(discount_promotion::Column::EndDate.gt(now)),
        )
        .into_tuple()
        .all(db)
        .await
        .unwrap();
    let (ch_id, _) = catalog::channel_info(db, "default-channel").await.unwrap();
    let rules = discount_promotionrule::Entity::find()
        .filter(discount_promotionrule::Column::PromotionId.is_in(promos))
        .filter(discount_promotionrule::Column::RewardValueType.eq("percentage"))
        .all(db)
        .await
        .unwrap();
    for r in rules {
        let on_channel = discount_promotionrule_channels::Entity::find()
            .filter(discount_promotionrule_channels::Column::PromotionruleId.eq(r.id))
            .filter(discount_promotionrule_channels::Column::ChannelId.eq(ch_id))
            .one(db)
            .await
            .unwrap()
            .is_some();
        if !on_channel {
            continue;
        }
        let ids = r.catalogue_predicate
            .get("variantPredicate")
            .and_then(|v| v.get("ids"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for gid in ids {
            let raw = gid.as_str().unwrap_or_default();
            let Some(vid) = saleor_rustify_db::catalog::parse_gid(raw) else { continue };
            let pricing = catalog::checkout_pricing(db, "default-channel", &[vid]).await.unwrap();
            let Some((money, _)) = pricing.get(&vid) else { continue };
            if money.amount < Decimal::new(5000, 2) {
                continue;
            }
            let pid: Option<i32> = {
                use saleor_rustify_db::entities::product_productvariant;
                use sea_orm::QuerySelect;
                product_productvariant::Entity::find_by_id(vid)
                    .select_only()
                    .column(product_productvariant::Column::ProductId)
                    .into_tuple()
                    .one(db)
                    .await
                    .unwrap()
                    .flatten()
            };
            if let Some(pid) = pid {
                return (vid, pid, money.amount, r.reward_value.unwrap_or_default());
            }
        }
    }
    panic!("no live catalogue percentage rule covering a priced variant (seed data?)");
}

#[tokio::test]
async fn catalogue_rule_evaluates_on_real_variant() {
    use saleor_rustify_db::entities::product_productvariant;
    use sea_orm::{EntityTrait, QuerySelect};

    let db = db().await;
    let (ch_id, _) = catalog::channel_info(&db, "default-channel").await.unwrap();
    let (vid, pid, price, pct) = live_pct_variant(&db).await;
    let eval = promotions::evaluate_line(&db, ch_id, vid, pid, price, 2, "USD")
        .await
        .unwrap()
        .expect("rule must apply");
    assert_eq!(eval.reward_value_type, "percentage");
    assert_eq!(eval.reward_value, pct);
    // Whole-line amount = per-unit saving x qty (engine's own math).
    let want = saleor_rustify_core::discount::saving_for_rule(price, "percentage", pct, "USD")
        * Decimal::new(2, 0);
    assert_eq!(eval.amount, want);
}

#[tokio::test]
async fn checkout_totals_drop_by_promotion() {
    use saleor_rustify_db::checkout_store::NewLine;

    let db = db().await;
    let (ch_id, currency) = catalog::channel_info(&db, "default-channel").await.unwrap();
    let (vid, _pid, price, pct) = live_pct_variant(&db).await;
    let token = checkout_store::create_checkout_row(&db, ch_id, &currency, "")
        .await
        .unwrap();
    checkout_store::add_lines_tx(
        &db,
        token,
        ch_id,
        &currency,
        &[NewLine { variant_id: vid, quantity: 1, unit_price: price, price_override: None }],
    )
    .await
    .unwrap();

    let (co, lines) = checkout_store::load_checkout(&db, token).await.unwrap().unwrap();
    let saving = saleor_rustify_core::discount::saving_for_rule(price, "percentage", pct, &currency);
    // Undiscounted line stays full price; checkout total drops by the saving.
    assert_eq!(lines[0].total_price_gross_amount, price);
    assert_eq!(co.total_gross_amount, price - saving);

    // The discount row Django would write exists.
    use saleor_rustify_db::entities::discount_checkoutlinediscount;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let d = discount_checkoutlinediscount::Entity::find()
        .filter(discount_checkoutlinediscount::Column::LineId.eq(lines[0].id))
        .filter(discount_checkoutlinediscount::Column::UniqueType.eq("promotion"))
        .one(&db)
        .await
        .unwrap()
        .expect("promotion discount row must exist");
    assert_eq!(d.amount_value, saving);
    assert_eq!(d.r#type, "promotion");

    checkout_store::delete_checkout_row(&db, token).await.unwrap();
    // Discount rows cascade with lines (deleted above via checkout delete).
    assert!(
        discount_checkoutlinediscount::Entity::find()
            .filter(discount_checkoutlinediscount::Column::LineId.eq(lines[0].id))
            .one(&db)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn voucher_entire_order_applies_and_increments_on_complete() {
    use saleor_rustify_db::{checkout_store::NewLine, order_store};
    use saleor_rustify_db::entities::discount_vouchercode;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    let db = db().await;
    let (ch_id, currency) = catalog::channel_info(&db, "default-channel").await.unwrap();
    let (vid, _pid, price, pct) = live_pct_variant(&db).await;
    let token = checkout_store::create_checkout_row(&db, ch_id, &currency, "buyer@example.com")
        .await
        .unwrap();
    checkout_store::add_lines_tx(
        &db,
        token,
        ch_id,
        &currency,
        &[NewLine { variant_id: vid, quantity: 1, unit_price: price, price_override: None }],
    )
    .await
    .unwrap();

    let before: i32 = discount_vouchercode::Entity::find()
        .filter(discount_vouchercode::Column::Code.eq("DISCOUNT"))
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .used;

    // DISCOUNT = entire_order fixed 25.00 USD. Base after catalogue saving.
    let saving = saleor_rustify_core::discount::saving_for_rule(price, "percentage", pct, &currency);
    let applied = promotions::apply_voucher(&db, token, "DISCOUNT", "default-channel")
        .await
        .unwrap();
    assert_eq!(applied.voucher_type, "entire_order");
    assert_eq!(applied.amount, Decimal::new(2500, 2));
    checkout_store::refresh_totals(&db, token).await.unwrap();
    let (co, _) = checkout_store::load_checkout(&db, token).await.unwrap().unwrap();
    assert_eq!(co.total_gross_amount, price - saving - Decimal::new(2500, 2));
    assert_eq!(co.voucher_code.as_deref(), Some("DISCOUNT"));

    // Complete path increments usage (same call complete_checkout makes).
    promotions::increase_usage(&db, "DISCOUNT", Some("buyer@example.com"))
        .await
        .unwrap();
    let after: i32 = discount_vouchercode::Entity::find()
        .filter(discount_vouchercode::Column::Code.eq("DISCOUNT"))
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .used;
    assert_eq!(after, before + 1);

    // Roll usage back (test hygiene on shared data) + cleanup checkout.
    {
        use sea_orm::Statement;
        db.execute(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "UPDATE discount_vouchercode SET used = $1 WHERE code = 'DISCOUNT'",
            vec![before.into()],
        ))
        .await
        .unwrap();
        use saleor_rustify_db::entities::discount_vouchercustomer;
        discount_vouchercustomer::Entity::delete_many()
            .filter(discount_vouchercustomer::Column::CustomerEmail.eq("buyer@example.com"))
            .exec(&db)
            .await
            .unwrap();
    }
    // Remove voucher discount rows + checkout.
    {
        use saleor_rustify_db::entities::discount_checkoutdiscount;
        discount_checkoutdiscount::Entity::delete_many()
            .filter(discount_checkoutdiscount::Column::CheckoutId.eq(token))
            .exec(&db)
            .await
            .unwrap();
    }
    checkout_store::delete_checkout_row(&db, token).await.unwrap();
    let _ = order_store::next_number(&db).await.unwrap();
}
