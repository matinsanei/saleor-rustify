//! End-to-end thumbnail contract (live Postgres + local media dir):
//! seed product → media row over a generated PNG → `GET /thumbnail/`
//! renders a 302 to a real rendition → second call serves the cached row →
//! `/image/` redirects to the original. Mirrors
//! `saleor/thumbnail/tests/test_views.py` status mapping (302/404/503).

use axum::http::StatusCode;
use saleor_rustify_db::{catalog_writes, database_url};
use sea_orm::DatabaseConnection;

async fn db() -> DatabaseConnection {
    saleor_rustify_db::connect(&database_url())
        .await
        .expect("saleor postgres must be up (localhost:5434)")
}

fn png_bytes(w: u32, h: u32) -> Vec<u8> {
    use image::ImageEncoder;
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
    });
    let mut buf = Vec::new();
    image::codecs::png::PngEncoder::new(&mut buf)
        .write_image(&img, w, h, image::ExtendedColorType::Rgb8)
        .unwrap();
    buf
}

/// ALL env mutation lives in this ONE test (process env is global; the lib
/// test binary has its own env-touching test, but integration binaries are
/// separate processes so they can't race each other).
#[tokio::test]
async fn thumbnail_end_to_end() {
    for v in [
        "AWS_S3_ENDPOINT_URL",
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_MEDIA_BUCKET_NAME",
        "AWS_STORAGE_BUCKET_NAME",
        "AWS_MEDIA_CUSTOM_DOMAIN",
        "SALEOR_MEDIA_URL",
        "SALEOR_MEDIA_DIR",
    ] {
        std::env::remove_var(v);
    }
    let dir = std::env::temp_dir().join("rustify-thumbnail-e2e");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("products")).unwrap();
    std::env::set_var("SALEOR_MEDIA_DIR", &dir);

    let db = db().await;
    // Any seeded product; direct query avoids depending on id shapes.
    let pid: Option<i32> = {
        use saleor_rustify_db::entities::product_product::{Column as PCol, Entity as PEnt};
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
        PEnt::find()
            .select_only()
            .column(PCol::Id)
            .filter(PCol::Name.is_not_null())
            .order_by_desc(PCol::Id)
            .into_tuple::<i32>()
            .one(&db)
            .await
            .unwrap()
    };
    let pid = pid.expect("seed products must exist");

    // Source: 200x100 PNG on local disk (S3 unset → local mode).
    let src = png_bytes(200, 100);
    std::fs::write(dir.join("products/e2e_src.png"), &src).unwrap();
    let mid = catalog_writes::create_media(
        &db,
        pid,
        "e2e",
        Some("products/e2e_src.png".into()),
        None,
        None, // legacy default: image present → IMAGE
    )
    .await
    .unwrap();
    let gid = saleor_rustify_graphql::common::gid("ProductMedia", mid);

    // 1. First call renders (miss → encode → store → 302).
    let (st, h, _) = saleor_rustify_graphql::thumbnail::serve_thumbnail(&db, &gid, "64", None).await;
    assert_eq!(st, StatusCode::FOUND);
    let loc = h["location"].to_str().unwrap().to_string();
    assert!(
        loc.ends_with("thumbnails/products/e2e_src_thumbnail_64.png"),
        "{loc}"
    );
    let rend = tokio::fs::read(dir.join("thumbnails/products/e2e_src_thumbnail_64.png"))
        .await
        .unwrap();
    let got = image::load_from_memory(&rend).unwrap();
    // Fit-inside box: 200x100 → 64x32 (aspect preserved, never upscaled).
    assert_eq!((got.width(), got.height()), (64, 32));

    // 2. Cached row served (delete the file → still 302 from the row).
    tokio::fs::remove_file(dir.join("thumbnails/products/e2e_src_thumbnail_64.png"))
        .await
        .unwrap();
    let (st2, h2, _) =
        saleor_rustify_graphql::thumbnail::serve_thumbnail(&db, &gid, "60", None).await;
    assert_eq!(st2, StatusCode::FOUND);
    assert_eq!(h2["location"].to_str().unwrap(), loc); // 60 → closest 64

    // 3. WebP rendition.
    let (stw, hw, _) =
        saleor_rustify_graphql::thumbnail::serve_thumbnail(&db, &gid, "32", Some("webp")).await;
    assert_eq!(stw, StatusCode::FOUND);
    assert!(hw["location"].to_str().unwrap().ends_with("_thumbnail_32.webp"));

    // 4. Original redirect.
    let (sto, ho, _) = saleor_rustify_graphql::thumbnail::serve_original(&db, &gid).await;
    assert_eq!(sto, StatusCode::FOUND);
    assert!(ho["location"].to_str().unwrap().ends_with("products/e2e_src.png"));

    // 5. Status mapping parity.
    let (bad, _, _) =
        saleor_rustify_graphql::thumbnail::serve_thumbnail(&db, "not-a-gid", "64", None).await;
    assert_eq!(bad, StatusCode::NOT_FOUND);
    let (badsize, _, _) =
        saleor_rustify_graphql::thumbnail::serve_thumbnail(&db, &gid, "xx", None).await;
    assert_eq!(badsize, StatusCode::NOT_FOUND);
    let (badfmt, _, _) =
        saleor_rustify_graphql::thumbnail::serve_thumbnail(&db, &gid, "64", Some("tiff")).await;
    assert_eq!(badfmt, StatusCode::NOT_FOUND);
    let (explicit_original, _, _) =
        saleor_rustify_graphql::thumbnail::serve_thumbnail(&db, &gid, "64", Some("original")).await;
    assert_eq!(explicit_original, StatusCode::NOT_FOUND); // 404 upstream too
    let (avif, _, _) =
        saleor_rustify_graphql::thumbnail::serve_thumbnail(&db, &gid, "64", Some("avif")).await;
    assert_eq!(avif, StatusCode::BAD_REQUEST); // documented encoder gap

    // 6. Pending remote (IMAGE + external_url, image empty → 503).
    let pend = catalog_writes::create_media(
        &db,
        pid,
        "pend",
        None,
        Some("https://example.com/x.jpg".into()),
        Some("IMAGE"), // Django pre-fetch row state for image URLs
    )
    .await
    .unwrap();
    let pgid = saleor_rustify_graphql::common::gid("ProductMedia", pend);
    let (stp, hp, _) =
        saleor_rustify_graphql::thumbnail::serve_thumbnail(&db, &pgid, "64", None).await;
    assert_eq!(stp, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(hp["retry-after"].to_str().unwrap(), "60");

    // Cleanup rows (files live in temp dir).
    catalog_writes::delete_media(&db, pend).await.unwrap();
    catalog_writes::delete_media(&db, mid).await.unwrap();
    for v in ["SALEOR_MEDIA_DIR"] {
        std::env::remove_var(v);
    }
    let _ = std::fs::remove_dir_all(&dir);
}
