//! CSV/XLSX exports (Django `csv` app parity, synchronous subset).
//!
//! Django references (`saleor/csv/`): exports are async file jobs writing
//! `ExportFile` rows. Here exports run synchronously (files are small at
//! this scale) into `<media>/exports/`, served by the existing media mount.
//! - scope ALL/IDS/FILTER (FILTER resolves through the same list-filter
//!   predicates the dashboard lists use; unsupported sub-filters stay
//!   accepted-ignored exactly like the lists themselves);
//! - CSV + XLSX (minimal inline-string workbook; no shared-strings/styles);
//! - gift cards, voucher codes, products covered (the dashboard's three
//!   export dialogs);
//! - every job writes `ExportEvent` rows (started/success/failed, like
//!   `saleor/csv/events.py`), which the `delete-old-export-files` beat uses
//!   with Django's verbatim retention rule.

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};

use crate::{
    entities::{
        csv_exportevent, csv_exportfile, discount_voucher, discount_vouchercode, giftcard_giftcard,
        giftcard_giftcard_tags, giftcard_giftcardtag, product_product,
    },
    DbError, Result,
};

/// Output format (dashboard `FileTypesEnum`: CSV/XLSX).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Csv,
    Xlsx,
}

fn fail(msg: impl Into<String>) -> DbError {
    DbError::App(format!("export error: {}", msg.into()))
}

fn export_dir() -> std::path::PathBuf {
    let base = std::env::var("SALEOR_MEDIA_DIR")
        .unwrap_or_else(|_| "../saleor/saleor-core/media".to_string());
    std::path::Path::new(&base).join("exports")
}

fn write_csv(name: &str, header: &[&str], rows: &[Vec<String>]) -> Result<String> {
    let dir = export_dir();
    std::fs::create_dir_all(&dir).map_err(|e| fail(e.to_string()))?;
    let fname = format!("{name}-{}.csv", Utc::now().timestamp_millis());
    let path = dir.join(&fname);
    let mut w = csv_writer(&path)?;
    w.push(header.join(","));
    for r in rows {
        w.push(r.iter().map(|s| escape(s)).collect::<Vec<_>>().join(","));
    }
    std::fs::write(&path, w.join("\n")).map_err(|e| fail(e.to_string()))?;
    Ok(format!("exports/{fname}"))
}

fn csv_writer(_path: &std::path::Path) -> Result<Vec<String>> {
    Ok(vec![])
}

