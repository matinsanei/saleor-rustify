# Fleet Rollout Checklist — 1 custom-api + N Rust engines

> Goal: 5 independent shops, each with its own data + engine + DB,
> orchestrated by ONE custom-api. No tenant layer, no shared catalog.
> Owner decision 2026-10-01: fleet model (not multi-tenant single stack).
>
> ## Readiness rollup (built 2026-10-01 — all streams DONE, verified)
>
> | Stream | Result |
> |---|---|
> | GraphQL caller parity | `productBulkPublish` implemented (was on NEITHER backend); all ~70 caller ops resolve; harness 458/458 (5th run) |
> | Beat/worker/domain | All Deferred entries now Real: promo recalc, release-funds scan, expire/delete orders, toggle, giftcard, preorder, stock-alloc, hygiene (payloads/apps/allocations); order emails wired (7 triggers) |
> | Custom-api multi-engine | Registry (`SHOPS_JSON`), AsyncLocalStorage shopId, shop-aware clients, per-engine breakers, pool map, multi-secret webhooks, views loop, shop_id migration 0021 (drizzle-kit snapshot), API `tsc` clean, vitest 118 pass / 4 pre-existing fails (baseline-identical) |
> | Deploy + cutover | compose `saleor-rust{,-worker,-beat,-init}` + Traefik + MinIO bucket + `.env.example`; `docker compose config` valid; migrate story = Python-image one-shot (schema stays Django-owned); `/health` live |
> | Gates | `cargo check --workspace` 0 errors, all Rust suites green, harness 458/458 |

## Conventions

- `[ ]` todo · `[x]` done · `[~]` partially / documented gap
- Every item cites the file that owns it. No item closes without either
  a green test/harness line or a live E2E proof noted in-line.
- Golden gates (must stay green after every change):
  `cargo check` 0 errors · dashboard harness 458/458 ·
  `cargo test -p saleor-rustify-db` all green.

---

## Phase 0 — Swap readiness (custom-API compat) [x] DONE

Verified live against the exact caller query texts + harness 458/458 (x3).
Details in CHANGELOG.md Unreleased.

- [x] Full `checkout` object (token/voucher/discount/addresses/methods/priced lines)
- [x] `checkoutComplete {order errors}` + gates (payment/address/method) + replay
- [x] Coded write-path validation via shared `db::validate_lines` (GraphQL+gRPC)
- [x] Shipping leg in totals + shipping vouchers (no double count)
- [x] `transactionCreate` currency/psp-less/clamps; init empty-key reject
- [x] `orders` channel arg + 1000 window; catalog variants/pricing/externalRef/updatedAt
- [x] `customerCreate` UNIQUE; `confirmAccount` email binding
- [x] Zero hardcoded channels (central `fallback_channel_slug`, header +
      variables inference, unambiguous-or-none nested pricing)
- [x] Channel-scoped stock validation in `validate_lines`
- [x] Variant `updatedAt` real (was breaking inventory staleness flags)
- [x] PixelStack one-liner: `updatePrivateMetadata` in
      `reconciliation.service.ts:407` (was silently dropped on BOTH backends)

## Phase 1 — Media pipeline (PRODUCTION BLOCKER per shop) [x] DONE 2026-10-01

Django reference: `saleor-3.23.33/saleor/graphql/product/mutations/product/product_media_create.py:72-150`,
`saleor/thumbnail/views.py:59` (`handle_thumbnail`), `saleor/core/storages.py:7` (S3MediaStorage).

- [x] **1.1 S3 upload** (`crates/graphql/src/storage.rs`: `store_upload`/`put_object`)
  - SigV4 PUT (path-style `/{bucket}/{key}`), Django `AWS_*` env names verbatim
    (endpoint/key/secret/bucket/region/custom-domain); missing creds fail
    closed to local disk with a warn, never silent
  - DB stores the relative path only (`products/<stem>_<8hex><ext>`) — same
    values Django writes; no host in columns
  - Evidence: `storage::tests::sigv4_matches_independent_vector` (signature
    cross-checked with Python hashlib/hmac) + live PUT/DELETE against a mock
    S3 (auth header / path / body asserted)
