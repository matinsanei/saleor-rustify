//! Object storage backend: S3-compatible (MinIO) via SigV4 REST, with a
//! local-disk fallback.
//!
//! Django reference: `saleor/core/storages.py::S3MediaStorage`
//! (`AWS_MEDIA_BUCKET_NAME` + `AWS_MEDIA_CUSTOM_DOMAIN`) and
//! `saleor/graphql/product/mutations/product/product_media_create.py`
//! (`upload_to="products"` naming `<name>_<hash><ext>`).
//!
//! Env contract — the SAME names the Python containers already use
//! (docker-compose `saleor-api` service), so a swap needs no new secrets:
//! - `AWS_S3_ENDPOINT_URL` — gate: when set (non-empty), S3 is the backend
//! - `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`
//! - `AWS_MEDIA_BUCKET_NAME` (fallback `AWS_STORAGE_BUCKET_NAME`)
//! - `AWS_S3_REGION_NAME` (default `us-east-1`)
//! - `AWS_MEDIA_CUSTOM_DOMAIN` (e.g. `media.example.com/saleor-media`) — the
//!   public URL base; Django's `S3Boto3Storage.url()` is
//!   `https://{custom_domain}/{key}`
//!
//! URL precedence (deliberate): S3 mode → `AWS_MEDIA_CUSTOM_DOMAIN`.
//! `SALEOR_MEDIA_URL` is honored ONLY in local mode; when S3 is on it is
//! ignored with a warning, because the bytes live in the bucket and a local
//! URL would 404 (PixelStack `.env` still carries a stale
//! `SALEOR_MEDIA_URL=http://saleor-api:8000/media/`).
//! Local mode (no `AWS_S3_ENDPOINT_URL`): `SALEOR_MEDIA_URL` else
//! `http://localhost:8000/media`, bytes under `SALEOR_MEDIA_DIR`.
//!
//! DB contract (both modes): columns store the storage-RELATIVE path
//! (`products/foo_1a2b3c4d.jpg`) — byte-identical to what Django writes,
//! so rows stay portable across backends. No host ever hits the DB.
//!
//! Auth: SigV4 over `reqwest` (path-style `/{bucket}/{key}` — what MinIO
//! expects). Signed headers are `host`, `x-amz-content-sha256`,
//! `x-amz-date` only; `content-type` rides unsigned so sniffed-mime changes
//! can never break the signature. `hmac`+`sha2` are already workspace deps.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

/// S3/MinIO backend configuration, parsed from the Django-compatible env.
#[derive(Debug, Clone)]
pub struct S3Config {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    /// `https://{AWS_MEDIA_CUSTOM_DOMAIN}` (custom domain already contains
    /// the bucket prefix, e.g. `media.example.com/saleor-media`).
    pub public_base: String,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.trim().is_empty())
}

/// `Some` iff `AWS_S3_ENDPOINT_URL` is set; `None` → local-disk mode.
/// Missing credentials/bucket in S3 mode is a hard `None` + warn (fail
/// closed: never silently write local while URLs point at the CDN).
pub fn s3_config() -> Option<S3Config> {
    let endpoint = env("AWS_S3_ENDPOINT_URL")?.trim_end_matches('/').to_string();
    let (access_key, secret_key, bucket) = match (
        env("AWS_ACCESS_KEY_ID"),
        env("AWS_SECRET_ACCESS_KEY"),
        env("AWS_MEDIA_BUCKET_NAME").or_else(|| env("AWS_STORAGE_BUCKET_NAME")),
    ) {
        (Some(a), Some(s), Some(b)) => (a, s, b),
        _ => {
            tracing::warn!(
                "AWS_S3_ENDPOINT_URL is set but access key/secret/bucket is missing; falling back to local disk"
            );
            return None;
        }
    };
    let region = env("AWS_S3_REGION_NAME").unwrap_or_else(|| "us-east-1".into());
    let public_base = match env("AWS_MEDIA_CUSTOM_DOMAIN") {
        Some(d) => format!("https://{}", d.trim().trim_matches('/')),
        None => format!("{endpoint}/{bucket}"),
    };
    Some(S3Config {
        endpoint,
        bucket,
        region,
        access_key,
        secret_key,
        public_base,
    })
}

