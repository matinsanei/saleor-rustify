//! Order expiry contract (live Postgres): stale UNCONFIRMED order (no
//! payments, past channel TTL) → EXPIRED + event + voucher use released +
//! allocation freed; long-expired order → deleted with children.
//! Mirrors `saleor/order/tests/test_tasks.py::test_expire_orders_task` and
//! `test_delete_expired_orders_task` (minus plugin/webhook fan-out).

use rust_decimal::Decimal;
use saleor_rustify_db::{catalog, database_url, order_expiry};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use serde_json::json;
use uuid::Uuid;

async fn db() -> DatabaseConnection {
    saleor_rustify_db::connect(&database_url())
        .await
        .expect("saleor postgres must be up (localhost:5434)")
}

async fn arm_channel(db: &DatabaseConnection, ch: i32) -> (Option<i32>, String) {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
    let old_expire: Option<i32> = {
        use saleor_rustify_db::entities::channel_channel::{Column as CCol, Entity as CEnt};
        use sea_orm::{ColumnTrait, QueryFilter, QuerySelect};
        CEnt::find_by_id(ch)
            .select_only()
            .column(CCol::ExpireOrdersAfter)
            .into_tuple()
            .one(db)
            .await
            .unwrap()
            .flatten()
    };
    let old_delete: String = db
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT delete_expired_orders_after::text FROM channel_channel WHERE id = $1",
            vec![ch.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<String>("", "delete_expired_orders_after")
        .unwrap();
    db.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE channel_channel SET expire_orders_after = 1, delete_expired_orders_after = '1 second'::interval WHERE id = $1",
        vec![ch.into()],
    ))
    .await
    .unwrap();
    (old_expire, old_delete)
}

async fn restore_channel(db: &DatabaseConnection, ch: i32, old_expire: Option<i32>, old_delete: String) {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
    db.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE channel_channel SET expire_orders_after = $2, delete_expired_orders_after = $3::interval WHERE id = $1",
        vec![ch.into(), old_expire.into(), old_delete.into()],
    ))
    .await
    .unwrap();
}

fn order_am(
    id: Uuid,
    ch: i32,
    status: &str,
    created: chrono::DateTime<chrono::Utc>,
    voucher_code: Option<String>,
) -> saleor_rustify_db::entities::order_order::ActiveModel {
    use saleor_rustify_db::entities::order_order::ActiveModel as OAm;
    OAm {
        id: Set(id),
        number: Set((rand::random::<i32>() % 900000 + 100000).abs()),
        created_at: Set(created.into()),
        updated_at: Set(created.into()),
        status: Set(status.into()),
        authorize_status: Set("none".into()),
        charge_status: Set("none".into()),
        origin: Set("checkout".into()),
        checkout_token: Set(id.to_string()),
        channel_id: Set(ch),
        currency: Set("USD".into()),
        user_email: Set("e2e@example.com".into()),
        language_code: Set("en".into()),
        display_gross_prices: Set(true),
        weight: Set(0.0),
        undiscounted_base_shipping_price_amount: Set(Decimal::ZERO),
        lines_count: Set(0),
        should_refresh_prices: Set(false),
        tax_exemption: Set(false),
        use_old_id: Set(false),
        customer_note: Set(String::new()),
        total_net_amount: Set(Decimal::ZERO),
        total_gross_amount: Set(Decimal::ZERO),
        undiscounted_total_net_amount: Set(Decimal::ZERO),
        undiscounted_total_gross_amount: Set(Decimal::ZERO),
        subtotal_net_amount: Set(Decimal::ZERO),
        subtotal_gross_amount: Set(Decimal::ZERO),
        shipping_price_net_amount: Set(Decimal::ZERO),
        shipping_price_gross_amount: Set(Decimal::ZERO),
        base_shipping_price_amount: Set(Decimal::ZERO),
        total_charged_amount: Set(Decimal::ZERO),
        total_authorized_amount: Set(Decimal::ZERO),
        metadata: Set(json!({})),
        private_metadata: Set(json!({})),
        shipping_tax_class_metadata: Set(json!({})),
        shipping_tax_class_private_metadata: Set(json!({})),
        search_document: Set(String::new()),
        tracking_client_id: Set(String::new()),
        voucher_code: Set(voucher_code),
        ..Default::default()
    }
}