- [x] **1.2 Public URL base** (`common::media_url` → `storage::public_base`)
  - S3 mode → `https://{AWS_MEDIA_CUSTOM_DOMAIN}` (wins over the stale
    `SALEOR_MEDIA_URL` in PixelStack `.env`); local mode → `SALEOR_MEDIA_URL`
    else localhost. All callers (gen/checkout/apps/avatar) inherit it
- [x] **1.3 Delete purges objects** — `delete_upload` (S3 DELETE, 404-tolerant
  / local unlink, lexical fence kept); wired into avatar clear, media delete,
  media bulk delete (per-id, survivors keep files), product delete + bulk
  (cascade). `delete_media`/`delete_product` also delete `Thumbnail` cache
  rows first (FK has no DB cascade — live-caught 23503 in e2e)
- [x] **1.4 Remote-URL fetch** (`storage::fetch_remote_image`: http/https only,
  10s timeout, `RUSTIFY_MAX_UPLOAD_BYTES` cap, magic-bytes image check;
  failure → external-only row, mutation succeeds — Django failed-task parity)
  - Also fixed: `create_media` now takes explicit `media_type`; image URLs →
    `IMAGE` (was wrongly `video` for every external URL), oembed hosts
    (youtube/vimeo/…) → `VIDEO`; stored uppercase like Django seeds
    (was lowercase)
- [x] **1.5 `/thumbnail/` + `/image/` endpoints** (`crates/graphql/src/thumbnail.rs`,
  mounted in `crates/server/src/main.rs`, slash + no-slash variants)
  - Contract: gid decode (double-encoding tolerant) → closest of
    `[32…4096]` → format allowlist (`avif`/`webp`, icons `webp`, explicit
    `original` 404s like upstream) → cached `Thumbnail` row → 302, else
    render (fit-inside box-fit, JPEG q70 / lossless-WebP / PNG / GIF-first-frame,
    TIFF→PNG) → key `thumbnails/<stem>_thumbnail_<size>.<ext>` (Django-identical)
    → store → row → 302. 503+Retry-After-60 for pending remotes, 400 for
    bomb-limit/ undecodable, 404 mapping 1:1. Unauthenticated. 302 FOUND.
  - Honest deltas: AVIF output → 400 (no encoder in build; dashboard never
    requests a format), WebP is lossless (no lossy encoder in pure Rust;
    same pixels, heavier bytes), EXIF orientation not applied, no
    `thumbnail_created` event (no consumer)
  - Evidence: `crates/graphql/tests/thumbnail_flow.rs::thumbnail_end_to_end`
    (live PG: render 200x100→64x32, cache hit, closest-size 60→64, webp,
    original, 404×4, avif 400, pending 503) + live server 302s on seed data
    (`ProductMedia:2` → `…_thumbnail_64.png`, 300→256 webp, `/image/` →
    original) + rendition followed to HTTP 200 `image/png`
  - Gates after all Phase 1 work: `cargo check` 0 errors (only pre-existing
    warnings), graphql + db suites green, harness **458/458** (4th run)

Django reference: `saleor-3.23.33/saleor/graphql/product/mutations/product/product_media_create.py:72-150`,
`saleor/thumbnail/views.py:59` (`handle_thumbnail`), `saleor/core/storages.py:7` (S3MediaStorage).

How it works today (Django): dashboard `productMediaCreate` (multipart) →
`S3MediaStorage` PUT to MinIO `saleor-media` (`products/<name>_<hash>.<ext>`) →
URL `https://media.<domain>/saleor-media/<path>`. Thumbnails: on-demand via
`/thumbnail/<id>/<size>/` (Traefik routes it to the engine), generated once,
stored back to MinIO, then redirect. Remote-URL uploads: celery
`fetch_product_media_image_task` downloads async.

