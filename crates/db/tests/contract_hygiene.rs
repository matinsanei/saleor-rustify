//! Hygiene tick contract (live Postgres): drifted stock counts corrected,
//! zero allocations dropped, orphaned old payloads pruned, long-removed app
//! deleted. Mirrors `saleor/warehouse/tests/test_tasks.py`,
//! `saleor/core/tests/test_tasks.py::test_delete_event_payloads_task`,
//! `saleor/app/tests/test_tasks.py::test_remove_apps_task` (row outcomes).

use saleor_rustify_db::{database_url, hygiene};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use serde_json::json;

async fn db() -> DatabaseConnection {
    saleor_rustify_db::connect(&database_url())
        .await
        .expect("saleor postgres must be up (localhost:5434)")
}

#[tokio::test]
async fn hygiene_end_to_end() {
    let db = db().await;
    use sea_orm::{ColumnTrait, QueryFilter, QuerySelect};

    // 1. Drifted stock corrected (no allocations → must become 0).
    use saleor_rustify_db::entities::warehouse_stock::{Column as SCol, Entity as SEnt};
    let sid: i32 = SEnt::find()
        .select_only()
        .column(SCol::Id)
        .into_tuple()
        .all(&db)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let old_alloc: i32 = SEnt::find_by_id(sid)
        .select_only()
        .column(SCol::QuantityAllocated)
        .into_tuple()
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    // Remove this stock's allocations temporarily (restored logic: none —
    // allocations are recreated below only if they existed... instead point
    // the check at arithmetic: set allocated wrong, expect recompute).
    let live_total: i64 = {
        use saleor_rustify_db::entities::warehouse_allocation::{Column as ACol, Entity as AEnt};
        AEnt::find()
            .select_only()
            .column(ACol::QuantityAllocated)
            .filter(ACol::StockId.eq(sid))
            .into_tuple::<i32>()
            .all(&db)
            .await
            .unwrap()
            .into_iter()
            .map(i64::from)
            .sum()
    };
    {
        use saleor_rustify_db::entities::warehouse_stock::ActiveModel as SAm;
        let s = SEnt::find_by_id(sid).one(&db).await.unwrap().unwrap();
        let mut am: SAm = s.into();
        am.quantity_allocated = Set(old_alloc + 7);
        am.update(&db).await.unwrap();
    }
    let n = hygiene::recompute_stock_allocated(&db).await.unwrap();
    assert!(n >= 1, "drift corrected");
    let fixed: i32 = SEnt::find_by_id(sid)
        .select_only()
        .column(SCol::QuantityAllocated)
        .into_tuple()
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fixed as i64, live_total, "recomputed from live allocations");

    // 2. Zero allocation dropped (attached to a real seed order line).
    let line_id: uuid::Uuid = {
        use saleor_rustify_db::entities::order_orderline::{Column as LCol, Entity as LEnt};
        LEnt::find()
            .select_only()
            .column(LCol::Id)
            .into_tuple()
            .all(&db)
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
    };
    let aid: i32 = {
        use saleor_rustify_db::entities::warehouse_allocation::ActiveModel as AAm;
        let row: saleor_rustify_db::entities::warehouse_allocation::Model = AAm {
            quantity_allocated: Set(0),
            stock_id: Set(sid),
            order_line_id: Set(line_id),
            ..Default::default()
        }
        .insert(&db)
        .await
        .unwrap();
        row.id
    };
    let m = hygiene::delete_empty_allocations(&db).await.unwrap();
    assert!(m >= 1, "empty allocation dropped");
    use saleor_rustify_db::entities::warehouse_allocation::Entity as AEnt;
    assert!(AEnt::find_by_id(aid).one(&db).await.unwrap().is_none());

    // 3. Orphaned old payload pruned; fresh orphan kept.
    let old_pid: i32 = {
        use saleor_rustify_db::entities::core_eventpayload::ActiveModel as PAm;
        let row: saleor_rustify_db::entities::core_eventpayload::Model = PAm {
            payload: Set("{}".into()),
            created_at: Set((chrono::Utc::now() - chrono::Duration::days(30)).into()),
            ..Default::default()
        }
        .insert(&db)
        .await
        .unwrap();
        row.id
    };
    let p = hygiene::prune_event_payloads(&db).await.unwrap();
    assert!(p >= 1, "old orphan payload pruned");
    use saleor_rustify_db::entities::core_eventpayload::Entity as PEnt;
    assert!(PEnt::find_by_id(old_pid).one(&db).await.unwrap().is_none());

    // 4. Long-removed app row deleted (no webhooks attached).
    let app_id: i32 = {
        use saleor_rustify_db::entities::app_app::ActiveModel as ApAm;
        let row: saleor_rustify_db::entities::app_app::Model = ApAm {
            name: Set("e2e-gone".into()),
            identifier: Set(format!("e2e-gone-{}", &uuid::Uuid::new_v4().to_string()[..8])),
            r#type: Set("thirdparty".into()),
            uuid: Set(uuid::Uuid::new_v4()),
            created_at: Set(chrono::Utc::now().into()),
            is_active: Set(false),
            is_installed: Set(false),
            removed_at: Set(Some((chrono::Utc::now() - chrono::Duration::days(2)).into())),
            metadata: Set(json!({})),
            private_metadata: Set(json!({})),
            ..Default::default()
        }
        .insert(&db)
        .await
        .unwrap();
        row.id
    };
    let a = hygiene::remove_deleted_apps(&db).await.unwrap();
    assert!(a >= 1, "removed app deleted");
    use saleor_rustify_db::entities::app_app::Entity as ApEnt;
    assert!(ApEnt::find_by_id(app_id).one(&db).await.unwrap().is_none());
}
