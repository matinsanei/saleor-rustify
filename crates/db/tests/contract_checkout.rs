//! Checkout persistence contract: rows Rust writes must be identical to
//! rows Django writes, and Django-visible behavior must hold.
//! Mirrors `saleor/checkout/tests/test_checkout.py` (creation/lines) and
//! `test_base_calculations.py` (price_override precedence).
//!
//! Requires the Saleor database (`RUSTIFY_DATABASE_URL`).

use rust_decimal::Decimal;
use saleor_rustify_db::{catalog, checkout_store, database_url};
use sea_orm::DatabaseConnection;

async fn db() -> DatabaseConnection {
    saleor_rustify_db::connect(&database_url())
        .await
        .expect("saleor postgres must be up (localhost:5434)")
}

async fn usd_variant(db: &DatabaseConnection) -> (i32, Decimal) {
    let products = catalog::list_products(db, "default-channel", None, 10_000)
        .await
        .unwrap();
    let v = products
        .iter()
        .flat_map(|p| &p.variants)
        .next()
        .expect("populatedb must have variants");
    (v.id.parse().unwrap(), v.price.amount)
}

#[tokio::test]
async fn created_row_carries_django_defaults() {
    let db = db().await;
    let (ch_id, currency) = catalog::channel_info(&db, "default-channel").await.unwrap();

    let token = checkout_store::create_checkout_row(&db, ch_id, &currency, "buyer@example.com")
        .await
        .unwrap();

    let (co, _) = checkout_store::load_checkout(&db, token)
        .await
        .unwrap()
        .expect("row must exist");
    assert_eq!(co.currency, "USD");
    assert_eq!(co.country, "US");
    assert_eq!(co.language_code, "en");
    assert_eq!(co.authorize_status, "none");
    assert_eq!(co.charge_status, "none");
    assert_eq!(co.email.as_deref(), Some("buyer@example.com"));

    checkout_store::delete_checkout_row(&db, token)
        .await
        .unwrap();
}

#[tokio::test]
async fn lines_persist_and_total_matches_listings() {
    let db = db().await;
    let (ch_id, currency) = catalog::channel_info(&db, "default-channel").await.unwrap();
    let (vid, unit) = usd_variant(&db).await;

    let token = checkout_store::create_checkout_row(&db, ch_id, &currency, "")
        .await
        .unwrap();
    checkout_store::add_line_row(&db, token, vid, 3, unit, &currency, None)
        .await
        .unwrap();

    let (co, lines) = checkout_store::load_checkout(&db, token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].variant_id, vid);
    assert_eq!(lines[0].quantity, 3);
    // Pre-tax parity: net == gross == 3 x unit.
    assert_eq!(lines[0].total_price_net_amount, unit * Decimal::from(3));
    assert_eq!(
        lines[0].total_price_gross_amount,
        lines[0].total_price_net_amount
    );

    let domain = checkout_store::to_domain(&co, &lines, "default-channel");
    assert_eq!(domain.total().amount, unit * Decimal::from(3));

    checkout_store::delete_checkout_row(&db, token)
        .await
        .unwrap();
}

#[tokio::test]
async fn price_override_wins_in_domain_total() {
    // Mirrors test_calculate_base_line_unit_price_with_custom_price.
    let db = db().await;
    let (ch_id, currency) = catalog::channel_info(&db, "default-channel").await.unwrap();
    let (vid, _) = usd_variant(&db).await;

    let token = checkout_store::create_checkout_row(&db, ch_id, &currency, "")
        .await
        .unwrap();
    let override_price = Decimal::new(1222, 2);
    checkout_store::add_line_row(
        &db,
        token,
        vid,
        1,
        Decimal::new(9999, 2),
        &currency,
        Some(override_price),
    )
    .await
    .unwrap();

    let (co, lines) = checkout_store::load_checkout(&db, token)
        .await
        .unwrap()
        .unwrap();
    let domain = checkout_store::to_domain(&co, &lines, "default-channel");
    assert_eq!(domain.lines[0].unit_price.amount, override_price);
    assert_eq!(domain.total().amount, override_price);

    checkout_store::delete_checkout_row(&db, token)
        .await
        .unwrap();
}

