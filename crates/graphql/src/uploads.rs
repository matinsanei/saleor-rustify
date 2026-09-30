//! Multipart uploads: `fileUpload`, `userAvatarUpdate`, `userAvatarDelete`.
//!
//! Mirrors saleor-core (`graphql/core/mutations.py::FileUpload`,
//! `account/mutations/staff/user_avatar_{update,delete}.py`, `settings.py`
//! `ALLOWED_MIME_TYPES`):
//! - transport = graphql-multipart-request-spec (server POST handler accepts
//!   `multipart/form-data` via `async_graphql_axum::GraphQLRequest`)
//! - empty files rejected; mime sniffed from magic bytes (never the declared
//!   content-type — spoofable), extension must match the mime's allowlist
//! - `file_upload/<stem>_<8hex><ext>` naming (spaces → `_`); avatars under
//!   `user-avatars/`, product images under `products/` (Django `upload_to`)
//! Files land in `SALEOR_MEDIA_DIR` — the same dir `/media/` serves, so the
//! returned URLs resolve immediately in the dashboard.
//!
//! Honest bounds (documented, same as upstream gaps): no rendition pipeline
//! (VersatileImageField sizes served as originals), no virus scan.
//! Uploads are buffered in RAM capped at `RUSTIFY_MAX_UPLOAD_BYTES`
//! (default 20MiB) — the OOM guard; cleanup paths never escape the media
//! root (absolute/`..` DB paths refused).

use async_graphql::*;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QuerySelect, Set};

use crate::gen;

/// Django `ALLOWED_MIME_TYPES` verbatim (saleor-core/saleor/settings.py;
/// `UPLOAD_ADDITIONAL_ALLOWED_MIME_TYPES` env merge is server ops, not schema).
fn allowed_exts(mime: &str) -> &'static [&'static str] {
    match mime {
        "image/avif" => &[".avif"],
        "image/bmp" => &[".bmp"],
        "image/gif" => &[".gif"],
        "image/jpeg" => &[".jpg", ".jpeg", ".jpe", ".jfif"],
        "image/png" => &[".png"],
        "image/tiff" => &[".tiff", ".tif"],
        "image/webp" => &[".webp"],
        "application/msword" => &[".doc"],
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => &[".docx"],
        "application/vnd.ms-excel" => &[".xls"],
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => &[".xlsx"],
        "application/vnd.ms-powerpoint" => &[".ppt"],
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => &[".pptx"],
        "video/mp4" => &[".mp4"],
        "video/webm" => &[".webm"],
        "video/ogg" => &[".ogv", ".ogg"],
        "video/quicktime" => &[".mov"],
        "audio/mpeg" => &[".mp3", ".mpeg"],
        "audio/mp4" => &[".m4a"],
        "audio/webm" => &[".weba"],
        "audio/ogg" => &[".oga", ".ogg"],
        "audio/wav" => &[".wav"],
        "text/plain" => &[".txt"],
        "text/csv" => &[".csv"],
        _ => &[],
    }
}

