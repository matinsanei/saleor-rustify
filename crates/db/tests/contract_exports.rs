//! CSV/XLSX exports: gift cards, voucher codes, products (+ FILTER scope,
//! events, prune beat).

use saleor_rustify_db::{database_url, exports};
use sea_orm::DatabaseConnection;

async fn db() -> DatabaseConnection {
    saleor_rustify_db::connect(&database_url())
        .await
        .expect("saleor postgres must be up (localhost:5434)")
}

fn media_base() -> std::path::PathBuf {
    std::path::PathBuf::from(
        std::env::var("SALEOR_MEDIA_DIR")
            .unwrap_or_else(|_| "../saleor/saleor-core/media".to_string()),
    )
}

/// Serializes this file's tests: they share the global csv_exportfile table
/// and two of them run the retention prune, which would otherwise steal each
/// other's rows mid-assertion (same pattern as `stock_guard` in the
/// promotions suite).
fn exports_guard() -> fs2::FileLockGuard {
    fs2::FileLockGuard::new()
}

mod fs2 {
    pub struct FileLockGuard {
        _f: std::fs::File,
    }
    impl FileLockGuard {
        pub fn new() -> Self {
            use fs2::FileExt;
            let f = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .open("/tmp/rustify-exports.lock")
                .expect("exports lock");
            f.lock_exclusive().expect("exports lock");
            Self { _f: f }
        }
    }
}

async fn cleanup_job(db: &DatabaseConnection, job: i32, expect_ext: &str) {
    use saleor_rustify_db::entities::{csv_exportevent, csv_exportfile};
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let row = csv_exportfile::Entity::find_by_id(job)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, "success");
    let path = row.content_file.clone().expect("export must store a path");
    assert!(path.ends_with(expect_ext), "unexpected path {path}");
    assert!(media_base().join(&path).exists(), "file missing: {path}");
    // Every export writes started + success events (the prune beat's input).
    let n_events = csv_exportevent::Entity::find()
        .filter(csv_exportevent::Column::ExportFileId.eq(job))
        .all(db)
        .await
        .unwrap()
        .len();
    assert!(
        n_events >= 2,
        "expected started+success events, got {n_events}"
    );
    let _ = std::fs::remove_file(media_base().join(&path));
    // Atomic teardown: events+file vanish together so a concurrent prune
    // never observes a half-deleted (eventless) row.
    use sea_orm::TransactionTrait;
    let txn = db.begin().await.unwrap();
    csv_exportevent::Entity::delete_many()
        .filter(csv_exportevent::Column::ExportFileId.eq(job))
        .exec(&txn)
        .await
        .unwrap();
    use sea_orm::ActiveModelTrait;
    let am: csv_exportfile::ActiveModel = row.into();
    am.delete(&txn).await.unwrap();
    txn.commit().await.unwrap();
}

#[tokio::test]
async fn export_jobs_produce_files() {
    use exports::ExportFormat::Csv;
    let db = db().await;
    let _guard = exports_guard();
    let g = exports::export_gift_cards(&db, None, None, Csv)
        .await
        .unwrap();
    let v = exports::export_voucher_codes(&db, None, None, None, Csv)
        .await
        .unwrap();
    let p = exports::export_products(&db, None, None, Csv)
        .await
        .unwrap();
    for job in [g, v, p] {
        cleanup_job(&db, job, ".csv").await;
    }
}

#[tokio::test]
async fn export_xlsx_produces_workbook() {
    use exports::ExportFormat::Xlsx;
    let db = db().await;
    let _guard = exports_guard();
    let p = exports::export_products(&db, None, None, Xlsx)
        .await
        .unwrap();
    use saleor_rustify_db::entities::csv_exportfile;
    use sea_orm::EntityTrait;
    let row = csv_exportfile::Entity::find_by_id(p)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let path = row.content_file.clone().unwrap();
    assert!(path.ends_with(".xlsx"));
    let bytes = std::fs::read(media_base().join(&path)).unwrap();
    assert!(bytes.starts_with(b"PK"), "xlsx must be a zip");
    cleanup_job(&db, p, ".xlsx").await;
}

#[tokio::test]
async fn export_filter_scopes_narrow() {
    use exports::ExportFormat::Csv;
    let db = db().await;
    let _guard = exports_guard();
    // Gift cards: impossible code narrows to zero rows (header only).
    let f = saleor_rustify_db::giftcards::GiftCardListFilter {
        code_like: Some("no-such-card-zzz".into()),
        ..Default::default()
    };
    let ids = saleor_rustify_db::giftcards::giftcard_ids_filtered(&db, &f)
        .await
        .unwrap();
    assert!(ids.is_empty());
    let g = exports::export_gift_cards(&db, None, Some(ids), Csv)
        .await
        .unwrap();
    use saleor_rustify_db::entities::csv_exportfile;
    use sea_orm::EntityTrait;
    let row = csv_exportfile::Entity::find_by_id(g)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let text =
        std::fs::read_to_string(media_base().join(row.content_file.clone().unwrap())).unwrap();
    assert_eq!(text.lines().count(), 1, "header only, got: {text}");
    cleanup_job(&db, g, ".csv").await;
    // Products: same shape via the list-filter path.
    let pf = saleor_rustify_db::catalog::ProductListFilter {
        search: Some("no-such-product-zzz".into()),
        ..Default::default()
    };
    let pids = saleor_rustify_db::catalog::product_ids_filtered(&db, "default-channel", &pf)
        .await
        .unwrap();
    assert!(pids.is_empty());
}