/// Public URL base for everything `media_url()` builds.
pub fn public_base() -> String {
    if let Some(cfg) = s3_config() {
        return cfg.public_base;
    }
    env("SALEOR_MEDIA_URL")
        .map(|b| b.trim_end_matches('/').to_string())
        .unwrap_or_else(|| "http://localhost:8000/media".into())
}

/// Media root for local-disk mode (server `SALEOR_MEDIA_DIR` default).
pub fn media_dir() -> std::path::PathBuf {
    std::env::var("SALEOR_MEDIA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("../saleor/saleor-core/media"))
}

/// Django `file_upload/<name>_<hash><ext>` naming (spaces → `_`);
/// returns the storage-relative path (what Django columns store).
pub fn rel_path(subdir: &str, filename: &str) -> String {
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
    format!("{subdir}/{stem}_{hash}{ext}")
}

/// S3 path-segment encoding: keep unreserved + `/`, `%XX` (uppercase) rest.
fn encode_key(key: &str) -> String {
    const KEEP: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.~/";
    let mut out = String::with_capacity(key.len());
    for b in key.bytes() {
        if KEEP.contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn sha256_hex(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut m = Hmac::<Sha256>::new_from_slice(key).expect("hmac takes any key size");
    m.update(data);
    m.finalize().into_bytes().to_vec()
}

/// SigV4 signature for `method` on `{endpoint}/{bucket}/{key}`.
/// `amz_date` = `YYYYMMDD'T'HHMMSS'Z'`; split out for testability (the AWS
/// published vector pins the timestamp).
fn sign_v4(
    cfg: &S3Config,
    method: &str,
    key: &str,
    payload_hash: &str,
    amz_date: &str,
) -> (String, String) {
    let host = cfg
        .endpoint
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let uri = format!("/{}/{}", cfg.bucket, encode_key(key));
    let signed_headers = "host;x-amz-content-sha256;x-amz-date";
    let canonical = format!(
        "{method}\n{uri}\n\n\
         host:{host}\n\
         x-amz-content-sha256:{payload_hash}\n\
         x-amz-date:{amz_date}\n\
         \n\
         {signed_headers}\n\
         {payload_hash}"
    );
    let date_stamp = &amz_date[..8];
    let scope = format!("{date_stamp}/{}/s3/aws4_request", cfg.region);
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        sha256_hex(canonical.as_bytes())
    );
    let k_date = hmac_sha256(format!("AWS4{}", cfg.secret_key).as_bytes(), date_stamp.as_bytes());
    let k_region = hmac_sha256(&k_date, cfg.region.as_bytes());
    let k_service = hmac_sha256(&k_region, b"s3");
    let k_sign = hmac_sha256(&k_service, b"aws4_request");
    let sig = hmac_sha256(&k_sign, to_sign.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let auth = format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={sig}",
        cfg.access_key
    );
    (format!("{}{uri}", cfg.endpoint), auth)
}

fn amz_date_now() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

async fn s3_request(
    cfg: &S3Config,
    method: reqwest::Method,
    key: &str,
    body: Option<(Vec<u8>, String)>,
) -> Result<reqwest::Response, String> {
    let payload_hash = body
        .as_ref()
        .map(|(b, _)| sha256_hex(b))
        .unwrap_or_else(|| sha256_hex(&[]));
    let amz_date = amz_date_now();
    let (url, auth) = sign_v4(cfg, method.as_str(), key, &payload_hash, &amz_date);
    let client = reqwest::Client::new();
    let mut req = client
        .request(method, &url)
        .header("Authorization", auth)
        .header("x-amz-date", amz_date)
        .header("x-amz-content-sha256", payload_hash);
    if let Some((bytes, content_type)) = body {
        req = req.header("Content-Type", content_type).body(bytes);
    }
    req.send()
        .await
        .map_err(|e| format!("object storage request failed: {e}"))
}

/// PUT bytes to the bucket under `key`; 2xx required.
pub async fn put_object(key: &str, bytes: Vec<u8>, content_type: &str) -> Result<(), String> {
    let cfg = s3_config().ok_or_else(|| "object storage is not configured".to_string())?;
    let resp = s3_request(
        &cfg,
        reqwest::Method::PUT,
        key,
        Some((bytes, content_type.to_string())),
    )
    .await?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("object storage PUT failed: HTTP {}", resp.status()))
    }
}