/// Magic-bytes mime candidates (content sniffing, like python-magic upstream).
/// Container formats that need the extension to disambiguate (zip/ole/ebml)
/// return all candidates; the extension cross-check picks.
fn sniff_mime(b: &[u8]) -> Vec<&'static str> {
    if b.len() >= 8 && &b[0..8] == b"\x89PNG\r\n\x1a\n" {
        return vec!["image/png"];
    }
    if b.len() >= 3 && &b[0..3] == b"\xff\xd8\xff" {
        return vec!["image/jpeg"];
    }
    if b.len() >= 6 && (&b[0..6] == b"GIF87a" || &b[0..6] == b"GIF89a") {
        return vec!["image/gif"];
    }
    if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        return vec!["image/webp"];
    }
    if b.len() >= 2 && &b[0..2] == b"BM" {
        return vec!["image/bmp"];
    }
    if b.len() >= 12 && &b[4..8] == b"ftyp" {
        let brand = &b[8..12];
        if brand == b"avif" || brand == b"avis" {
            return vec!["image/avif"];
        }
        if brand == b"M4A " || brand == b"M4B " {
            return vec!["audio/mp4"];
        }
        if brand == b"qt  " {
            return vec!["video/quicktime"];
        }
        // isom/mp41/mp42… → video/mp4 (m4a/mov brands handled above).
        return vec!["video/mp4"];
    }
    if b.len() >= 4 && ((&b[0..4] == b"II*\x00") || (&b[0..4] == b"MM\x00*")) {
        return vec!["image/tiff"];
    }
    if b.len() >= 4 && &b[0..4] == b"\x1a\x45\xdf\xa3" {
        return vec!["video/webm", "audio/webm"];
    }
    if b.len() >= 4 && &b[0..4] == b"OggS" {
        return vec!["video/ogg", "audio/ogg"];
    }
    if b.len() >= 3
        && (&b[0..3] == b"ID3" || (b[0] == 0xFF && (b[1] & 0xE0) == 0xE0 && b.len() > 2))
    {
        return vec!["audio/mpeg"];
    }
    if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WAVE" {
        return vec!["audio/wav"];
    }
    if b.len() >= 8 && &b[0..8] == b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1" {
        return vec![
            "application/msword",
            "application/vnd.ms-excel",
            "application/vnd.ms-powerpoint",
        ];
    }
    if b.len() >= 4 && &b[0..4] == b"PK\x03\x04" {
        return vec![
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        ];
    }
    // Plain-text fallback (Django python-magic reports text/plain for UTF-8
    // prose; demand printable so binaries can't sneak in as .txt/.csv).
    if !b.is_empty()
        && std::str::from_utf8(b)
            .map(|s| {
                s.chars()
                    .all(|c| !c.is_control() || c == '\n' || c == '\r' || c == '\t')
            })
            .unwrap_or(false)
    {
        return vec!["text/plain", "text/csv"];
    }
    vec![]
}

pub struct StoredUpload {
    pub filename: String,
    pub bytes: Vec<u8>,
}

/// Read a multipart `Upload` argument into memory (blocking read parked on
/// the blocking pool; uploads are media-sized, never streamed to object
/// storage here — same localhost-filesystem posture as Django dev storage).
/// Because the whole file is buffered, bytes are capped DURING the read
/// (`take(limit+1)` — never more than ~limit RAM per upload); override with
/// `RUSTIFY_MAX_UPLOAD_BYTES`.
fn max_upload_bytes() -> u64 {
    std::env::var("RUSTIFY_MAX_UPLOAD_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20 * 1024 * 1024)
}

pub async fn read_upload(ctx: &Context<'_>, up: &Upload) -> Result<StoredUpload, String> {
    let v = up
        .value(ctx)
        .map_err(|e| format!("cannot read uploaded file: {e}"))?;
    let filename = if v.filename.trim().is_empty() {
        "upload".to_string()
    } else {
        v.filename.clone()
    };
    let limit = max_upload_bytes();
    let mut r = v.into_read();
    let bytes = tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let mut b = Vec::new();
        r.by_ref().take(limit + 1).read_to_end(&mut b).map(|_| b)
    })
    .await
    .map_err(|e| format!("upload read task failed: {e}"))?
    .map_err(|e| format!("cannot read uploaded file: {e}"))?;
    if bytes.len() as u64 > limit {
        return Err(format!("uploaded file exceeds the {limit}-byte limit"));
    }
    Ok(StoredUpload { filename, bytes })
}

