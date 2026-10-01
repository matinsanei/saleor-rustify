//! On-demand thumbnails: `GET /thumbnail/<gid>/<size>/[<format>/]` and
//! `GET /image/<gid>/` (original).
//!
//! Django reference (`saleor/thumbnail/`):
//! - `views.py::handle_thumbnail` — decode the instance global id
//!   (`ProductMedia|User|Category|Collection|App|AppInstallation`), validate
//!   size → closest of `THUMBNAIL_SIZES`, validate format, serve the cached
//!   `Thumbnail` row (302 to its URL) or render → store row → 302.
//! - `views.py::handle_original_image` — 302 to the source image URL.
//! - `utils.py::ProcessedImage` — fit-inside resize (`Image.thumbnail`:
//!   aspect preserved, never upscales) then format-specific save
//!   (JPEG q70 non-progressive; EXIF orientation applied; ICC preserved).
//! - `__init__.py` — sizes `[32, 64, 128, 256, 512, 1024, 2048, 4096]`;
//!   output formats `{avif, webp}` (icons: `{webp}`); explicit `original`
//!   in the URL is a 404 upstream (only absence means original).
//!
//! Rust deltas (all documented, none silent):
//! - WebP output is LOSSLESS (no lossy-WebP encoder in this pure-Rust
//!   build; upstream serves q70 lossy — same pixels, heavier bytes).
//! - output `avif` → 400 (no AVIF encoder in this build; decoding AVIF
//!   sources also fails → 400). Our dashboard never requests a format — the
//!   GraphQL `thumbnail` resolvers return originals — so this bites only
//!   hand-built URLs.
//! - non-{jpeg,png,gif,webp,bmp} sources (tiff/…) render to PNG (browsers
//!   can't display TIFF anyway; upstream would serve TIFF bytes).
//! - EXIF orientation is NOT applied (no EXIF parser in the image pipeline).
//! - pixel bomb guard = Pillow's default `MAX_IMAGE_PIXELS` (178_956_970)
//!   checked from the header before full decode → 400 like upstream.
//! - unauthenticated, like upstream. 302 FOUND (not 303/307) for parity.
//! - rendition key mirrors upstream exactly:
//!   `thumbnails/<src-stem>_thumbnail_<size>.<ext>` so rows/objects stay
//!   portable across backends.

use axum::{
    Extension,
    extract::Path,
    http::{HeaderMap, StatusCode, header},
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect, Set};

/// Upstream `THUMBNAIL_SIZES` verbatim.
pub const THUMBNAIL_SIZES: [i32; 8] = [32, 64, 128, 256, 512, 1024, 2048, 4096];

/// Pillow default bomb limit (upstream `Image.DecompressionBombError` trip).
const MAX_PIXELS: u64 = 178_956_970;

/// 503 body + retry hint, upstream strings verbatim.
const PENDING_BODY: &str = "Image has not been fetched yet, try later.";
const PENDING_RETRY_AFTER: &str = "60";