#[tokio::test]
async fn prune_old_export_files_keeps_fresh() {
    use exports::ExportFormat::Csv;
    let db = db().await;
    let _guard = exports_guard();
    let p = exports::export_products(&db, None, None, Csv)
        .await
        .unwrap();
    // Fresh job has recent events → prune must keep it.
    let kept = exports::prune_old_export_files(&db).await.unwrap();
    use saleor_rustify_db::entities::csv_exportfile;
    use sea_orm::EntityTrait;
    assert!(csv_exportfile::Entity::find_by_id(p)
        .one(&db)
        .await
        .unwrap()
        .is_some());
    let _ = kept;
    cleanup_job(&db, p, ".csv").await;
}

#[tokio::test]
async fn prune_old_export_files_deletes_stale() {
    use exports::ExportFormat::Csv;
    use saleor_rustify_db::entities::{csv_exportevent, csv_exportfile};
    use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
    let db = db().await;
    let _guard = exports_guard();
    let p = exports::export_products(&db, None, None, Csv)
        .await
        .unwrap();
    let path = csv_exportfile::Entity::find_by_id(p)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .content_file
        .unwrap();
    assert!(media_base().join(&path).exists());
    // Hostile content_file values (DB-sourced): prune must delete their ROWS
    // but never unlink outside the media dir.
    let outside = std::env::temp_dir().join("rustify-prune-guard-probe.txt");
    std::fs::write(&outside, b"do not delete").unwrap();
    let dotdot = media_base().join("rustify-prune-guard-dotdot.txt");
    std::fs::write(&dotdot, b"do not delete").unwrap();
    let mut hostile_ids = Vec::new();
    for hostile in [
        outside.to_str().unwrap().to_string(),
        "../rustify-prune-guard-dotdot.txt".to_string(),
    ] {
        // Atomic file+event insert: a concurrent prune must never observe a
        // half-opened (eventless) row and delete it mid-setup.
        use sea_orm::TransactionTrait;
        let txn = db.begin().await.unwrap();
        let t = chrono::Utc::now();
        let row = csv_exportfile::ActiveModel {
            status: Set("success".to_string()),
            created_at: Set(t.into()),
            updated_at: Set(t.into()),
            content_file: Set(Some(hostile)),
            app_id: Set(None),
            user_id: Set(None),
            message: Set(None),
            ..Default::default()
        }
        .insert(&txn)
        .await
        .unwrap();
        csv_exportevent::ActiveModel {
            date: Set((chrono::Utc::now() - chrono::Duration::days(31)).into()),
            r#type: Set(saleor_rustify_db::exports::events::SUCCESS.to_string()),
            parameters: Set(serde_json::json!({})),
            app_id: Set(None),
            export_file_id: Set(row.id),
            user_id: Set(None),
            ..Default::default()
        }
        .insert(&txn)
        .await
        .unwrap();
        txn.commit().await.unwrap();
        hostile_ids.push(row.id);
    }
    // Backdate every event past retention (Django rule: ANY old event → file goes).
    use sea_orm::{ConnectionTrait, Statement};
    db.execute(Statement::from_string(
        sea_orm::DatabaseBackend::Postgres,
        format!(
            "UPDATE csv_exportevent SET date = NOW() - INTERVAL '31 days' WHERE export_file_id = {p}"
        ),
    ))
    .await
    .unwrap();
    let n = exports::prune_old_export_files(&db).await.unwrap();
    assert!(n >= 3, "stale file + hostile rows must be pruned");
    assert!(csv_exportfile::Entity::find_by_id(p)
        .one(&db)
        .await
        .unwrap()
        .is_none());
    assert!(csv_exportevent::Entity::find()
        .filter(csv_exportevent::Column::ExportFileId.eq(p))
        .all(&db)
        .await
        .unwrap()
        .is_empty());
    assert!(
        !media_base().join(&path).exists(),
        "storage file must be unlinked"
    );
    for id in hostile_ids {
        assert!(csv_exportfile::Entity::find_by_id(id)
            .one(&db)
            .await
            .unwrap()
            .is_none());
        assert!(csv_exportevent::Entity::find()
            .filter(csv_exportevent::Column::ExportFileId.eq(id))
            .all(&db)
            .await
            .unwrap()
            .is_empty());
    }
    assert!(
        outside.exists(),
        "absolute path outside media root must survive"
    );
    assert!(dotdot.exists(), "'..' path must survive");
    let _ = std::fs::remove_file(&outside);
    let _ = std::fs::remove_file(&dotdot);
}
