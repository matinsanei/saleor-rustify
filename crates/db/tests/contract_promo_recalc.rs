//! Beat promotion-recalc contract (live Postgres):
//! catalogue promotion (10% off one product) → link refresh writes the M2M +
//! dirties listings → price pass writes promo-adjusted denormalized prices →
//! promotion delete restores list prices. Mirrors
//! `saleor/product/tests/test_tasks.py` + `test_discounted_prices.py` status flow.

use rust_decimal::Decimal;
use saleor_rustify_db::{catalog, database_url, promo_recalc, promo_writes};
use sea_orm::DatabaseConnection;
use serde_json::json;
use uuid::Uuid;

async fn db() -> DatabaseConnection {
    saleor_rustify_db::connect(&database_url())
        .await
        .expect("saleor postgres must be up (localhost:5434)")
}

fn gid(ty: &str, id: i32) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(format!("{ty}:{id}"))
}

#[tokio::test]
async fn promo_recalc_end_to_end() {
    let db = db().await;
    let (ch, currency) = catalog::channel_info(&db, "default-channel").await.unwrap();
    // Seed DB ships active catalogue promos (30/40/10% on fixed products/
    // variants) + leftovers from earlier failed runs can pollute the math.
    // Use a product covered by NO seed predicate, and wipe our leftovers.
    use saleor_rustify_db::entities::discount_promotion::{Column as DPCol, Entity as DPEnt};
    let stale: Vec<Uuid> = DPEnt::find()
        .select_only()
        .column(DPCol::Id)
        .filter(DPCol::Name.like("e2e-promo%"))
        .into_tuple()
        .all(&db)
        .await
        .unwrap();
    for id in stale {
        let _ = promo_writes::delete_promotion(&db, id).await;
    }
    let _ = promo_recalc::refresh_rule_variant_links(&db).await;
    let _ = promo_recalc::recalculate_dirty_listings(&db).await;

    // Seed variant listing with a known price, on a product covered by NO
    // seed predicate (products 130/131/141/163 + variants 338/366/384/395
    // are taken by seed promos).
    use saleor_rustify_db::entities::product_productvariant::{Column as VCol, Entity as VEnt};
    use saleor_rustify_db::entities::product_productvariantchannellisting::{
        Column as LCol, Entity as LEnt,
    };
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
    let taken_pids = [130, 131, 141, 163];
    let taken_vids = [338, 366, 384, 395];
    let (vid, pid): (i32, i32) = VEnt::find()
        .select_only()
        .column(VCol::Id)
        .column(VCol::ProductId)
        .into_tuple::<(i32, i32)>()
        .all(&db)
        .await
        .unwrap()
        .into_iter()
        .find(|(v, p)| !taken_vids.contains(v) && !taken_pids.contains(p))
        .expect("a seed variant outside seed predicates must exist");
    // Ensure a listing row exists in the default channel.
    let listing = LEnt::find()
        .filter(LCol::VariantId.eq(vid))
        .filter(LCol::ChannelId.eq(ch))
        .one(&db)
        .await
        .unwrap()
        .expect("seed variant listing must exist");
    let price = listing.price_amount.unwrap();

    // Catalogue promotion: 10% off exactly this product, active now.
    let promo = promo_writes::create_promotion(
        &db,
        "e2e-promo",
        "catalogue",
        json!({}),
        Some(chrono::Utc::now() - chrono::Duration::days(1)),
        None,
        vec![promo_writes::NewRule {
            name: Some("e2e-rule".into()),
            description: json!({}),
            catalogue_predicate: json!({"productPredicate": {"ids": [gid("Product", pid)]}}),
            order_predicate: json!({}),
            reward_value_type: Some("percentage".into()),
            reward_value: Some(Decimal::new(10, 0)),
            reward_type: Some("subtotal".into()),
            channel_ids: vec![ch],
            gift_variant_ids: vec![],
        }],
    )
    .await
    .unwrap();

    // 1. Link refresh: M2M row appears, listings dirtied.
    let n = promo_recalc::refresh_rule_variant_links(&db).await.unwrap();
    assert_eq!(n, 1, "one dirty rule processed");
    use saleor_rustify_db::entities::discount_promotionrule_variants::{
        Column as MVCol, Entity as MVEnt,
    };
    let linked = MVEnt::find()
        .filter(MVCol::ProductvariantId.eq(vid))
        .one(&db)
        .await
        .unwrap()
        .is_some();
    assert!(linked, "rule↔variant M2M must exist");

    // 2. Price pass: variant listing drops to 90%, product listing follows.
    let m = promo_recalc::recalculate_dirty_listings(&db).await.unwrap();
    assert!(m >= 1, "dirty listings repriced");
    let after: Option<Decimal> = LEnt::find_by_id(listing.id)
        .select_only()
        .column(LCol::DiscountedPriceAmount)
        .into_tuple()
        .one(&db)
        .await
        .unwrap()
        .flatten();
    let expect = (price * Decimal::new(90, 0) / Decimal::new(100, 0)).round_dp(2);
    let got = after.unwrap().round_dp(2);
    assert_eq!(got, expect, "price {price} {currency} → {got}, want {expect}");
    use saleor_rustify_db::entities::product_productchannellisting::{
        Column as PCol, Entity as PEnt,
    };
    let pmin: Option<Decimal> = PEnt::find()
        .select_only()
        .column(PCol::DiscountedPriceAmount)
        .filter(PCol::ProductId.eq(pid))
        .filter(PCol::ChannelId.eq(ch))
        .into_tuple()
        .one(&db)
        .await
        .unwrap()
        .flatten();
    assert!(pmin.is_some(), "product listing repriced");

    // 3. Promotion delete restores list prices (mark → recalc).
    promo_writes::delete_promotion(&db, promo).await.unwrap();
    let m2 = promo_recalc::recalculate_dirty_listings(&db).await.unwrap();
    assert!(m2 >= 1, "listings dirtied by delete");
    let restored: Option<Decimal> = LEnt::find_by_id(listing.id)
        .select_only()
        .column(LCol::DiscountedPriceAmount)
        .into_tuple()
        .one(&db)
        .await
        .unwrap()
        .flatten();
    assert_eq!(restored.unwrap().round_dp(2), price.round_dp(2));
}