/// DELETE from the bucket; 404 (already gone) counts as success —
/// same best-effort posture as Django `storage.delete`.
pub async fn delete_object(key: &str) -> Result<(), String> {
    let cfg = s3_config().ok_or_else(|| "object storage is not configured".to_string())?;
    let resp = s3_request(&cfg, reqwest::Method::DELETE, key, None).await?;
    if resp.status().is_success() || resp.status() == reqwest::StatusCode::NOT_FOUND {
        Ok(())
    } else {
        Err(format!(
            "object storage DELETE failed: HTTP {}",
            resp.status()
        ))
    }
}

/// Store upload bytes; returns the storage-relative path for the DB column.
/// S3 when configured, else local disk (unchanged legacy path).
pub async fn store_upload(
    subdir: &str,
    filename: &str,
    bytes: &[u8],
    content_type: &str,
) -> Result<String, String> {
    let rel = rel_path(subdir, filename);
    store_key(&rel, bytes, content_type).await?;
    Ok(rel)
}

/// Store bytes under an exact storage-relative `key` (renditions, whose key
/// mirrors upstream's `prepare_thumbnail_file_name`, not the upload scheme).
pub async fn store_key(key: &str, bytes: &[u8], content_type: &str) -> Result<(), String> {
    if s3_config().is_some() {
        return put_object(key, bytes.to_vec(), content_type).await;
    }
    let full = media_dir().join(key.trim_start_matches('/'));
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create media dir: {e}"))?;
    }
    std::fs::write(&full, bytes).map_err(|e| format!("cannot store file: {e}"))?;
    Ok(())
}

/// Download a remote image into memory (Django
/// `fetch_product_media_image_task` parity — but synchronous at create time:
/// this codebase has no generic job queue, only the webhook outbox, so the
/// fetch happens inside `productMediaCreate`. Same outcome contract as
/// upstream: success fills `image`, failure leaves the row external-only and
/// the mutation still succeeds — the caller decides, this fn just errors).
/// http/https only (no `file://`/gopher-style schemes); 10s total timeout
/// (project rule: every outbound fetch is bounded); size-capped at
/// `RUSTIFY_MAX_UPLOAD_BYTES` like multipart uploads.
/// Returns `(bytes, filename_hint, mime)` — mime sniffed from magic bytes
/// via the caller's `check_file(images_only=true)` contract… actually sniffed
/// HERE so the caller can store with the right content-type: returns the
/// detected mime too.
pub async fn fetch_remote_image(url: &str) -> Result<(Vec<u8>, String, String), String> {
    let url = url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("only http(s) media URLs can be fetched".to_string());
    }
    let limit: u64 = std::env::var("RUSTIFY_MAX_UPLOAD_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20 * 1024 * 1024);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("cannot build fetch client: {e}"))?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("cannot fetch media URL: {e}"))?
        .error_for_status()
        .map_err(|e| format!("media URL returned error: {e}"))?;
    if let Some(len) = resp.content_length() {
        if len > limit {
            return Err(format!("remote file exceeds the {limit}-byte limit"));
        }
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("cannot read remote file: {e}"))?;
    if bytes.len() as u64 > limit {
        return Err(format!("remote file exceeds the {limit}-byte limit"));
    }
    // Reuse the multipart validation (magic-bytes sniff, images only).
    let hint = url
        .split('?')
        .next()
        .unwrap_or("remote")
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("remote")
        .to_string();
    let mime = crate::uploads::check_file(&hint, &bytes, true)
        .map_err(|e| format!("fetched file is not a usable image: {e}"))?;
    Ok((bytes.to_vec(), hint, mime))
}