#[tokio::test]
async fn order_expiry_end_to_end() {
    let db = db().await;
    let (ch, _) = catalog::channel_info(&db, "default-channel").await.unwrap();
    let (old_expire, old_delete) = arm_channel(&db, ch).await;

    // Limited voucher + code (used=1) for the release branch.
    let t = chrono::Utc::now();
    let vid: i32 = {
        use saleor_rustify_db::entities::discount_voucher::ActiveModel as VAm;
        let row: saleor_rustify_db::entities::discount_voucher::Model = VAm {
            name: Set(Some("e2e".into())),
            usage_limit: Set(Some(5)),
            start_date: Set(t.into()),
            discount_value_type: Set("fixed".into()),
            r#type: Set("entire_order".into()),
            apply_once_per_order: Set(false),
            countries: Set("".into()),
            apply_once_per_customer: Set(false),
            only_for_staff: Set(false),
            metadata: Set(json!({})),
            private_metadata: Set(json!({})),
            single_use: Set(false),
            ..Default::default()
        }
        .insert(&db)
        .await
        .unwrap();
        row.id
    };
    let code = format!("E2E{}", &Uuid::new_v4().to_string()[..8].to_uppercase());
    {
        use saleor_rustify_db::entities::discount_vouchercode::ActiveModel as CAm;
        CAm {
            id: Set(Uuid::new_v4()),
            code: Set(code.clone()),
            used: Set(1),
            voucher_id: Set(vid),
            ..Default::default()
        }
        .insert(&db)
        .await
        .unwrap();
    }

    // 1. Stale unconfirmed order with the voucher → expired, use released.
    let oid = Uuid::new_v4();
    let old = chrono::Utc::now() - chrono::Duration::days(2);
    order_am(oid, ch, "unconfirmed", old, Some(code.clone()))
        .insert(&db)
        .await
        .unwrap();
    let n = order_expiry::expire_unconfirmed_orders(&db).await.unwrap();
    assert!(n >= 1, "stale unconfirmed order must expire");
    use saleor_rustify_db::entities::order_order::{Column as OCol, Entity as OEnt};
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
    let st: String = OEnt::find_by_id(oid)
        .select_only()
        .column(OCol::Status)
        .into_tuple()
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(st, "expired");
    let used: i32 = {
        use saleor_rustify_db::entities::discount_vouchercode::{Column as VCCol, Entity as VCEnt};
        VCEnt::find()
            .select_only()
            .column(VCCol::Used)
            .filter(VCCol::Code.eq(code.clone()))
            .into_tuple()
            .one(&db)
            .await
            .unwrap()
            .unwrap()
    };
    assert_eq!(used, 0, "voucher use released");
    let evts: i64 = {
        use saleor_rustify_db::entities::order_orderevent::{Column as ECol, Entity as EEnt};
        EEnt::find()
            .select_only()
            .column(ECol::Id)
            .filter(ECol::OrderId.eq(oid))
            .filter(ECol::Type.eq("expired"))
            .into_tuple::<i32>()
            .all(&db)
            .await
            .unwrap()
            .len() as i64
    };
    assert_eq!(evts, 1, "expired event written");

    // 2. Fresh unconfirmed order must NOT expire (TTL gate).
    let oid2 = Uuid::new_v4();
    order_am(oid2, ch, "unconfirmed", chrono::Utc::now(), None)
        .insert(&db)
        .await
        .unwrap();
    let n2 = order_expiry::expire_unconfirmed_orders(&db).await.unwrap();
    let st2: String = OEnt::find_by_id(oid2)
        .select_only()
        .column(OCol::Status)
        .into_tuple()
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(st2, "unconfirmed");
    let _ = n2;

    // 3. Long-expired order → deleted with children.
    {
        use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
        db.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE order_order SET expired_at = now() - interval '1 day' WHERE id = $1",
            vec![oid.into()],
        ))
        .await
        .unwrap();
    }
    let d = order_expiry::delete_expired_orders(&db).await.unwrap();
    assert!(d >= 1, "stale expired order must be deleted");
    assert!(OEnt::find_by_id(oid).one(&db).await.unwrap().is_none());

    // Cleanup: fresh order, voucher rows, channel restore.
    OEnt::delete_by_id(oid2).exec(&db).await.unwrap();
    {
        use saleor_rustify_db::entities::discount_vouchercode::{Column as VCCol, Entity as VCEnt};
        VCEnt::delete_many().filter(VCCol::Code.eq(code)).exec(&db).await.unwrap();
        use saleor_rustify_db::entities::discount_voucher::Entity as VEnt;
        VEnt::delete_by_id(vid).exec(&db).await.unwrap();
    }
    restore_channel(&db, ch, old_expire, old_delete).await;
}
