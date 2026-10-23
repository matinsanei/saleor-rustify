# saleor-rustify

[English](#english) | [فارسی](#فارسی)

---

## فارسی

موتور تجارت الکترونیک Saleor را با Rust بازنویسی کرده‌ایم. همان PostgreSQL،
همان اسکیمای Django، بدون هیچ جابه‌جایی داده. فقط ایمیج را عوض کنید.

ما در پیکسل استک برای فروشگاه قائم پلاست به یک موتور تجارتی سریع، کم‌مصرف و
پایدار نیاز داشتیم؛ چیزی که روی سرور کوچک هم نفس بکشد. نسخه اصلی سالور روی
CPython است و زیر فشار، مصرف رم و تاخیر درخواست‌ها بارها خودش را نشان داد.
به جای پچ کردن مشکل، تصمیم گرفتیم آن را از ریشه حل کنیم: بازنویسی کامل در
Rust روی همان دیتابیس؛ تا کاربران دیگر هم بدون هزینه‌ای که ما کردیم از آن
استفاده کنند.

### چرا این پروژه خوب است

- هیچ migration نمی‌خواهد؛ همان دیتابیس Django سالور را مستقیم می‌خواند و می‌نویسد
- می‌توانید کنار Django اجرا کنید؛ هر دو روی همان ردیف‌ها کار می‌کنند (الگوی strangler-fig)
- مصرف رم حدود 58 مگابایت؛ به جای چند گیگ برای هر نود در Python
- داشبورد اصلی سالور با این موتور درست کار می‌کند؛ از مرور تا بازپرداخت
- سرعت و تاخیر با عدد ثابت‌شده؛ روش اندازه‌گیری در [BENCHMARK.md](BENCHMARK.md)

اگر از Saleor استفاده می‌کنید و از مصرف منابع آن خسته شده‌اید، این پروژه
برای شما نوشته شده است.

وب‌سایت رسمی سالور: [saleor.io](https://saleor.io) - منبع کد:
[github.com/saleor/saleor](https://github.com/saleor/saleor)

بقیه‌ی مستندات این فایل به انگلیسی است.

---

## English

Saleor, re-engineered in Rust. Same PostgreSQL. Zero migration. Just swap the image.

We run the [Ghaem Plastic](https://ghaemplast.com) storefront on [Saleor](https://github.com/saleor/saleor).
Saleor is a solid commerce engine, but it runs on CPython: GIL-bound requests,
row-by-row ORM overhead, and RAM bills that grow faster than traffic. This
project is a ground-up Rust rewrite that talks to the exact same PostgreSQL
schema Django created - same 147 tables, same constraints, same rows.
You point the binary at your database URL and go.

Both implementations can run side by side, reading and writing the same rows.
Django keeps serving while Rust takes over route by route - the classic
strangler-fig pattern, with no downtime window at any step.

Official links: [saleor.io](https://saleor.io) from the original project website, and
[github.com/saleor/saleor](https://github.com/saleor/saleor) on GitHub.

Trademark note: saleor-rustify is an independent community project, not
affiliated with or endorsed by Saleor Ltd. or Mirumee. The "Saleor" name is
used only to describe API and database compatibility. We will rename if the
Saleor team asks.

### Run it in 60 seconds

```bash
# 1) A Saleor database (yours, or spin one up and populate it):
cd saleor-core && cp .env.example .env && uv run poe migrate && uv run poe populatedb

# 2) The Rust engine against the same database - no dumps, no ETL:
export RUSTIFY_DATABASE_URL="postgres://saleor:saleor@localhost:5434/saleor"
export RSA_PRIVATE_KEY="$(cat crates/db/tests/testdata/test_rsa.pem)"  # dev only
./target/debug/saleor-rustify
# gRPC :50051 - GraphQL http://127.0.0.1:8000/graphql - metrics :9000

# 3) The stock Saleor Dashboard, pointed at Rust:
API_URL=http://localhost:8000/graphql pnpm --filter dashboard dev
```

Docker works too. Images are published on every push to `master`:

```bash
docker run --rm -p 50051:50051 -p 8000:8000 \
  -e RUSTIFY_DATABASE_URL="postgres://saleor:saleor@host.docker.internal:5434/saleor" \
  ghcr.io/matinsanei/saleor-rustify:latest
# other modes: worker, beat, check, migrate
```

### Measured results

Numbers below are from September 2026, release binary against a local Postgres.
Methodology and benchmark history live in [BENCHMARK.md](BENCHMARK.md).

- binary size: about 60.5MB stripped
- k6, 200 concurrent users: 3.28k requests/s, p50 around 41ms, p95 around 96ms, RSS about 58MB
- head to head with CPython on catalog reads: roughly 20x throughput, 19x better latency

| Signal | Value | How we know |
|---|---|---|
| GraphQL surface | 91 queries / 346 mutations live; every root resolves | introspection diff against `schema-main.graphql` |
| Dashboard compat | 458 of 458 operations validate | `scripts/verify_dashboard_ops.py` |
| Behavioral contracts | 171 of 171 pass | `cargo test -p saleor-rustify-db` |
| Money math | `rust_decimal` everywhere, decimals cross the wire as strings | parity with Django serializers |

### What works today

The stock Saleor Dashboard runs end to end on this backend: browse, checkout,
pay, fulfill, refund, plus the full back office. The complete item-by-item
ledger is in [docs/SALEOR_PARITY_CHECKLIST.md](docs/SALEOR_PARITY_CHECKLIST.md).
Short version:

- Checkout: new API plus legacy names, promos, delivery options, payment create
- Money: transactions, Stripe intents, legacy payments, granted refunds - one ledger
- Orders: confirm, capture, refund, void, mark-paid, fulfillments, returns, drafts, bulk import
- Gift cards, vouchers, promotions, channel and tax configuration
- Catalog: products, variants, categories, collections, attributes, media, translations
- Accounts: staff, customers, groups, auth with throttling, real SMTP mail
- Apps: webhooks with HMAC and retries, app installs, plugins, CSV exports
- An AI crate on top: pg_trgm search, co-occurrence recommendations, grounded chat

Known gaps, each documented instead of hidden: multipart upload (`fileUpload`),
the `checkouts` list query, Adyen and secondary gateways, tax providers,
XLSX/filtered exports, replacement orders, TLS mail auth.

### Architecture

```
crates/proto    protobuf contracts (tonic)
crates/core     pure domain logic - money, PSP, discounts
crates/db       SeaORM entities 1:1 over the real Django schema
crates/graphql  thin generated GraphQL BFF
crates/server   tonic gRPC (21 services) + Axum GraphQL
                modes: api, worker, beat, check, migrate, seed
```

Two rules keep it sane. First, GraphQL is a thin translation layer over the
same `db` functions gRPC uses, so money and stock math exists exactly once.
Second, `gen.rs` is generated from Saleor's own published schema and is never
hand-edited. Every write is permission-gated.

Agent documentation lives in [AGENT.md](AGENT.md) - it lists every trap this
project has already fallen into, so you don't have to repeat them.

### Why you should care

If you operate Saleor today, the RAM line on your invoice and the CPython
profile under load are probably familiar. We wrote this for our own store
first and are now opening it up. If you end up running it in production,
a GitHub star genuinely helps the project reach the next operator.

### Contributing

PRs welcome. House rules: no `float` for money, no `SELECT *` (tsvector and
INTERVAL columns mistype with it), every domain PR ships contract tests, and
every behavior cites the Saleor test it mirrors. Read [AGENT.md](AGENT.md)
first.

Issues: [github.com/matinsanei/saleor-rustify](https://github.com/matinsanei/saleor-rustify)

### License

AGPL-3.0. See [LICENSE](LICENSE). Saleor core is BSD-3-Clause; this project
copies no Saleor code - it is a clean reimplementation against the published
GraphQL schema and the SQL tables Django manages.