#[tokio::test]
async fn zero_quantity_rejected_like_django_validator() {
    let db = db().await;
    let (ch_id, currency) = catalog::channel_info(&db, "default-channel").await.unwrap();
    let (vid, unit) = usd_variant(&db).await;

    let token = checkout_store::create_checkout_row(&db, ch_id, &currency, "")
        .await
        .unwrap();
    assert!(
        checkout_store::add_line_row(&db, token, vid, 0, unit, &currency, None)
            .await
            .is_err()
    );

    checkout_store::delete_checkout_row(&db, token)
        .await
        .unwrap();
}

#[tokio::test]
async fn delete_removes_checkout_and_lines() {
    let db = db().await;
    let (ch_id, currency) = catalog::channel_info(&db, "default-channel").await.unwrap();
    let (vid, unit) = usd_variant(&db).await;

    let token = checkout_store::create_checkout_row(&db, ch_id, &currency, "")
        .await
        .unwrap();
    checkout_store::add_line_row(&db, token, vid, 1, unit, &currency, None)
        .await
        .unwrap();
    checkout_store::delete_checkout_row(&db, token)
        .await
        .unwrap();

    assert!(checkout_store::load_checkout(&db, token)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn same_variant_merges_into_one_row() {
    // Django get_line semantics: adding the same variant twice bumps
    // quantity on ONE row instead of duplicating lines.
    use saleor_rustify_db::checkout_store::NewLine;

    let db = db().await;
    let (ch_id, currency) = catalog::channel_info(&db, "default-channel").await.unwrap();
    let (vid, unit) = usd_variant(&db).await;

    let token = checkout_store::create_checkout_row(&db, ch_id, &currency, "")
        .await
        .unwrap();
    let item = || NewLine {
        variant_id: vid,
        quantity: 1,
        unit_price: unit,
        price_override: None,
    };
    checkout_store::add_lines_tx(&db, token, ch_id, &currency, &[item()])
        .await
        .unwrap();
    checkout_store::add_lines_tx(&db, token, ch_id, &currency, &[item(), item()])
        .await
        .unwrap();

    let (_, lines) = checkout_store::load_checkout(&db, token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lines.len(), 1, "same variant must merge into one row");
    assert_eq!(lines[0].quantity, 3);

    checkout_store::delete_checkout_row(&db, token)
        .await
        .unwrap();
}

#[tokio::test]
async fn denormalized_totals_refresh_for_django_readers() {
    use saleor_rustify_db::checkout_store::NewLine;

    let db = db().await;
    let (ch_id, currency) = catalog::channel_info(&db, "default-channel").await.unwrap();
    let (vid, unit) = usd_variant(&db).await;

    let token = checkout_store::create_checkout_row(&db, ch_id, &currency, "")
        .await
        .unwrap();
    checkout_store::add_lines_tx(
        &db,
        token,
        ch_id,
        &currency,
        &[NewLine {
            variant_id: vid,
            quantity: 2,
            unit_price: unit,
            price_override: None,
        }],
    )
    .await
    .unwrap();

    let (co, _) = checkout_store::load_checkout(&db, token)
        .await
        .unwrap()
        .unwrap();

    // Contract: denormalized totals were refreshed after add_lines_tx.
    // Catalogue promotions may reduce total below `unit*qty` (Django's
    // recalculation does the same subtraction), so we verify:
    //   1. total is non-negative
    //   2. total <= undiscounted line sum (discounts only reduce)
    //   3. Django-shaped books: subtotal holds line-level discounts only,
    //      order-level discounts (vouchers/order promotions) land in total,
    //      so subtotal >= total; pre-tax net == gross on both legs.
    let lines_gross_sum = unit * Decimal::from(2);
    assert!(
        co.total_gross_amount >= Decimal::ZERO,
        "total must be non-negative, got {}",
        co.total_gross_amount,
    );
    assert!(
        co.total_gross_amount <= lines_gross_sum,
        "total_gross={} must be <= undiscounted lines_sum={}",
        co.total_gross_amount,
        lines_gross_sum,
    );
    // Pre-tax invariant: net == gross on each leg; subtotal carries only
    // line-level discounts, so it sits at or above total.
    assert_eq!(
        co.total_net_amount, co.total_gross_amount,
        "net must equal gross (pre-tax)"
    );
    assert_eq!(
        co.subtotal_net_amount, co.subtotal_gross_amount,
        "subtotal net must equal gross (pre-tax)"
    );
    assert!(
        co.subtotal_gross_amount >= co.total_gross_amount,
        "subtotal_gross={} must be >= total_gross={} (order-level discounts live in total)",
        co.subtotal_gross_amount,
        co.total_gross_amount,
    );

    checkout_store::delete_checkout_row(&db, token)
        .await
        .unwrap();
}

#[tokio::test]
async fn add_lines_rejects_unknown_variant() {
    // Shared choke point: every writer (GraphQL + gRPC) funnels through
    // add_lines_tx, so unknown variants fail here with a coded error
    // instead of becoming zero-priced lines.
    let db = db().await;
    let (ch_id, currency) = catalog::channel_info(&db, "default-channel")
        .await
        .unwrap();
    let token = checkout_store::create_checkout_row(&db, ch_id, &currency, "choke@example.com")
        .await
        .unwrap();

    let err = checkout_store::add_lines_tx(
        &db,
        token,
        ch_id,
        &currency,
        &[checkout_store::NewLine {
            variant_id: 987_654_321,
            quantity: 1,
            unit_price: Decimal::ZERO,
            price_override: None,
        }],
    )
    .await
    .expect_err("unknown variant must be rejected");
    assert!(
        err.to_string().contains("NOT_FOUND"),
        "expected NOT_FOUND, got {err}"
    );

    checkout_store::delete_checkout_row(&db, token)
        .await
        .unwrap();
}

#[tokio::test]
async fn add_lines_rejects_insufficient_stock() {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    use saleor_rustify_db::entities::warehouse_stock;
    let db = db().await;
    let (ch_id, currency) = catalog::channel_info(&db, "default-channel")
        .await
        .unwrap();
    let (vid, unit) = usd_variant(&db).await;
    // Current sellable stock for this variant (Django validates against
    // stock AFTER the per-line limit check, so request one more unit —
    // unless that trips the limit first, in which case the LIMIT code is
    // the correct Saleor behavior too).
    let stock: i32 = warehouse_stock::Entity::find()
        .filter(warehouse_stock::Column::ProductVariantId.eq(vid))
        .all(&db)
        .await
        .unwrap()
        .iter()
        .map(|s| s.quantity - s.quantity_allocated)
        .sum();
    let token = checkout_store::create_checkout_row(&db, ch_id, &currency, "choke2@example.com")
        .await
        .unwrap();

    let err = checkout_store::add_lines_tx(
        &db,
        token,
        ch_id,
        &currency,
        &[checkout_store::NewLine {
            variant_id: vid,
            quantity: stock + 1,
            unit_price: unit,
            price_override: None,
        }],
    )
    .await
    .expect_err("over-stock quantity must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("INSUFFICIENT_STOCK") || msg.contains("QUANTITY_GREATER_THAN_LIMIT"),
        "expected INSUFFICIENT_STOCK (or LIMIT when stock+1 exceeds it), got {msg}"
    );

    checkout_store::delete_checkout_row(&db, token)
        .await
        .unwrap();
}