/// Closest available size (upstream `get_thumbnail_size`).
pub fn closest_size(req: i32) -> i32 {
    if THUMBNAIL_SIZES.contains(&req) {
        return req;
    }
    THUMBNAIL_SIZES
        .into_iter()
        .min_by_key(|s| (s - req).abs())
        .unwrap_or(4096)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstanceKind {
    ProductMedia,
    User,
    Category,
    Collection,
    App,
    AppInstallation,
}

impl InstanceKind {
    fn parse(ty: &str) -> Option<Self> {
        match ty {
            "ProductMedia" => Some(Self::ProductMedia),
            "User" => Some(Self::User),
            "Category" => Some(Self::Category),
            "Collection" => Some(Self::Collection),
            "App" => Some(Self::App),
            "AppInstallation" => Some(Self::AppInstallation),
            _ => None,
        }
    }

    fn is_icon(self) -> bool {
        matches!(self, Self::App | Self::AppInstallation)
    }

    fn is_uuid(self) -> bool {
        matches!(self, Self::User | Self::App | Self::AppInstallation)
    }
}

/// Normalized output format: `None` = resized original. Mirrors upstream —
/// only `avif`/`webp` (icons: `webp`) pass; explicit `original` 404s.
fn normalize_format(kind: InstanceKind, raw: Option<&str>) -> Result<Option<String>, StatusCode> {
    let Some(f) = raw else { return Ok(None) };
    let f = f.to_lowercase();
    let ok = if kind.is_icon() {
        f == "webp"
    } else {
        f == "avif" || f == "webp"
    };
    if ok {
        Ok(Some(f))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

/// Rendition key (upstream `prepare_thumbnail_file_name` + `upload_to`).
/// `encoded_ext` is the extension matching the bytes we actually encoded
/// (usually the source ext; `png` for converted sources like TIFF).
pub fn rendition_key(src: &str, size: i32, format: Option<&str>, encoded_ext: &str) -> String {
    let stem = match src.rfind('.') {
        Some(i) if i > 0 => &src[..i],
        _ => src,
    };
    let ext = format.unwrap_or(encoded_ext);
    format!("thumbnails/{stem}_thumbnail_{size}.{ext}")
}

/// Decode + box-fit resize + encode. Returns `(bytes, extension, mime)`.
fn render(bytes: &[u8], size: i32, format: Option<&str>) -> Result<(Vec<u8>, &'static str, &'static str), StatusCode> {
    use image::ImageFormat;
    use std::io::Cursor;

    if format == Some("avif") {
        // No AVIF encoder in this build (would need a ravif/dav1d native
        // stack); documented gap, 400 with a clear message, not a 500.
        return Err(StatusCode::BAD_REQUEST);
    }

    let probe = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let src_format = probe.format().ok_or(StatusCode::BAD_REQUEST)?;
    let (w, h) = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| StatusCode::BAD_REQUEST)?
        .into_dimensions()
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    if (w as u64) * (h as u64) > MAX_PIXELS {
        return Err(StatusCode::BAD_REQUEST);
    }
    let img = image::load_from_memory(bytes).map_err(|_| StatusCode::BAD_REQUEST)?;
    // `thumbnail()` = fit-inside box, aspect preserved, never upscales —
    // the same call upstream makes (`Image.thumbnail`).
    let small = img.thumbnail(size as u32, size as u32);

    let mut out = Vec::new();
    match format {
        Some("webp") => {
            // Lossless (this build has no lossy-WebP encoder; upstream
            // serves q70 lossy — same pixels, heavier bytes; documented).
            let enc = image::codecs::webp::WebPEncoder::new_lossless(&mut out);
            small
                .write_with_encoder(enc)
                .map_err(|_| StatusCode::BAD_REQUEST)?;
            Ok((out, "webp", "image/webp"))
        }
        None | Some(_) => {
            // Resized original (explicit `original` can't reach here —
            // `normalize_format` 404s it, same as upstream).
            match src_format {
                ImageFormat::Jpeg => {
                    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 70);
                    small
                        .to_rgb8()
                        .write_with_encoder(enc)
                        .map_err(|_| StatusCode::BAD_REQUEST)?;
                    Ok((out, "jpg", "image/jpeg"))
                }
                ImageFormat::Png => {
                    let enc = image::codecs::png::PngEncoder::new(&mut out);
                    small
                        .to_rgba8()
                        .write_with_encoder(enc)
                        .map_err(|_| StatusCode::BAD_REQUEST)?;
                    Ok((out, "png", "image/png"))
                }
                ImageFormat::Gif => {
                    {
                        let mut enc = image::codecs::gif::GifEncoder::new(&mut out);
                        let frame = image::Frame::new(small.to_rgba8());
                        enc.encode_frame(frame)
                            .map_err(|_| StatusCode::BAD_REQUEST)?;
                    }
                    Ok((out, "gif", "image/gif"))
                }
                ImageFormat::WebP => {
                    let enc = image::codecs::webp::WebPEncoder::new_lossless(&mut out);
                    small
                        .write_with_encoder(enc)
                        .map_err(|_| StatusCode::BAD_REQUEST)?;
                    Ok((out, "webp", "image/webp"))
                }
                ImageFormat::Bmp => {
                    let enc = image::codecs::bmp::BmpEncoder::new(&mut out);
                    small
                        .to_rgb8()
                        .write_with_encoder(enc)
                        .map_err(|_| StatusCode::BAD_REQUEST)?;
                    Ok((out, "bmp", "image/bmp"))
                }
                // TIFF/ICO/PNM/… → PNG (browsers can't show TIFF anyway).
                _ => {
                    let enc = image::codecs::png::PngEncoder::new(&mut out);
                    small
                        .to_rgba8()
                        .write_with_encoder(enc)
                        .map_err(|_| StatusCode::BAD_REQUEST)?;
                    Ok((out, "png", "image/png"))
                }
            }
        }
    }
}

/// Source image for one instance: stored path (None = no image), whether a
/// remote fetch is still pending (upstream 503 case), and the FK column to
/// stamp on the cached `Thumbnail` row.
struct Source {
    rel: Option<String>,
    pending: bool,
    fk: Fk,
}

#[derive(Clone, Copy)]
enum Fk {
    ProductMedia(i32),
    User(i32),
    Category(i32),
    Collection(i32),
    App(i32),
    AppInstallation(i32),
}

async fn load_source(
    db: &sea_orm::DatabaseConnection,
    kind: InstanceKind,
    pk: &str,
) -> Result<Source, StatusCode> {
    const NF: StatusCode = StatusCode::NOT_FOUND;
    match kind {
        InstanceKind::ProductMedia => {
            let id: i32 = pk.parse().map_err(|_| NF)?;
            use saleor_rustify_db::entities::product_productmedia::Entity as MEnt;
            let m = MEnt::find_by_id(id).one(db).await.map_err(|_| NF)?.ok_or(NF)?;
            // Case-insensitive: Django seeds `IMAGE`, older Rust rows may
            // carry lowercase `image`.
            let pending = m.image.as_ref().map(|s| s.trim().is_empty()).unwrap_or(true)
                && m.r#type.eq_ignore_ascii_case("image")
                && m.external_url.is_some();
            Ok(Source {
                rel: m.image.filter(|s| !s.trim().is_empty()),
                pending,
                fk: Fk::ProductMedia(id),
            })
        }
        InstanceKind::User => {
            let u: uuid::Uuid = pk.parse().map_err(|_| NF)?;
            use saleor_rustify_db::entities::account_user::{Column as UCol, Entity as UEnt};
            let row = UEnt::find()
                .filter(UCol::Uuid.eq(u))
                .one(db)
                .await
                .map_err(|_| NF)?
                .ok_or(NF)?;
            Ok(Source {
                rel: row.avatar.filter(|s| !s.trim().is_empty()),
                pending: false,
                fk: Fk::User(row.id),
            })
        }
        InstanceKind::Category => {
            let id: i32 = pk.parse().map_err(|_| NF)?;
            use saleor_rustify_db::entities::product_category::Entity as CEnt;
            let c = CEnt::find_by_id(id).one(db).await.map_err(|_| NF)?.ok_or(NF)?;
            Ok(Source {
                rel: c.background_image.filter(|s| !s.trim().is_empty()),
                pending: false,
                fk: Fk::Category(id),
            })
        }
        InstanceKind::Collection => {
            let id: i32 = pk.parse().map_err(|_| NF)?;
            use saleor_rustify_db::entities::product_collection::Entity as CEnt;
            let c = CEnt::find_by_id(id).one(db).await.map_err(|_| NF)?.ok_or(NF)?;
            Ok(Source {
                rel: c.background_image.filter(|s| !s.trim().is_empty()),
                pending: false,
                fk: Fk::Collection(id),
            })
        }
        InstanceKind::App => {
            let u: uuid::Uuid = pk.parse().map_err(|_| NF)?;
            use saleor_rustify_db::entities::app_app::{Column as ACol, Entity as AEnt};
            let row = AEnt::find()
                .filter(ACol::Uuid.eq(u))
                .one(db)
                .await
                .map_err(|_| NF)?
                .ok_or(NF)?;
            Ok(Source {
                rel: row.brand_logo_default.filter(|s| !s.trim().is_empty()),
                pending: false,
                fk: Fk::App(row.id),
            })
        }
        InstanceKind::AppInstallation => {
            let u: uuid::Uuid = pk.parse().map_err(|_| NF)?;
            use saleor_rustify_db::entities::app_appinstallation::{Column as ACol, Entity as AEnt};
            let row = AEnt::find()
                .filter(ACol::Uuid.eq(u))
                .one(db)
                .await
                .map_err(|_| NF)?
                .ok_or(NF)?;
            Ok(Source {
                rel: row.brand_logo_default.filter(|s| !s.trim().is_empty()),
                pending: false,
                fk: Fk::AppInstallation(row.id),
            })
        }
    }
}

async fn find_cached(
    db: &sea_orm::DatabaseConnection,
    fk: Fk,
    size: i32,
    format: Option<&str>,
) -> Option<String> {
    use saleor_rustify_db::entities::thumbnail_thumbnail::{Column as TCol, Entity as TEnt};
    let mut q = TEnt::find()
        .filter(TCol::Size.eq(size))
        .filter(
            // NULL format means "resized original" upstream — `=` never
            // matches NULL, so the filter must branch.
            match format {
                Some(f) => TCol::Format.eq(f.to_string()),
                None => TCol::Format.is_null(),
            },
        );
    q = match fk {
        Fk::ProductMedia(id) => q.filter(TCol::ProductMediaId.eq(id)),
        Fk::User(id) => q.filter(TCol::UserId.eq(id)),
        Fk::Category(id) => q.filter(TCol::CategoryId.eq(id)),
        Fk::Collection(id) => q.filter(TCol::CollectionId.eq(id)),
        Fk::App(id) => q.filter(TCol::AppId.eq(id)),
        Fk::AppInstallation(id) => q.filter(TCol::AppInstallationId.eq(id)),
    };
    q.select_only()
        .column(TCol::Image)
        .into_tuple::<String>()
        .one(db)
        .await
        .ok()
        .flatten()
}

async fn insert_row(
    db: &sea_orm::DatabaseConnection,
    fk: Fk,
    size: i32,
    format: Option<&str>,
    key: &str,
) {
    use saleor_rustify_db::entities::thumbnail_thumbnail::ActiveModel as TAM;
    let mut am = TAM {
        image: Set(key.to_string()),
        size: Set(size),
        format: Set(format.map(|s| s.to_string())),
        ..Default::default()
    };
    match fk {
        Fk::ProductMedia(id) => am.product_media_id = Set(Some(id)),
        Fk::User(id) => am.user_id = Set(Some(id)),
        Fk::Category(id) => am.category_id = Set(Some(id)),
        Fk::Collection(id) => am.collection_id = Set(Some(id)),
        Fk::App(id) => am.app_id = Set(Some(id)),
        Fk::AppInstallation(id) => am.app_installation_id = Set(Some(id)),
    }
    use sea_orm::ActiveModelTrait;
    // Best-effort: a concurrent request may win the race (no unique guard
    // upstream either) — the redirect below works regardless.
    if let Err(e) = am.insert(db).await {
        tracing::warn!(key = %key, error = %e, "thumbnail: cache row insert failed");
    }
}

fn redirect(url: String) -> (StatusCode, HeaderMap, String) {
    let mut h = HeaderMap::new();
    if let Ok(v) = url.parse::<header::HeaderValue>() {
        h.insert(header::LOCATION, v);
    }
    (StatusCode::FOUND, h, String::new())
}

fn plain(status: StatusCode, body: &str) -> (StatusCode, HeaderMap, String) {
    (status, HeaderMap::new(), body.to_string())
}

fn decode_kind(gid: &str) -> Result<(InstanceKind, String), StatusCode> {
    // Dashboard round-trips sometimes double-encode; unwrap one layer first.
    let (ty, pk) = crate::common::unsplit_double(gid)
        .or_else(|| crate::common::split_gid(gid))
        .ok_or(StatusCode::NOT_FOUND)?;
    let kind = InstanceKind::parse(&ty).ok_or(StatusCode::NOT_FOUND)?;
    if kind.is_uuid() {
        pk.parse::<uuid::Uuid>().map_err(|_| StatusCode::NOT_FOUND)?;
    } else {
        pk.parse::<i32>().map_err(|_| StatusCode::NOT_FOUND)?;
    }
    Ok((kind, pk))
}

/// Core of `handle_thumbnail`: 302 to cached-or-rendered rendition.
/// Errors map 1:1 to upstream statuses (404/503/400).
pub async fn serve_thumbnail(
    db: &sea_orm::DatabaseConnection,
    gid: &str,
    size_raw: &str,
    format_raw: Option<&str>,
) -> (StatusCode, HeaderMap, String) {
    let (kind, pk) = match decode_kind(gid) {
        Ok(v) => v,
        Err(s) => return plain(s, "Cannot found instance with the given id."),
    };
    let req_size: i32 = match size_raw.parse() {
        Ok(n) => n,
        Err(_) => return plain(StatusCode::NOT_FOUND, "Invalid size."),
    };
    let format = match normalize_format(kind, format_raw) {
        Ok(f) => f,
        Err(s) => return plain(s, "Unsupported image format."),
    };
    let size = closest_size(req_size);

    let src = match load_source(db, kind, &pk).await {
        Ok(s) => s,
        Err(s) => return plain(s, "Instance with the given id cannot be found."),
    };
    let Some(rel) = src.rel else {
        if src.pending {
            let mut h = HeaderMap::new();
            h.insert(header::RETRY_AFTER, PENDING_RETRY_AFTER.parse().unwrap());
            return (StatusCode::SERVICE_UNAVAILABLE, h, PENDING_BODY.to_string());
        }
        return plain(StatusCode::NOT_FOUND, "There is no image for provided instance.");
    };

    if let Some(key) = find_cached(db, src.fk, size, format.as_deref()).await {
        return redirect(crate::storage::public_base() + "/" + key.trim_start_matches('/'));
    }

    let bytes = match crate::storage::read_object(&rel).await {
        Ok(b) => b,
        Err(_) => return plain(StatusCode::NOT_FOUND, "Cannot found image file."),
    };
    let (rend_bytes, out_ext, mime) = match render(&bytes, size, format.as_deref()) {
            Ok(v) => v,
            Err(_) if format.as_deref() == Some("avif") => {
                return plain(
                    StatusCode::BAD_REQUEST,
                    "AVIF output is not supported by this build.",
                )
            }
            Err(_) => return plain(StatusCode::BAD_REQUEST, "Invalid image."),
        };
    let key = rendition_key(&rel, size, format.as_deref(), out_ext);
    if let Err(e) = crate::storage::store_key(&key, &rend_bytes, mime).await {
        tracing::warn!(key = %key, error = %e, "thumbnail: rendition store failed");
        return plain(StatusCode::BAD_REQUEST, "Invalid image.");
    }
    insert_row(db, src.fk, size, format.as_deref(), &key).await;
    // Upstream fires `thumbnail_created` here; our outbox covers domain
    // events and the subscription has no local consumer — documented.
    redirect(crate::storage::public_base() + "/" + key.trim_start_matches('/'))
}

/// Core of `handle_original_image`: 302 to the source image URL.
pub async fn serve_original(
    db: &sea_orm::DatabaseConnection,
    gid: &str,
) -> (StatusCode, HeaderMap, String) {
    let (kind, pk) = match decode_kind(gid) {
        Ok(v) => v,
        Err(s) => return plain(s, "Cannot find instance with the given id."),
    };
    let src = match load_source(db, kind, &pk).await {
        Ok(s) => s,
        Err(s) => return plain(s, "Instance with the given id cannot be found."),
    };
    let Some(rel) = src.rel else {
        if src.pending {
            let mut h = HeaderMap::new();
            h.insert(header::RETRY_AFTER, PENDING_RETRY_AFTER.parse().unwrap());
            return (StatusCode::SERVICE_UNAVAILABLE, h, PENDING_BODY.to_string());
        }
        return plain(StatusCode::NOT_FOUND, "There is no image for provided instance.");
    };
    redirect(crate::common::media_url(&rel))
}

/// `GET /thumbnail/:instance_id/:size` + `.../:size/:format`.
pub async fn thumbnail_route(
    Extension(db): Extension<sea_orm::DatabaseConnection>,
    Path((instance_id, size)): Path<(String, String)>,
) -> (StatusCode, HeaderMap, String) {
    serve_thumbnail(&db, &instance_id, &size, None).await
}

/// `GET /thumbnail/:instance_id/:size/:format`.
pub async fn thumbnail_format_route(
    Extension(db): Extension<sea_orm::DatabaseConnection>,
    Path((instance_id, size, format)): Path<(String, String, String)>,
) -> (StatusCode, HeaderMap, String) {
    serve_thumbnail(&db, &instance_id, &size, Some(&format)).await
}

/// `GET /image/:instance_id`.
pub async fn original_route(
    Extension(db): Extension<sea_orm::DatabaseConnection>,
    Path(instance_id): Path<String>,
) -> (StatusCode, HeaderMap, String) {
    serve_original(&db, &instance_id).await
}
