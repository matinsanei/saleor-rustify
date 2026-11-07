//! Marketplace Database Schema DDL & Migration
//!
//! Provides automated idempotent schema generation and index setup for the
//! unified PostgreSQL marketplace & SDUI CMS engine.

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use crate::Result;

pub const MARKETPLACE_SCHEMA_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS vendors (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL,
    slug VARCHAR(255) NOT NULL UNIQUE,
    name VARCHAR(255) NOT NULL,
    bio TEXT,
    logo_url TEXT,
    banner_url TEXT,
    commission_rate DECIMAL(5, 4) NOT NULL DEFAULT 0.1000,
    is_verified BOOLEAN NOT NULL DEFAULT FALSE,
    is_active BOOLEAN NOT NULL DEFAULT TRUE,
    payout_account_info JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_vendors_user_id ON vendors(user_id);
CREATE INDEX IF NOT EXISTS idx_vendors_is_active ON vendors(is_active);

CREATE TABLE IF NOT EXISTS store_layouts (
    id UUID PRIMARY KEY,
    vendor_id UUID NOT NULL UNIQUE REFERENCES vendors(id) ON DELETE CASCADE,
    blocks JSONB NOT NULL DEFAULT '[]'::jsonb,
    theme_tokens JSONB NOT NULL DEFAULT '{}'::jsonb,
    is_published BOOLEAN NOT NULL DEFAULT TRUE,
    version INT NOT NULL DEFAULT 1,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_store_layouts_vendor_id ON store_layouts(vendor_id);

CREATE TABLE IF NOT EXISTS products (
    id UUID PRIMARY KEY,
    vendor_id UUID NOT NULL REFERENCES vendors(id) ON DELETE CASCADE,
    title VARCHAR(255) NOT NULL,
    slug VARCHAR(255) NOT NULL UNIQUE,
    description TEXT,
    category_id UUID,
    is_published BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    rating_avg DECIMAL(3, 2) NOT NULL DEFAULT 5.00,
    sales_count INT NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_products_vendor_id ON products(vendor_id);
CREATE INDEX IF NOT EXISTS idx_products_vendor_published ON products(vendor_id, is_published);
CREATE INDEX IF NOT EXISTS idx_products_category_id ON products(category_id);

CREATE TABLE IF NOT EXISTS product_variants (
    id UUID PRIMARY KEY,
    product_id UUID NOT NULL REFERENCES products(id) ON DELETE CASCADE,
    sku VARCHAR(255) NOT NULL UNIQUE,
    name VARCHAR(255) NOT NULL,
    price_amount DECIMAL(12, 2) NOT NULL,
    currency VARCHAR(10) NOT NULL DEFAULT 'USD',
    compare_at_price DECIMAL(12, 2),
    stock_quantity INT NOT NULL DEFAULT 0,
    attributes JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_product_variants_product_id ON product_variants(product_id);

CREATE TABLE IF NOT EXISTS product_media (
    id UUID PRIMARY KEY,
    product_id UUID NOT NULL REFERENCES products(id) ON DELETE CASCADE,
    media_type VARCHAR(50) NOT NULL DEFAULT 'IMAGE',
    url TEXT NOT NULL,
    alt_text TEXT,
    sort_order INT NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_product_media_product_sort ON product_media(product_id, sort_order);

CREATE TABLE IF NOT EXISTS marketplace_carts (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_marketplace_carts_user_id ON marketplace_carts(user_id);

CREATE TABLE IF NOT EXISTS marketplace_cart_items (
    id UUID PRIMARY KEY,
    cart_id UUID NOT NULL REFERENCES marketplace_carts(id) ON DELETE CASCADE,
    product_variant_id UUID NOT NULL REFERENCES product_variants(id) ON DELETE CASCADE,
    vendor_id UUID NOT NULL REFERENCES vendors(id) ON DELETE CASCADE,
    quantity INT NOT NULL DEFAULT 1,
    price_amount DECIMAL(12, 2) NOT NULL,
    currency VARCHAR(10) NOT NULL DEFAULT 'USD',
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uq_cart_variant UNIQUE (cart_id, product_variant_id)
);

CREATE INDEX IF NOT EXISTS idx_cart_items_cart_id ON marketplace_cart_items(cart_id);
CREATE INDEX IF NOT EXISTS idx_cart_items_vendor_id ON marketplace_cart_items(vendor_id);

CREATE TABLE IF NOT EXISTS marketplace_orders (
    id UUID PRIMARY KEY,
    buyer_id UUID NOT NULL,
    total_gross_amount DECIMAL(12, 2) NOT NULL,
    payment_status VARCHAR(50) NOT NULL DEFAULT 'PENDING',
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_marketplace_orders_buyer_id ON marketplace_orders(buyer_id);

CREATE TABLE IF NOT EXISTS vendor_sub_orders (
    id UUID PRIMARY KEY,
    parent_order_id UUID NOT NULL REFERENCES marketplace_orders(id) ON DELETE CASCADE,
    vendor_id UUID NOT NULL REFERENCES vendors(id) ON DELETE CASCADE,
    subtotal_amount DECIMAL(12, 2) NOT NULL,
    commission_amount DECIMAL(12, 2) NOT NULL,
    vendor_payout_amount DECIMAL(12, 2) NOT NULL,
    fulfillment_status VARCHAR(50) NOT NULL DEFAULT 'PENDING',
    tracking_number VARCHAR(255),
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_vendor_sub_orders_parent_id ON vendor_sub_orders(parent_order_id);
CREATE INDEX IF NOT EXISTS idx_vendor_sub_orders_vendor_status ON vendor_sub_orders(vendor_id, fulfillment_status);

CREATE TABLE IF NOT EXISTS order_items (
    id UUID PRIMARY KEY,
    sub_order_id UUID NOT NULL REFERENCES vendor_sub_orders(id) ON DELETE CASCADE,
    product_variant_id UUID NOT NULL,
    product_title VARCHAR(255) NOT NULL,
    variant_name VARCHAR(255) NOT NULL,
    sku VARCHAR(255) NOT NULL,
    quantity INT NOT NULL,
    unit_price_amount DECIMAL(12, 2) NOT NULL,
    total_price_amount DECIMAL(12, 2) NOT NULL,
    currency VARCHAR(10) NOT NULL DEFAULT 'USD'
);

CREATE INDEX IF NOT EXISTS idx_order_items_sub_order_id ON order_items(sub_order_id);
"#;

/// Apply marketplace schema DDL migration to the database
pub async fn apply_marketplace_migrations(db: &DatabaseConnection) -> Result<()> {
    // Split on statements or execute directly if backend supports multi-statement execution
    let stmts: Vec<&str> = MARKETPLACE_SCHEMA_DDL
        .split(';')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    for stmt_sql in stmts {
        let stmt = Statement::from_string(DbBackend::Postgres, stmt_sql.to_string());
        db.execute(stmt).await?;
    }

    Ok(())
}