/// Upstream `validate_upload_file` semantics: non-empty + sniffed mime in the
/// allowlist + extension matches that mime. Returns the detected mime.
pub fn check_file(filename: &str, bytes: &[u8], images_only: bool) -> Result<String, String> {
    if bytes.is_empty() {
        return Err("Received an empty file.".to_string());
    }
    let ext = std::path::Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e.to_lowercase()))
        .unwrap_or_default();
    let mut cands = sniff_mime(bytes);
    if images_only {
        cands.retain(|m| m.starts_with("image/"));
        if cands.is_empty() {
            return Err("Only image files are allowed for avatars.".to_string());
        }
    }
    if cands.is_empty() {
        return Err(
            "File type not permitted. Contact your administrator or support team.".to_string(),
        );
    }
    if !ext.is_empty() {
        if let Some(m) = cands
            .iter()
            .find(|m| allowed_exts(m).contains(&ext.as_str()))
        {
            return Ok(m.to_string());
        }
        return Err(format!(
            "File extension '{ext}' does not match its content type. Expected one of: {}.",
            cands
                .iter()
                .flat_map(|m| allowed_exts(m).iter())
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(cands[0].to_string())
}

/// Media root: same dir `/media/` serves (server `SALEOR_MEDIA_DIR` default).
pub fn media_dir() -> std::path::PathBuf {
    std::env::var("SALEOR_MEDIA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("../saleor/saleor-core/media"))
}

/// Persist bytes under `<media>/<subdir>/<stem>_<8hex><ext>` (Django
/// `file_upload/<name>_<hash><ext>` scheme; spaces → `_`), return the
/// storage-relative path (what Django columns store).
pub fn save_upload(subdir: &str, filename: &str, bytes: &[u8]) -> Result<String, String> {
    let base = std::path::Path::new(filename)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("upload");
    let (stem, ext) = match base.rfind('.') {
        Some(i) if i > 0 => (&base[..i], base[i..].to_lowercase()),
        _ => (base, String::new()),
    };
    let clean: String = stem
        .replace(' ', "_")
        .chars()
        .filter(|c| !c.is_control() && *c != '/' && *c != '\\')
        .collect();
    let stem = if clean.trim().is_empty() {
        "upload".to_string()
    } else {
        clean
    };
    let hash = format!("{:08x}", rand::random::<u32>());
    let rel = format!("{subdir}/{stem}_{hash}{ext}");
    let dir = media_dir().join(subdir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create media dir: {e}"))?;
    std::fs::write(media_dir().join(&rel), bytes)
        .map_err(|e| format!("cannot store uploaded file: {e}"))?;
    Ok(rel)
}

/// Best-effort unlink (Django storage.delete never fails the mutation either).
/// `rel` comes from the DB, so it is fenced into the media root: absolute
/// paths and `..` are refused (lexical check — the target may already be
/// gone, so no canonicalize).
fn unlink_rel(rel: &str) {
    let rel = rel.trim();
    if rel.is_empty() {
        return;
    }
    let p = std::path::Path::new(rel);
    if p.is_absolute()
        || p.components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        tracing::warn!(path = %rel, "upload cleanup: refusing path outside media dir");
        return;
    }
    let target = media_dir().join(p);
    if let Err(e) = std::fs::remove_file(&target) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(path = %rel, error = %e, "upload cleanup: cannot delete file");
        }
    }
}

/// Staff JWT or app token (Django `fileUpload` permissions:
/// AUTHENTICATED_APP / AUTHENTICATED_STAFF_USER).
async fn staff_or_app(ctx: &Context<'_>, db: &sea_orm::DatabaseConnection) -> Result<(), Error> {
    if crate::account::app_caller(ctx, db).await.is_ok() {
        return Ok(());
    }
    let (uid, _) = crate::account::requester(ctx, db).await?;
    use saleor_rustify_db::entities::account_user::{Column as UCol, Entity as UEnt};
    let staff: Option<bool> = UEnt::find_by_id(uid)
        .select_only()
        .column(UCol::IsStaff)
        .into_tuple()
        .one(db)
        .await
        .map_err(|e| Error::new(e.to_string()))?;
    if staff == Some(true) {
        Ok(())
    } else {
        Err(Error::new("staff or app authentication required"))
    }
}

/// Staff user behind the JWT (Django avatar mutations act on context user and
/// require AUTHENTICATED_STAFF_USER).
async fn staff_user(ctx: &Context<'_>, db: &sea_orm::DatabaseConnection) -> Result<i32, Error> {
    let (uid, _) = crate::account::requester(ctx, db).await?;
    use saleor_rustify_db::entities::account_user::{Column as UCol, Entity as UEnt};
    let staff: Option<bool> = UEnt::find_by_id(uid)
        .select_only()
        .column(UCol::IsStaff)
        .into_tuple()
        .one(db)
        .await
        .map_err(|e| Error::new(e.to_string()))?;
    if staff == Some(true) {
        Ok(uid)
    } else {
        Err(Error::new("staff only"))
    }
}

fn aerr(message: String) -> Vec<crate::account::GqlAccountError> {
    vec![crate::account::GqlAccountError {
        field: None,
        message,
        code: "INVALID".to_string(),
        address_type: None,
        attributes: None,
    }]
}

fn uerr(message: String) -> Vec<gen::UploadError> {
    vec![gen::UploadError {
        field: None,
        message: Some(message),
        code: None,
    }]
}

/// Delete avatar file + thumbnail rows (Django also deletes `Thumbnail`
/// objects for the user; rendition files are best-effort unlinked).
async fn clear_avatar(
    db: &sea_orm::DatabaseConnection,
    uid: i32,
    current: Option<String>,
) -> Result<(), Error> {
    use saleor_rustify_db::entities::thumbnail_thumbnail::{Column as TCol, Entity as TEnt};
    if let Some(rel) = current.filter(|s| !s.trim().is_empty()) {
        unlink_rel(&rel);
    }
    let thumbs: Vec<String> = TEnt::find()
        .select_only()
        .column(TCol::Image)
        .filter(TCol::UserId.eq(uid))
        .into_tuple()
        .all(db)
        .await
        .map_err(|e| Error::new(e.to_string()))?;
    TEnt::delete_many()
        .filter(TCol::UserId.eq(uid))
        .exec(db)
        .await
        .map_err(|e| Error::new(e.to_string()))?;
    for t in thumbs {
        unlink_rel(&t);
    }
    Ok(())
}

/// `User.avatar` field: stored path → `/media/` URL (REAL_METHODS backer).
pub async fn user_avatar(
    db: &sea_orm::DatabaseConnection,
    user_gid: &str,
) -> Option<crate::account::GqlImage> {
    let uid = saleor_rustify_db::catalog::parse_gid(user_gid.trim())?;
    use saleor_rustify_db::entities::account_user::{Column as UCol, Entity as UEnt};
    let rel: Option<String> = UEnt::find_by_id(uid)
        .select_only()
        .column(UCol::Avatar)
        .into_tuple()
        .one(db)
        .await
        .ok()??;
    rel.filter(|s| !s.trim().is_empty())
        .map(|p| crate::account::GqlImage {
            url: crate::common::media_url(&p),
            alt: None,
        })
}

#[derive(Default)]
pub struct UploadsMutation;

#[Object]
impl UploadsMutation {
    /// Upload a file (Django `fileUpload`): validate + store under
    /// `file_upload/`, return URL + content type.
    async fn file_upload(&self, ctx: &Context<'_>, file: Upload) -> Result<gen::FileUpload> {
        let g = ctx.data::<crate::context::GqlContext>()?;
        let db = g.db()?;
        if let Err(e) = staff_or_app(ctx, db).await {
            return Ok(gen::FileUpload {
                uploaded_file: None,
                errors: uerr(e.message.to_string()),
            });
        }
        let up = read_upload(ctx, &file).await.map_err(Error::new)?;
        let mime = match check_file(&up.filename, &up.bytes, false) {
            Ok(m) => m,
            Err(e) => {
                return Ok(gen::FileUpload {
                    uploaded_file: None,
                    errors: uerr(e),
                })
            }
        };
        let rel = save_upload("file_upload", &up.filename, &up.bytes).map_err(Error::new)?;
        Ok(gen::FileUpload {
            uploaded_file: Some(gen::File {
                url: Some(crate::common::media_url(&rel)),
                content_type: Some(mime),
            }),
            errors: vec![],
        })
    }

    /// Set the requesting staff member's avatar (Django `userAvatarUpdate`).
    async fn user_avatar_update(
        &self,
        ctx: &Context<'_>,
        image: Upload,
    ) -> Result<gen::UserAvatarUpdate> {
        let g = ctx.data::<crate::context::GqlContext>()?;
        let db = g.db()?;
        let uid = staff_user(ctx, db).await?;
        let up = read_upload(ctx, &image).await.map_err(Error::new)?;
        if let Err(e) = check_file(&up.filename, &up.bytes, true) {
            return Ok(gen::UserAvatarUpdate {
                user: None,
                errors: aerr(e),
            });
        }
        use saleor_rustify_db::entities::account_user::{Column as UCol, Entity as UEnt};
        let current: Option<String> = UEnt::find_by_id(uid)
            .select_only()
            .column(UCol::Avatar)
            .into_tuple()
            .one(db)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .flatten();
        clear_avatar(db, uid, current).await?;
        let rel = save_upload("user-avatars", &up.filename, &up.bytes).map_err(Error::new)?;
        let mut am: saleor_rustify_db::entities::account_user::ActiveModel = UEnt::find_by_id(uid)
            .one(db)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .ok_or_else(|| Error::new("user not found"))?
            .into();
        am.avatar = Set(Some(rel));
        am.update(db).await.map_err(|e| Error::new(e.to_string()))?;
        let user = crate::account::assemble_user(db, uid, &uid.to_string())
            .await
            .map_err(Error::new)?;
        Ok(gen::UserAvatarUpdate {
            user: Some(user),
            errors: vec![],
        })
    }

    /// Delete the requesting staff member avatar (Django `userAvatarDelete`).
    async fn user_avatar_delete(&self, ctx: &Context<'_>) -> Result<gen::UserAvatarDelete> {
        let g = ctx.data::<crate::context::GqlContext>()?;
        let db = g.db()?;
        let uid = staff_user(ctx, db).await?;
        use saleor_rustify_db::entities::account_user::Entity as UEnt;
        let row = UEnt::find_by_id(uid)
            .one(db)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .ok_or_else(|| Error::new("user not found"))?;
        clear_avatar(db, uid, row.avatar.clone()).await?;
        let mut am: saleor_rustify_db::entities::account_user::ActiveModel = row.into();
        am.avatar = Set(None);
        am.update(db).await.map_err(|e| Error::new(e.to_string()))?;
        let user = crate::account::assemble_user(db, uid, &uid.to_string())
            .await
            .map_err(Error::new)?;
        Ok(gen::UserAvatarDelete {
            user: Some(user),
            errors: vec![],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlink_refuses_escape_but_deletes_inside() {
        let dir = std::env::temp_dir().join("rustify-unlink-unit");
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("SALEOR_MEDIA_DIR", &dir);
        // Plain relative path under the root: deleted.
        std::fs::write(dir.join("gone.txt"), b"x").unwrap();
        unlink_rel("gone.txt");
        assert!(!dir.join("gone.txt").exists());
        // Absolute path outside the root: refused.
        let outside = std::env::temp_dir().join("rustify-unlink-unit-outside.txt");
        std::fs::write(&outside, b"x").unwrap();
        unlink_rel(outside.to_str().unwrap());
        assert!(outside.exists());
        // `..` escape: refused (no file is even resolved).
        unlink_rel("../rustify-unlink-unit-outside.txt");
        assert!(outside.exists());
        let _ = std::fs::remove_file(&outside);
        std::env::remove_var("SALEOR_MEDIA_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upload_cap_is_positive() {
        assert!(max_upload_bytes() > 0);
    }
}