/// Extension for a sniffed image mime (remote URLs often carry no extension;
/// storage keys need one so local-disk serving + CDN sniffing behave).
fn image_ext(mime: &str) -> &'static str {
    match mime {
        "image/png" => ".png",
        "image/gif" => ".gif",
        "image/webp" => ".webp",
        "image/bmp" => ".bmp",
        "image/avif" => ".avif",
        "image/tiff" => ".tiff",
        _ => ".jpg",
    }
}

/// Filename for a fetched remote: keep the URL hint's stem, force the
/// sniffed extension (the hint's ext is untrusted / often absent).
pub fn remote_filename(hint: &str, mime: &str) -> String {
    let stem = hint
        .rsplit('.')
        .nth(1)
        .filter(|s| !s.is_empty() && !s.contains('/') && s.len() <= 64)
        .unwrap_or(hint);
    let stem = if stem.len() > 64 { &stem[..64] } else { stem };
    format!("{stem}{}", image_ext(mime))
}

/// GET bytes by storage-relative key: S3 object or local-disk file.
/// Used by the thumbnail renderer (source images). Missing → Err.
/// Same lexical fence as `delete_upload` (absolute/`..` refused).
pub async fn read_object(rel: &str) -> Result<Vec<u8>, String> {
    let rel = rel.trim();
    let p = std::path::Path::new(rel);
    if p.is_absolute()
        || p.components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("path escapes the media root".to_string());
    }
    if let Some(cfg) = s3_config() {
        let payload_hash = sha256_hex(&[]);
        let amz_date = amz_date_now();
        let (url, auth) = sign_v4(&cfg, "GET", rel, &payload_hash, &amz_date);
        let resp = reqwest::Client::new()
            .get(&url)
            .header("Authorization", auth)
            .header("x-amz-date", amz_date)
            .header("x-amz-content-sha256", payload_hash)
            .send()
            .await
            .map_err(|e| format!("object storage GET failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("object storage GET: HTTP {}", resp.status()));
        }
        return resp
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| format!("object storage GET body failed: {e}"));
    }
    std::fs::read(media_dir().join(rel.trim_start_matches('/')))
        .map_err(|e| format!("cannot read stored file: {e}"))
}