- [x] **1.1 S3 upload in Rust uploads** (`crates/graphql/src/uploads.rs:233`
      `save_upload` writes LOCAL disk today)
  - PUT to MinIO (`saleor-media` bucket, same `products/<stem>_<hash><ext>`
    naming so DB values stay identical to Django's)
  - Config via env: endpoint / bucket / access keys / public base URL
    (mirror `AWS_*`/`MINIO_*` conventions, never hardcoded)
  - DB `image` value must equal what Django would write (path only, no host)
  - Proof: upload via dashboard → object appears in MinIO → URL loads on CDN
- [x] **1.2 Public URL base** (`ProductMedia.url`, `thumbnail` resolvers)
  - Serve MinIO-based URLs, not engine-local `/media/`
  - Keep local-disk mode behind an env flag for dev/offline
- [x] **1.3 `productMediaDelete` purges the object** (row delete exists;
      verify the MinIO object is removed too, else orphaned files pile up)
- [x] **1.4 Remote-URL fetch** (`create_media` stores `external_url` only today,
      `crates/db/src/catalog_writes.rs:1149`)
  - Django downloads via celery task; implement worker/sync fetch into MinIO
  - Until then: document hotlink behavior (remote death = broken image)
- [x] **1.5 `/thumbnail/<id>/<size>[/<format>]` endpoint** (Rust has none;
      returns first-image URL with no resizing today)
  - Closest-size matching, format allowlist, cache-as-Thumbnail-rows (or
    deterministic MinIO paths), redirect when cached
  - Any image crate is fine; exact pixel-parity with versatileimagefield
    is NOT required, size contract is

## Phase 2 — Beat/worker gaps [x] DONE 2026-10-01 (all Real, all e2e-tested)

Rust modes (`crates/server/src/modes.rs`): `api` + `worker` (webhook outbox)
+ `beat` (only `delete-expired-reservations`, `delete-expired-checkouts`,
`delete-old-export-files`, `checkout-automatic-completion` are Real).
No Redis needed anywhere (mail is inline SMTP).

- [x] **2.1 `recalculate-discounted-price` (every 30s)** 🔴 closest to a blocker
  - Without it, variant listing `discounted_price_amount` goes stale after
    any promotion change → display prices wrong (cart totals are live, so no
    money bug — but wrong shelf prices)
  - Port `update_variant_relations_for_active_promotion_rules_task` +
    `recalculate_discounted_price_for_products_task` against our tables
- [x] **2.2 `expire-orders` (every 5min)** — unpaid order expiry; check what
  the custom API reconciler already covers before duplicating
- [x] **2.3 `release-funds-for-abandoned-checkouts` (every 10min)** — frees
  authorized money on dead checkouts; money-adjacent, do not skip silently
- [x] **2.4 `handle-promotion-toggle`** — scheduled promo on/off
- [x] **2.5 `deactivate-expired-gift-cards`** + `deactivate-preorder` — when
  gift cards / preorders go live per shop
- [x] **2.6 Hygiene (low priority)**: empty allocations, event payloads,
  search vectors (custom API uses materialized views — likely N/A),
  export files, removed apps

## Phase 3 — custom-api goes multi-engine (PixelStack repo) [x] DONE 2026-10-01

Today: `SALEOR_API_URL` inline in ~10 call sites, single `saleorDb` pool
(`config/saleor-db.ts`), views-refresh + bootstrapper + webhook receiver
all assume ONE Saleor. Target: shop registry → per-request engine.

- [x] **3.1 Centralize engine addressing**
  - One `saleor-client` module owns the engine URL (kill the inline
    `process.env.SALEOR_API_URL || 'http://saleor-api:8000/graphql/'`
    copies in `cartUnified.controller.ts`, `payment.checkout.controller.ts`,
    `ensure-delivery-method.service.js`, `saleor-wrapper.service.ts`, …)
- [x] **3.2 Shop registry** (env JSON for 5 shops now, DB table later)
  - Per shop: engine GraphQL URL, Saleor DB connection, channel slug,
    webhook secret, admin token, MinIO prefix/bucket, ZarinPal merchant
- [x] **3.3 Request→shop resolution** (domain/header, ONE choke point feeding
  `RequestContextStore` alongside the existing `channelSlug`)
- [x] **3.4 Per-shop Saleor DB pools** (`saleorDb` becomes a pool map;
  views-refresh + `ensureSaleorViews` loop over shops; bootstrapper per shop)
- [x] **3.5 Webhook receiver identifies the shop** (per-shop HMAC secret;
      unknown shop → reject, never default-route)
- [x] **3.6 Reconciler/payment/logistics run in shop context** (engine URL +
      pool + merchant resolved per order, never global)
- [x] **3.7 Tenant config choke point** (`config/shop-settings.ts`, 2026-10-01):
      118 env vars inventoried — infra (ports/pools/JWT/tuning) stays
      `process.env`, everything seller-varying resolves per request as
      `<BASE>_<SHOP>` → legacy `<BASE>` via the secret store (DB + env,
      cluster-broadcast). Migrated: ZarinPal creds/sandbox, callback/return
      URLs, app tokens, SMS (per-shop clients), email (per-shop SMTP +
      brand), logistics (PostEx/Amadast/print), Google OAuth (per-shop),
      Strapi/CMS + revalidate/preview secrets, media/front URLs, SEO brand,
      order expiry, cache TTLs. `shop_id` stamped on carts/orders (migration
      0021 + drizzle-kit snapshot). Admin: `PUT /environment/:key` accepts
      `<BASE>_<SHOP>` + `GET /shops[/:id/settings]` (masked).
      Evidence: API `tsc` clean, vitest 118 pass / 4 pre-existing fails
      (baseline-identical: auth JWT, ZarinPal mocks, layout Redis-mock).

## Phase 4 — 5-shop rollout (one 8GB box) [x] COMPOSE+DONE 2026-10-01 — cutover itself is ops, per shop

## Phase 5 — Catalog-mode shops (no Saleor) [x] DONE 2026-10-01
For shops that only need a beautiful catalog (cafes etc.): no engine, no
Saleor DB, no worker/beat — just rows + content.
- Backend: `ShopMode full|catalog` + `locales` + `brand` in registry
  (`SHOP_MODE/SHOP_LOCALES/SHOP_BRAND` secret overrides, no redeploy);
  `catalog_items` table (migration 0022, drizzle-kit snapshot) with
  public reads (`GET /api/catalog/items[/:slug|/categories]`, shop-scoped
  via request context) + admin CRUD (`/api/admin/catalog?shop=`); public
  `GET /api/shops/resolve?host=` (no secrets) + admin `GET /shops` now
  returns the runtime profile.
- Web: middleware resolves shop per host (60s cache, fail-open legacy);
  stamps `x-shop-*` headers; catalog mode guards cart/checkout/payment/
  order-success/dashboard (redirect home); single-locale shops get
  prefix-less canonical URLs (internal rewrite, wrong-locale redirect);
  `ShopProvider` + `useShop()`; cart fetch skipped, CartButton/account/
  LanguageSwitcher hidden as appropriate; catalog list/detail from
  `/api/catalog` (own cards + gallery detail, price only if set);
  runtime brand (logo/name) + `--shop-primary` CSS var.
- Strapi stays SINGLE (already shared — no new servers): per-shop pages via
  `<shopId>--<slug>` convention with bare-slug fallback (no Strapi schema
  change needed); layout Redis keys shop-scoped (`layout:<kind>:<shop>:<slug>`,
  purge covers legacy + `*`); sellers never touch Strapi (admin-vip only).
- White-label admin page (`/white-label`): shop picker, mode + languages,
  brand editor, per-shop secrets (masked, `BASE_SHOP` keys) — all on the
  existing secret-store endpoints, live on next request.
- Speed (zero-loss design): no client-side shop fetching; middleware resolve
  is 1 cached fetch/min/host; ISR-incompatible per-host pages stay edge-
  cached (Varnish keyed by host, s-maxage preserved — incl. a fix applying
  cache policy to the REWRITTEN path so single-locale shops keep caching).
- Evidence: API `tsc` clean, web `tsc` clean, web `pnpm build` exit 0,
  admin-vip `vite build` ok, API vitest unchanged (118 pass / 4 pre-existing
  fails: auth JWT, ZarinPal mocks, layout Redis-mock).
- admin-vip content entry: `/white-label` (mode/locales/brand/secrets) +
  `/catalog` (items CRUD + incoming orders board, shop-scoped) pages,
  `/media` tenant library, launcher cards, all on existing endpoints.
- Cafe order flow (no Saleor, no account): `POST /api/catalog/checkout`
  (server-side re-price from `catalog_items`, snapshot order row with
  `metadata.source='catalog'`, ZarinPal direct via shared payment service +
  existing callback/reconcile machinery); web localStorage cart, catalog
  cart page branch, guest checkout form → gateway redirect; order-success
  works (generic status poll); seller board in `/catalog` Orders tab.
- Tenant media library (sellers never touch Strapi — Strapi admin is
  single-tenant by design; iframe-embedding changes nothing): `media_assets`
  table (migration 0023), per-shop MinIO buckets (auto-created + public
  policy on first upload), `POST/GET/DELETE /api/media/assets` (shop from
  request context, never params; auth-required), admin-vip `/media` page
  (grid/upload/copy-URL/delete). Logo/brand images come from here.

Proven numbers: Rust release idle ~8MB / load ~58MB / 1000-conn spike
~101MB (`BENCHMARK.md`); Python slice today ≈ 1.7GB limits. One engine
serves ~3.3k rps — all 5 shops fit in ONE engine many times over, but the
chosen model is one engine per shop for data independence.

- [x] **4.1 One PG cluster, 5 databases** (a database is cheap; a cluster
      is not — do NOT run 5 Postgres clusters)
- [x] **4.2 Shared Redis/MinIO/Traefik**, per-shop buckets or prefixes
- [x] **4.3 Per-shop engine container** (`RUSTIFY_DATABASE_URL` → its DB,
      `SALEOR_MEDIA_*` → its prefix; ~100-150MB limit each ≈ 750MB total)
- [ ] **4.4 Per-shop dashboard + webhook rows** in each engine DB pointing
      at the one custom-api (with that shop's secret)
- [ ] **4.5 Replace Python slice** (api 736M + worker 384M + db 384M +
      redis 128M limits ≈ 1.7GB freed vs ~750MB fleet cost)
- [ ] **4.6 Shadow mode per shop** before cutover (compare Rust vs Django
      responses on copied traffic), then cut shops over one by one

## Known non-blockers (documented, do not re-open without new evidence)- `Category.products` returns counts only (caller uses `totalCount` only)
- `before`/`last` pagination ignored (forward-only)
- `transactionReportEvent` caller text matches neither backend (dead code path)
- Search is ILIKE+tsvector (recall-favoring, no Saleor rank)
- `quantityAvailable`/`isAvailable` display sums are global; hard guards
  (validate/allocate) are channel-correct
- `orderUpdateMetadata` exists on neither backend (fixed caller-side)
- Thumbnail deltas (Phase 1, by design): AVIF output → 400; WebP lossless
  (not q70); EXIF orientation not applied; no oembed probe (host allowlist
  for VIDEO typing); no `thumbnail_created` event emission
- Dashboard harness is schema-validation (458/458); behavioral proof is
  the live E2E list in Phase 0, re-run after every engine change

## Parked decisions (explicitly NOT decided 2026-10-01 — revisit with real sellers)
- **User↔shop membership**: media/catalog uploads currently trust the request's
  shop context (host-derived). Any logged-in user on a shop's domain writes to
  that shop. Fine until real sellers arrive; then needs a membership model
  (which users administer which shops) before opening signups.
- **Strapi multi-tenancy**: rejected — Strapi admin is single-tenant by design;
  sellers get purpose-built admin-vip tools (media library, catalog, brand)
  instead. Strapi stays an internal composition tool.
- **Strapi-on-Rust**: parked. Honest take — Strapi's value IS its admin UI +
  plugin ecosystem; porting it means rebuilding both, which destroys the
  reason to use it. If Strapi ever becomes the bottleneck, replace it with a
  purpose-built lightweight CMS, don't port it.
- **Custom domains (seller-owned)**: Phase 1 = wildcard subdomains only.
  Phase 2 (Caddy on-demand TLS or equivalent) waits for the first paying
  customer who demands their own domain.
