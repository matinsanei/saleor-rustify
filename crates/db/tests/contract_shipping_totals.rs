//! Shipping-leg totals + shipping-voucher contracts (custom-API compat).
//!
//! Mirrors `saleor/checkout/` + `saleor/discount/` behavior the storefront
//! flow depends on:
//! - refresh_totals writes the method's channel-listing price into
//!   `shipping_price_*` and adds it to `total` (Django base calculations).
//! - SHIPPING vouchers require a selected delivery method
//!   (VOUCHER_NOT_APPLICABLE otherwise) and reduce the shipping leg.
//!
//! Requires the Saleor database (`RUSTIFY_DATABASE_URL`).

use rust_decimal::Decimal;
use saleor_rustify_db::{catalog, checkout_store, database_url, promotions};
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

async fn first_usd_method(db: &DatabaseConnection, ch_id: i32) -> (i32, Decimal) {
    use saleor_rustify_db::entities::shipping_shippingmethodchannellisting;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let row = shipping_shippingmethodchannellisting::Entity::find()
        .filter(shipping_shippingmethodchannellisting::Column::ChannelId.eq(ch_id))
        .one(db)
        .await
        .unwrap()
        .expect("populatedb must have shipping listings");
    (row.shipping_method_id, row.price_amount)
}

async fn new_checkout_with_line(
    db: &DatabaseConnection,
) -> (uuid::Uuid, i32, String) {
    let (ch_id, currency) = catalog::channel_info(db, "default-channel")
        .await
        .unwrap();
    let (vid, unit) = usd_variant(db).await;
    let token = checkout_store::create_checkout_row(db, ch_id, &currency, "ship@example.com")
        .await
        .unwrap();
    checkout_store::add_lines_tx(
        db,
        token,
        ch_id,
        &currency,
        &[checkout_store::NewLine {
            variant_id: vid,
            quantity: 1,
            unit_price: unit,
            price_override: None,
        }],
    )
    .await
    .unwrap();
    (token, ch_id, currency)
}

#[tokio::test]
async fn shipping_price_flows_into_totals() {
    let db = db().await;
    let (token, ch_id, _) = new_checkout_with_line(&db).await;
    let (mid, price) = first_usd_method(&db, ch_id).await;
    checkout_store::set_shipping_method_opt(&db, token, Some(mid))
        .await
        .unwrap();
    checkout_store::refresh_totals(&db, token).await.unwrap();

    let (co, _) = checkout_store::load_checkout(&db, token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(co.shipping_price_gross_amount, price);
    assert_eq!(co.shipping_price_net_amount, price);
    // total = subtotal + shipping (no order-level discounts here).
    assert_eq!(
        co.total_gross_amount,
        (co.subtotal_gross_amount + price).max(Decimal::ZERO)
    );

    checkout_store::delete_checkout_row(&db, token).await.unwrap();
}

#[tokio::test]
async fn shipping_voucher_needs_delivery_method() {
    let db = db().await;
    let (token, _, _) = new_checkout_with_line(&db).await;

    let err = match promotions::apply_voucher(&db, token, "FREESHIPPING", "default-channel").await {
        Ok(_) => panic!("shipping voucher without a method must fail"),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains("VOUCHER_NOT_APPLICABLE"),
        "expected VOUCHER_NOT_APPLICABLE, got {err}"
    );

    checkout_store::delete_checkout_row(&db, token).await.unwrap();
}

#[tokio::test]
async fn free_shipping_voucher_zeroes_shipping_leg() {
    let db = db().await;
    let (token, ch_id, _) = new_checkout_with_line(&db).await;
    let (mid, _) = first_usd_method(&db, ch_id).await;
    checkout_store::set_shipping_method_opt(&db, token, Some(mid))
        .await
        .unwrap();

    // FREESHIPPING is percentage/100 on USD: full shipping discount.
    let app = promotions::apply_voucher(&db, token, "FREESHIPPING", "default-channel")
        .await
        .expect("shipping voucher with a method must apply");
    assert_eq!(app.voucher_type, "shipping");
    checkout_store::refresh_totals(&db, token).await.unwrap();

    let (co, _) = checkout_store::load_checkout(&db, token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(co.voucher_code.as_deref(), Some("FREESHIPPING"));
    assert_eq!(co.shipping_price_gross_amount, Decimal::ZERO);
    // Nothing leaks into the order-level voucher term (no double count).
    assert_eq!(co.total_gross_amount, co.subtotal_gross_amount);

    checkout_store::delete_checkout_row(&db, token).await.unwrap();
}