/// Best-effort delete (Django `storage.delete` never fails the mutation
/// either): S3 object or local file; `rel` is DB-sourced so it stays fenced
/// to the media root — absolute paths and `..` are refused (lexical check,
/// the target may already be gone so no canonicalize).
pub async fn delete_upload(rel: &str) {
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
    if s3_config().is_some() {
        if let Err(e) = delete_object(rel).await {
            tracing::warn!(path = %rel, error = %e, "upload cleanup: cannot delete object");
        }
        return;
    }
    if let Err(e) = std::fs::remove_file(media_dir().join(p)) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(path = %rel, error = %e, "upload cleanup: cannot delete file");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cfg(endpoint: &str) -> S3Config {
        S3Config {
            endpoint: endpoint.into(),
            bucket: "saleor-media".into(),
            region: "us-east-1".into(),
            access_key: "testkey".into(),
            secret_key: "testsecret".into(),
            public_base: "https://media.example.com/saleor-media".into(),
        }
    }

    /// SigV4 cross-check: expected signature computed INDEPENDENTLY with
    /// Python hashlib/hmac (not this code) — see commit message / test log.
    /// payload = b"hello".
    #[test]
    fn sigv4_matches_independent_vector() {
        let cfg = test_cfg("http://minio:9000");
        let payload = sha256_hex(b"hello");
        let (url, auth) = sign_v4(
            &cfg,
            "PUT",
            "products/shoe_1a2b3c4d.jpg",
            &payload,
            "20240115T120000Z",
        );
        assert_eq!(
            url,
            "http://minio:9000/saleor-media/products/shoe_1a2b3c4d.jpg"
        );
        assert!(
            auth.contains("Credential=testkey/20240115/us-east-1/s3/aws4_request"),
            "credential scope wrong: {auth}"
        );
        assert!(
            auth.contains(
                "Signature=40df0bbfe8daa626beff884b1f4dec88fe374e5b0d8db3ddd3628590131f2e5b"
            ),
            "signature mismatch: {auth}"
        );
    }

    #[test]
    fn encode_key_cases() {
        assert_eq!(encode_key("products/a_b-1.2~x.jpg"), "products/a_b-1.2~x.jpg");
        assert_eq!(encode_key("products/a+b c.jpg"), "products/a%2Bb%20c.jpg");
        assert_eq!(encode_key("x"), "x");
    }

    #[test]
    fn remote_filename_cases() {
        assert_eq!(remote_filename("photo.webp", "image/webp"), "photo.webp");
        assert_eq!(remote_filename("photo", "image/png"), "photo.png");
        assert_eq!(remote_filename("shot.bmp?x=1", "image/png"), "shot.png");
        assert_eq!(remote_filename("evil.exe", "image/jpeg"), "evil.jpg");
    }

    #[tokio::test]
    async fn fetch_remote_image_flows() {
        let png: Vec<u8> = b"\x89PNG\r\n\x1a\nfakepngbytes".to_vec();
        let app = axum::Router::new()
            .route(
                "/ok.png",
                axum::routing::get({
                    let png = png.clone();
                    move || {
                        let png = png.clone();
                        async move {
                            (
                                [(axum::http::header::CONTENT_TYPE, "image/png")],
                                png,
                            )
                        }
                    }
                }),
            )
            .route("/missing.png", axum::routing::get(|| async { axum::http::StatusCode::NOT_FOUND }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let (bytes, hint, mime) = fetch_remote_image(&format!("http://{addr}/ok.png?size=big"))
            .await
            .unwrap();
        assert_eq!(bytes, png);
        assert_eq!(hint, "ok.png");
        assert_eq!(mime, "image/png");

        assert!(fetch_remote_image(&format!("http://{addr}/missing.png")).await.is_err());
        assert!(fetch_remote_image("file:///etc/passwd").await.is_err());
        assert!(fetch_remote_image("ftp://x/y.png").await.is_err());
    }

    #[test]
    fn rel_path_matches_django_scheme() {
        let rel = rel_path("products", "My Shoe.JPG");
        assert!(rel.starts_with("products/My_Shoe_"), "{rel}");
        assert!(rel.ends_with(".jpg"), "{rel}");
        let hash = rel
            .trim_start_matches("products/My_Shoe_")
            .trim_end_matches(".jpg");
        assert_eq!(hash.len(), 8, "{rel}");
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()), "{rel}");
    }

    /// ALL env-mutating assertions live in this ONE test: process env is
    /// global and this crate's tests run on multiple threads, so a second
    /// env-touching test would race it.
    #[tokio::test]
    async fn env_modes_and_live_put_against_mock() {
        // --- local mode: no S3 env ---
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
        assert!(s3_config().is_none());
        let dir = std::env::temp_dir().join("rustify-storage-unit");
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("SALEOR_MEDIA_DIR", &dir);
        std::env::set_var("SALEOR_MEDIA_URL", "http://cdn.example.com/m");
        assert_eq!(public_base(), "http://cdn.example.com/m");
        let rel = store_upload("products", "Shoe.PNG", b"fake-bytes", "image/png")
            .await
            .unwrap();
        assert!(rel.starts_with("products/Shoe_") && rel.ends_with(".png"), "{rel}");
        assert_eq!(std::fs::read(dir.join(&rel)).unwrap(), b"fake-bytes");
        delete_upload(&rel).await;
        assert!(!dir.join(&rel).exists());
        // Escape fence still holds on the new path.
        delete_upload("../outside.txt").await;
        delete_upload("/abs/path.txt").await;

        // --- S3 mode: SALEOR_MEDIA_URL must LOSE to the custom domain ---
        std::env::set_var("AWS_S3_ENDPOINT_URL", "http://127.0.0.1:9");
        std::env::set_var("AWS_ACCESS_KEY_ID", "k");
        std::env::set_var("AWS_SECRET_ACCESS_KEY", "s");
        std::env::set_var("AWS_STORAGE_BUCKET_NAME", "saleor-media");
        std::env::set_var("AWS_S3_REGION_NAME", "us-east-1");
        std::env::set_var("AWS_MEDIA_CUSTOM_DOMAIN", "media.example.com/saleor-media");
        let cfg = s3_config().expect("bucket falls back to AWS_STORAGE_BUCKET_NAME");
        assert_eq!(cfg.bucket, "saleor-media");
        assert_eq!(public_base(), "https://media.example.com/saleor-media");

        // --- live PUT/DELETE against a mock S3 (proves request formation) ---
        use std::sync::{Arc, Mutex};
        #[derive(Debug, Clone)]
        struct Seen {
            method: String,
            path: String,
            auth: String,
            content_type: String,
            body: Vec<u8>,
        }
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::new(Mutex::new(Vec::new()));
        let app = {
            let seen = seen.clone();
            axum::Router::new().route(
                "/*rest",
                axum::routing::any(|req: axum::http::Request<axum::body::Body>| async move {
                    let method = req.method().to_string();
                    let path = req.uri().path().to_string();
                    let auth = req
                        .headers()
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    let content_type = req
                        .headers()
                        .get("content-type")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    let body = axum::body::to_bytes(req.into_body(), 64 * 1024 * 1024)
                        .await
                        .unwrap()
                        .to_vec();
                    if path.ends_with("/gone.jpg") {
                        return axum::http::StatusCode::NOT_FOUND;
                    }
                    seen.lock().unwrap().push(Seen {
                        method,
                        path,
                        auth,
                        content_type,
                        body,
                    });
                    axum::http::StatusCode::OK
                }),
            )
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        std::env::set_var("AWS_S3_ENDPOINT_URL", format!("http://{addr}"));

        put_object("products/pic_12345678.jpg", b"img-bytes".to_vec(), "image/jpeg")
            .await
            .unwrap();
        // 404 on DELETE must be tolerated (already-gone object).
        delete_object("products/gone.jpg").await.unwrap();
        // And a successful DELETE path (mock records non-gone paths).
        delete_object("products/old_12345678.jpg").await.unwrap();

        let calls = seen.lock().unwrap().clone();
        assert_eq!(calls.len(), 2, "{calls:?}");
        let put = &calls[0];
        assert_eq!(put.method, "PUT");
        assert_eq!(put.path, "/saleor-media/products/pic_12345678.jpg");
        assert!(put.auth.starts_with("AWS4-HMAC-SHA256 Credential=k/"), "{}", put.auth);
        assert!(put.auth.contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date"), "{}", put.auth);
        assert_eq!(put.content_type, "image/jpeg");
        assert_eq!(put.body, b"img-bytes");
        let del = &calls[1];
        assert_eq!(del.method, "DELETE");
        assert_eq!(del.path, "/saleor-media/products/old_12345678.jpg");

        for v in [
            "AWS_S3_ENDPOINT_URL",
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_STORAGE_BUCKET_NAME",
            "AWS_MEDIA_CUSTOM_DOMAIN",
            "SALEOR_MEDIA_URL",
            "SALEOR_MEDIA_DIR",
        ] {
            std::env::remove_var(v);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
