//! Fund-release scan contract (live Postgres): an eligible transaction
//! (abandoned auto-refundable checkout past channel TTL, authorized funds,
//! no order) is detected with exact amounts; channel flags restored after.
//! Mirrors `saleor/payment/tests/test_tasks.py::test_transaction_release_funds_for_checkout_task`
//! eligibility (minus the PSP gateway call — detector only, by design).

use rust_decimal::Decimal;
use saleor_rustify_db::{catalog, database_url, funds_release};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use uuid::Uuid;

async fn db() -> DatabaseConnection {
    saleor_rustify_db::connect(&database_url())
        .await
        .expect("saleor postgres must be up (localhost:5434)")
}

#[tokio::test]
async fn release_funds_scan_end_to_end() {
    let db = db().await;
    let (ch, _) = catalog::channel_info(&db, "default-channel").await.unwrap();

    // Arm the channel gate (saved + restored): release flag + 1-second TTL.
    // NOTE: the interval columns are mapped as String in the entity, which
    // sqlx cannot decode — read/write them via raw SQL with ::text (the
    // scan itself keeps everything inside Postgres, unaffected).
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
    let old_ttl: String = db
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT checkout_ttl_before_releasing_funds::text FROM channel_channel WHERE id = $1",
            vec![ch.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<String>("", "checkout_ttl_before_releasing_funds")
        .unwrap();
    use saleor_rustify_db::entities::channel_channel::{Column as CCol, Entity as CEnt};
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
    let old_flag: bool = CEnt::find_by_id(ch)
        .select_only()
        .column(CCol::ReleaseFundsForExpiredCheckouts)
        .into_tuple()
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    db.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE channel_channel SET release_funds_for_expired_checkouts = TRUE, checkout_ttl_before_releasing_funds = '1 second'::interval WHERE id = $1",
        vec![ch.into()],
    ))
    .await
    .unwrap();

    // Abandoned checkout: partial authorize, refundable, last change long ago.
    // (PK is `token`, not `id`.)
    let co_token = Uuid::new_v4();
    let old = chrono::Utc::now() - chrono::Duration::days(2);
    {
        use saleor_rustify_db::entities::checkout_checkout::ActiveModel as CoAm;
        CoAm {
            token: Set(co_token),
            currency: Set("USD".into()),
            country: Set("US".into()),
            channel_id: Set(ch),
            language_code: Set("en".into()),
            authorize_status: Set("partial".into()),
            automatically_refundable: Set(true),
            discount_amount: Set(Decimal::ZERO),
            note: Set(String::new()),
            metadata: Set(Some(serde_json::json!({}))),
            private_metadata: Set(Some(serde_json::json!({}))),
            charge_status: Set("none".into()),
            total_net_amount: Set(Decimal::ZERO),
            total_gross_amount: Set(Decimal::ZERO),
            subtotal_net_amount: Set(Decimal::ZERO),
            subtotal_gross_amount: Set(Decimal::ZERO),
            shipping_price_net_amount: Set(Decimal::ZERO),
            shipping_price_gross_amount: Set(Decimal::ZERO),
            shipping_tax_rate: Set(Decimal::ZERO),
            created_at: Set(old.into()),
            last_change: Set(old.into()),
            price_expiration: Set((chrono::Utc::now() + chrono::Duration::days(1)).into()),
            ..Default::default()
        }
        .insert(&db)
        .await
        .unwrap();
    }
    let tid: i32 = {
        use saleor_rustify_db::entities::payment_transactionitem::ActiveModel as TAm;
        let row: saleor_rustify_db::entities::payment_transactionitem::Model = TAm {
            token: Set(Uuid::new_v4()),
            checkout_id: Set(Some(co_token)),
            currency: Set("USD".into()),
            authorized_value: Set(Decimal::new(100, 0)),
            charged_value: Set(Decimal::ZERO),
            last_refund_success: Set(true),
            created_at: Set(chrono::Utc::now().into()),
            modified_at: Set(chrono::Utc::now().into()),
            available_actions: Set(vec!["cancel".to_string(), "charge".to_string()]),
            metadata: Set(serde_json::json!({})),
            private_metadata: Set(serde_json::json!({})),
            refunded_value: Set(Decimal::ZERO),
            canceled_value: Set(Decimal::ZERO),
            refund_pending_value: Set(Decimal::ZERO),
            charge_pending_value: Set(Decimal::ZERO),
            authorize_pending_value: Set(Decimal::ZERO),
            cancel_pending_value: Set(Decimal::ZERO),
            ..Default::default()
        }
        .insert(&db)
        .await
        .unwrap();
        row.id
    };

    let hits = funds_release::scan_releasable_funds(&db).await.unwrap();
    let hit = hits.iter().find(|h| h.transaction_id == tid);
    assert!(hit.is_some(), "eligible transaction must be detected");
    let hit = hit.unwrap();
    assert_eq!(hit.authorized, Decimal::new(100, 0));
    assert_eq!(hit.currency, "USD");

    // Cleanup (reverse order) + channel gate restore.
    use saleor_rustify_db::entities::payment_transactionitem::Entity as TEnt;
    TEnt::delete_by_id(tid).exec(&db).await.unwrap();
    use saleor_rustify_db::entities::checkout_checkout::{Column as CoCol, Entity as CoEnt};
    CoEnt::delete_many()
        .filter(CoCol::Token.eq(co_token))
        .exec(&db)
        .await
        .unwrap();
    {
        db.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE channel_channel SET release_funds_for_expired_checkouts = $2, checkout_ttl_before_releasing_funds = $3::interval WHERE id = $1",
            vec![ch.into(), old_flag.into(), old_ttl.into()],
        ))
        .await
        .unwrap();
    }
    // Nothing left behind.
    let hits2 = funds_release::scan_releasable_funds(&db).await.unwrap();
    assert!(hits2.iter().all(|h| h.transaction_id != tid));
}