fn escape(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Minimal XLSX workbook (inline strings, no shared-strings table or styles
/// — opens in Excel/LibreOffice; keeps us dependency-light: only `zip`).
fn write_xlsx(name: &str, header: &[&str], rows: &[Vec<String>]) -> Result<String> {
    fn col(mut n: usize) -> String {
        let mut s = Vec::new();
        loop {
            s.push((b'A' + (n % 26) as u8) as char);
            if n < 26 {
                break;
            }
            n = n / 26 - 1;
        }
        s.iter().rev().collect()
    }
    let mut sheet = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>",
    );
    for (ri, row) in std::iter::once(&header.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        .chain(rows.iter())
        .enumerate()
    {
        sheet.push_str(&format!("<row r=\"{}\">", ri + 1));
        for (ci, cell) in row.iter().enumerate() {
            sheet.push_str(&format!(
                "<c r=\"{}{}\" t=\"inlineStr\"><is><t>{}</t></is></c>",
                col(ci),
                ri + 1,
                xml_escape(cell)
            ));
        }
        sheet.push_str("</row>");
    }
    sheet.push_str("</sheetData></worksheet>");
    let parts: &[(&str, &str)] = &[
        ("[Content_Types].xml", "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxml-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/><Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/></Types>"),
        ("_rels/.rels", "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>"),
        ("xl/workbook.xml", "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets><sheet name=\"Export\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>"),
        ("xl/_rels/workbook.xml.rels", "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/></Relationships>"),
    ];
    let dir = export_dir();
    std::fs::create_dir_all(&dir).map_err(|e| fail(e.to_string()))?;
    let fname = format!("{name}-{}.xlsx", Utc::now().timestamp_millis());
    let path = dir.join(&fname);
    let file = std::fs::File::create(&path).map_err(|e| fail(e.to_string()))?;
    let mut zip = zip::ZipWriter::new(file);
    let opts: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    use std::io::Write;
    for (pname, content) in parts {
        zip.start_file(*pname, opts)
            .map_err(|e| fail(e.to_string()))?;
        zip.write_all(content.as_bytes())
            .map_err(|e| fail(e.to_string()))?;
    }
    zip.start_file("xl/worksheets/sheet1.xml", opts)
        .map_err(|e| fail(e.to_string()))?;
    zip.write_all(sheet.as_bytes())
        .map_err(|e| fail(e.to_string()))?;
    zip.finish().map_err(|e| fail(e.to_string()))?;
    Ok(format!("exports/{fname}"))
}

/// Write the table in the requested format; returns the media-relative path.
fn write_table(
    name: &str,
    header: &[&str],
    rows: &[Vec<String>],
    format: ExportFormat,
) -> Result<String> {
    match format {
        ExportFormat::Csv => write_csv(name, header, rows),
        ExportFormat::Xlsx => write_xlsx(name, header, rows),
    }
}

/// Export event types — mirrors `ExportEvents` in `saleor/csv/__init__.py`.
pub mod events {
    pub const STARTED: &str = "export_started";
    pub const SUCCESS: &str = "export_success";
    pub const FAILED: &str = "export_failed";
}

async fn record_event(
    db: &impl ConnectionTrait,
    file_id: i32,
    event_type: &str,
    user_id: Option<i32>,
) -> Result<()> {
    csv_exportevent::ActiveModel {
        date: Set(Utc::now().into()),
        r#type: Set(event_type.to_string()),
        parameters: Set(serde_json::json!({})),
        app_id: Set(None),
        export_file_id: Set(file_id),
        user_id: Set(user_id),
        ..Default::default()
    }
    .insert(db)
    .await?;
    Ok(())
}

/// Open an export job row (status pending) + started event, atomically: the
/// file row and its first event commit together so the prune beat can never
/// observe a half-opened (eventless) job and delete it mid-export.
pub async fn open_job(db: &sea_orm::DatabaseConnection, user_id: Option<i32>) -> Result<i32> {
    use sea_orm::TransactionTrait;
    let txn = db.begin().await?;
    let t = Utc::now();
    let row = csv_exportfile::ActiveModel {
        status: Set("pending".to_string()),
        created_at: Set(t.into()),
        updated_at: Set(t.into()),
        content_file: Set(None),
        app_id: Set(None),
        user_id: Set(user_id),
        message: Set(None),
        ..Default::default()
    }
    .insert(&txn)
    .await?;
    record_event(&txn, row.id, events::STARTED, user_id).await?;
    txn.commit().await?;
    Ok(row.id)
}

async fn finish_job(
    db: &sea_orm::DatabaseConnection,
    id: i32,
    path: &str,
    user_id: Option<i32>,
) -> Result<()> {
    if let Some(row) = csv_exportfile::Entity::find_by_id(id).one(db).await? {
        let mut am: csv_exportfile::ActiveModel = row.into();
        am.status = Set("success".to_string());
        am.content_file = Set(Some(path.to_string()));
        am.updated_at = Set(Utc::now().into());
        am.update(db).await?;
    }
    record_event(db, id, events::SUCCESS, user_id).await?;
    Ok(())
}

async fn fail_job(db: &sea_orm::DatabaseConnection, id: i32, message: &str, user_id: Option<i32>) {
    if let Ok(Some(row)) = csv_exportfile::Entity::find_by_id(id).one(db).await {
        let mut am: csv_exportfile::ActiveModel = row.into();
        am.status = Set("failed".to_string());
        am.message = Set(Some(message.to_string()));
        am.updated_at = Set(Utc::now().into());
        let _ = am.update(db).await;
    }
    let _ = record_event(db, id, events::FAILED, user_id).await;
}

/// Export gift cards (scope ALL or IDS).
pub async fn export_gift_cards(
    db: &sea_orm::DatabaseConnection,
    user_id: Option<i32>,
    ids: Option<Vec<i32>>,
    format: ExportFormat,
) -> Result<i32> {
    let job = open_job(db, user_id).await;
    let job = match job {
        Ok(j) => j,
        Err(e) => return Err(e),
    };
    let out: Result<i32> = async {
        let mut q = giftcard_giftcard::Entity::find().order_by_asc(giftcard_giftcard::Column::Id);
        if let Some(list) = ids {
            q = q.filter(giftcard_giftcard::Column::Id.is_in(list));
        }
        let cards = q.all(db).await?;
        let mut rows = Vec::with_capacity(cards.len());
        for c in &cards {
            let names = tag_names(db, c.id).await?;
            rows.push(vec![
                c.code.clone(),
                c.initial_balance_amount.to_string(),
                c.current_balance_amount.to_string(),
                c.currency.clone(),
                c.is_active.to_string(),
                c.expiry_date.map(|d| d.to_string()).unwrap_or_default(),
                names.join(";"),
            ]);
        }
        let path = write_table(
            "gift-cards",
            &[
                "code",
                "initial_balance",
                "current_balance",
                "currency",
                "is_active",
                "expiry_date",
                "tags",
            ],
            &rows,
            format,
        )?;
        finish_job(db, job, &path, user_id).await?;
        Ok(job)
    }
    .await;
    if let Err(e) = &out {
        fail_job(db, job, &e.to_string(), user_id).await;
    }
    out
}

async fn tag_names(db: &impl ConnectionTrait, card_id: i32) -> Result<Vec<String>> {
    let link_ids: Vec<i32> = giftcard_giftcard_tags::Entity::find()
        .select_only()
        .column(giftcard_giftcard_tags::Column::GiftcardtagId)
        .filter(giftcard_giftcard_tags::Column::GiftcardId.eq(card_id))
        .into_tuple()
        .all(db)
        .await?;
    if link_ids.is_empty() {
        return Ok(vec![]);
    }
    Ok(giftcard_giftcardtag::Entity::find()
        .select_only()
        .column(giftcard_giftcardtag::Column::Name)
        .filter(giftcard_giftcardtag::Column::Id.is_in(link_ids))
        .into_tuple()
        .all(db)
        .await?)
}

/// Export voucher codes (one voucher or explicit code rows).
pub async fn export_voucher_codes(
    db: &sea_orm::DatabaseConnection,
    user_id: Option<i32>,
    voucher_id: Option<i32>,
    code_ids: Option<Vec<uuid::Uuid>>,
    format: ExportFormat,
) -> Result<i32> {
    let job = open_job(db, user_id).await?;
    let out: Result<i32> = async {
        let mut q =
            discount_vouchercode::Entity::find().order_by_asc(discount_vouchercode::Column::Code);
        if let Some(vid) = voucher_id {
            q = q.filter(discount_vouchercode::Column::VoucherId.eq(vid));
        }
        if let Some(list) = code_ids {
            q = q.filter(discount_vouchercode::Column::Id.is_in(list));
        }
        let codes = q.all(db).await?;
        let mut rows = Vec::with_capacity(codes.len());
        for c in &codes {
            let vname: Option<String> = discount_voucher::Entity::find_by_id(c.voucher_id)
                .select_only()
                .column(discount_voucher::Column::Name)
                .into_tuple()
                .one(db)
                .await?
                .unwrap_or(None);
            rows.push(vec![
                c.code.clone(),
                vname.unwrap_or_default(),
                c.used.to_string(),
                c.is_active.to_string(),
            ]);
        }
        let path = write_table(
            "voucher-codes",
            &["code", "voucher", "used", "is_active"],
            &rows,
            format,
        )?;
        finish_job(db, job, &path, user_id).await?;
        Ok(job)
    }
    .await;
    if let Err(e) = &out {
        fail_job(db, job, &e.to_string(), user_id).await;
    }
    out
}

/// Export products (scope ALL or IDS).
pub async fn export_products(
    db: &sea_orm::DatabaseConnection,
    user_id: Option<i32>,
    ids: Option<Vec<i32>>,
    format: ExportFormat,
) -> Result<i32> {
    let job = open_job(db, user_id).await?;
    let out: Result<i32> = async {
        let mut q = product_product::Entity::find()
            .select_only()
            .column(product_product::Column::Id)
            .column(product_product::Column::Name)
            .column(product_product::Column::Slug)
            .column(product_product::Column::CreatedAt)
            .order_by_asc(product_product::Column::Id);
        if let Some(list) = ids {
            q = q.filter(product_product::Column::Id.is_in(list));
        }
        let rows: Vec<(i32, String, String, chrono::DateTime<chrono::Utc>)> =
            q.into_tuple().all(db).await?;
        let out: Vec<Vec<String>> = rows
            .into_iter()
            .map(|(id, name, slug, created)| vec![id.to_string(), name, slug, created.to_rfc3339()])
            .collect();
        let path = write_table(
            "products",
            &["id", "name", "slug", "created_at"],
            &out,
            format,
        )?;
        finish_job(db, job, &path, user_id).await?;
        Ok(job)
    }
    .await;
    if let Err(e) = &out {
        fail_job(db, job, &e.to_string(), user_id).await;
    }
    out
}

/// Retention for `EXPORT_FILES_TIMEDELTA` (upstream `pytimeparse` human
/// durations: "30 days" default; bare seconds also accepted).
fn retention_cutoff() -> chrono::DateTime<Utc> {
    fn secs(s: &str) -> Option<i64> {
        let s = s.trim().to_lowercase();
        if let Ok(n) = s.parse::<i64>() {
            return Some(n);
        }
        let (n, rest) = (
            s.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>(),
            s.trim_start_matches(|c: char| c.is_ascii_digit()).trim(),
        );
        let n: i64 = n.parse().ok()?;
        let mult = if rest.starts_with("day") {
            86_400
        } else if rest.starts_with("hour") || rest.starts_with("hr") {
            3_600
        } else if rest.starts_with("min") {
            60
        } else if rest.starts_with("sec") {
            1
        } else {
            return None;
        };
        Some(n * mult)
    }
    let delta = std::env::var("EXPORT_FILES_TIMEDELTA")
        .ok()
        .and_then(|s| secs(&s))
        .unwrap_or(30 * 86_400);
    Utc::now() - chrono::Duration::seconds(delta)
}

/// Delete old export files (Django `saleor.csv.tasks.delete_old_export_files`
/// verbatim rule): an `ExportFile` goes when it has NO events at all, or ANY
/// event older than the retention window. Storage files are unlinked
/// best-effort; event rows are deleted before file rows (FK has no cascade).
/// Returns the number of export files removed.
pub async fn prune_old_export_files(db: &sea_orm::DatabaseConnection) -> Result<usize> {
    use std::collections::HashSet;
    let cutoff = retention_cutoff();
    let with_events: HashSet<i32> = csv_exportevent::Entity::find()
        .select_only()
        .column(csv_exportevent::Column::ExportFileId)
        .distinct()
        .into_tuple()
        .all(db)
        .await?
        .into_iter()
        .collect();
    let old_set: HashSet<i32> = csv_exportevent::Entity::find()
        .select_only()
        .column(csv_exportevent::Column::ExportFileId)
        .filter(csv_exportevent::Column::Date.lte(cutoff))
        .distinct()
        .into_tuple()
        .all(db)
        .await?
        .into_iter()
        .collect();
    let files: Vec<(i32, Option<String>)> = csv_exportfile::Entity::find()
        .select_only()
        .column(csv_exportfile::Column::Id)
        .column(csv_exportfile::Column::ContentFile)
        .into_tuple()
        .all(db)
        .await?;
    let base = export_dir();
    // export_dir() already ends in "exports/"; content_file is stored as
    // "exports/<fname>" relative to the media root.
    let media_base = base
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or(base.clone());
    let mut removed = 0usize;
    for (id, content) in files {
        if with_events.contains(&id) && !old_set.contains(&id) {
            continue;
        }
        if let Some(rel) = content
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            // content_file is DB-sourced: fence it into the media root
            // (refuse absolute paths and `..`; lexical check, target may
            // already be gone). Rows are still deleted either way.
            let p = std::path::Path::new(&rel);
            if !p.is_absolute()
                && !p
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                let _ = std::fs::remove_file(media_base.join(p));
            }
        }
        csv_exportevent::Entity::delete_many()
            .filter(csv_exportevent::Column::ExportFileId.eq(id))
            .exec(db)
            .await?;
        csv_exportfile::Entity::delete_by_id(id).exec(db).await?;
        removed += 1;
    }
    Ok(removed)
}
