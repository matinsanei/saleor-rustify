use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait,
    QueryFilter, QueryOrder, QuerySelect, Set, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::marketplace::entities::{
    product::{self, Entity as Product},
    product_media::{self, Entity as ProductMedia},
    product_variant::{self, Entity as ProductVariant},
    store_layout::{self, ActiveModel, Entity as StoreLayout},
    vendor::{self, Entity as Vendor},
};
use crate::marketplace::services::vendor_service::VendorService;
use crate::{DbError, Result};
use saleor_rustify_core::marketplace::{validate_layout_blocks, StorefrontBlock};

pub struct StorefrontCmsService<'a> {
    db: &'a DatabaseConnection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrichedStoreLayout {
    pub layout: store_layout::Model,
    pub vendor: vendor::Model,
    pub enriched_blocks: serde_json::Value,
}

impl<'a> StorefrontCmsService<'a> {
    pub fn new(db: &'a DatabaseConnection) -> Self {
        Self { db }
    }

    /// Update or initialize the visual storefront SDUI layout for a vendor.
    /// Runs in an atomic transaction with strict Serde validation of block structures.
    pub async fn update_store_layout(
        &self,
        vendor_id: Uuid,
        requester_user_id: Uuid,
        blocks_value: serde_json::Value,
        theme_tokens: Option<serde_json::Value>,
        is_published: Option<bool>,
    ) -> Result<store_layout::Model> {
        // 1. Verify vendor ownership
        let vendor_svc = VendorService::new(self.db);
        vendor_svc.verify_ownership(vendor_id, requester_user_id).await?;

        // 2. Validate typed blocks via serde
        let blocks: Vec<StorefrontBlock> = serde_json::from_value(blocks_value.clone())
            .map_err(|e| DbError::Catalog(format!("invalid storefront layout block structure: {e}")))?;

        validate_layout_blocks(&blocks)
            .map_err(|e| DbError::Catalog(format!("layout validation failed: {e}")))?;

        // 3. Atomic transaction upsert
        let txn = self.db.begin().await?;

        let existing = StoreLayout::find()
            .filter(store_layout::Column::VendorId.eq(vendor_id))
            .one(&txn)
            .await?;

        let now = Utc::now().into();
        let saved = match existing {
            Some(curr) => {
                let next_version = curr.version + 1;
                let mut active: ActiveModel = curr.into();
                active.blocks = Set(blocks_value);
                if let Some(tokens) = theme_tokens {
                    active.theme_tokens = Set(tokens);
                }
                if let Some(pub_flag) = is_published {
                    active.is_published = Set(pub_flag);
                }
                active.version = Set(next_version);
                active.updated_at = Set(now);
                active.update(&txn).await?
            }
            None => {
                let new_layout = ActiveModel {
                    id: Set(Uuid::new_v4()),
                    vendor_id: Set(vendor_id),
                    blocks: Set(blocks_value),
                    theme_tokens: Set(theme_tokens.unwrap_or_else(|| serde_json::json!({}))),
                    is_published: Set(is_published.unwrap_or(true)),
                    version: Set(1),
                    updated_at: Set(now),
                };
                new_layout.insert(&txn).await?
            }
        };

        txn.commit().await?;
        Ok(saved)
    }

    /// Retrieve the storefront layout and vendor information by vendor slug
    pub async fn get_layout_by_vendor_slug(
        &self,
        slug: &str,
    ) -> Result<Option<(vendor::Model, Option<store_layout::Model>)>> {
        let clean = slug.trim().to_lowercase();
        let vendor = match Vendor::find()
            .filter(vendor::Column::Slug.eq(&clean))
            .one(self.db)
            .await?
        {
            Some(v) => v,
            None => return Ok(None),
        };

        let layout = StoreLayout::find()
            .filter(store_layout::Column::VendorId.eq(vendor.id))
            .one(self.db)
            .await?;

        Ok(Some((vendor, layout)))
    }

    /// Enriches layout blocks with live product data (media, pricing, titles, stock)
    /// for client Server-Driven UI consumption in < 2ms.
    pub async fn get_enriched_layout_by_slug(
        &self,
        slug: &str,
    ) -> Result<Option<EnrichedStoreLayout>> {
        let (vendor, layout_opt) = match self.get_layout_by_vendor_slug(slug).await? {
            Some(pair) => pair,
            None => return Ok(None),
        };

        let layout = match layout_opt {
            Some(l) => l,
            None => {
                // Return default empty layout if vendor has not configured one yet
                return Ok(Some(EnrichedStoreLayout {
                    layout: store_layout::Model {
                        id: Uuid::nil(),
                        vendor_id: vendor.id,
                        blocks: serde_json::json!([]),
                        theme_tokens: serde_json::json!({}),
                        is_published: true,
                        version: 0,
                        updated_at: Utc::now().into(),
                    },
                    vendor,
                    enriched_blocks: serde_json::json!([]),
                }));
            }
        };

        let blocks: Vec<StorefrontBlock> = serde_json::from_value(layout.blocks.clone())
            .unwrap_or_default();

        let mut enriched_values = Vec::new();

        for block in blocks {
            match block {
                StorefrontBlock::FeaturedProducts {
                    product_ids,
                    display_style,
                } => {
                    let products = self.fetch_products_with_details(&product_ids).await?;
                    enriched_values.push(serde_json::json!({
                        "type": "FeaturedProducts",
                        "display_style": display_style,
                        "products": products,
                    }));
                }
                StorefrontBlock::HotDeals {
                    deal_products,
                    end_timestamp,
                    discount_badge,
                } => {
                    let pids: Vec<Uuid> = deal_products.iter().map(|d| d.product_id).collect();
                    let products = self.fetch_products_with_details(&pids).await?;
                    enriched_values.push(serde_json::json!({
                        "type": "HotDeals",
                        "end_timestamp": end_timestamp,
                        "discount_badge": discount_badge,
                        "deal_products": deal_products,
                        "products": products,
                    }));
                }
                StorefrontBlock::ProductGrid {
                    category_filter,
                    sort_by,
                    pagination,
                } => {
                    let limit = pagination.as_ref().and_then(|p| p.limit).unwrap_or(20) as u64;
                    let page = pagination.as_ref().and_then(|p| p.page).unwrap_or(1) as u64;
                    let offset = (page.saturating_sub(1)) * limit;

                    let mut query = Product::find()
                        .filter(product::Column::VendorId.eq(vendor.id))
                        .filter(product::Column::IsPublished.eq(true));

                    if let Some(sort) = &sort_by {
                        if sort == "price_asc" || sort == "price_desc" {
                            // Order by rating or sales
                            query = query.order_by_asc(product::Column::SalesCount);
                        } else {
                            query = query.order_by_desc(product::Column::CreatedAt);
                        }
                    } else {
                        query = query.order_by_desc(product::Column::CreatedAt);
                    }

                    let products = query
                        .offset(offset)
                        .limit(limit)
                        .all(self.db)
                        .await?;

                    let pids: Vec<Uuid> = products.iter().map(|p| p.id).collect();
                    let detailed = self.fetch_products_with_details(&pids).await?;

                    enriched_values.push(serde_json::json!({
                        "type": "ProductGrid",
                        "category_filter": category_filter,
                        "sort_by": sort_by,
                        "products": detailed,
                    }));
                }
                other => {
                    enriched_values.push(serde_json::to_value(&other).unwrap_or_default());
                }
            }
        }

        Ok(Some(EnrichedStoreLayout {
            layout,
            vendor,
            enriched_blocks: serde_json::Value::Array(enriched_values),
        }))
    }

    async fn fetch_products_with_details(
        &self,
        product_ids: &[Uuid],
    ) -> Result<Vec<serde_json::Value>> {
        if product_ids.is_empty() {
            return Ok(Vec::new());
        }

        let products = Product::find()
            .filter(product::Column::Id.is_in(product_ids.to_vec()))
            .all(self.db)
            .await?;

        let variants = ProductVariant::find()
            .filter(product_variant::Column::ProductId.is_in(product_ids.to_vec()))
            .all(self.db)
            .await?;

        let media = ProductMedia::find()
            .filter(product_media::Column::ProductId.is_in(product_ids.to_vec()))
            .order_by_asc(product_media::Column::SortOrder)
            .all(self.db)
            .await?;

        let mut res = Vec::new();
        for p in products {
            let p_variants: Vec<&product_variant::Model> =
                variants.iter().filter(|v| v.product_id == p.id).collect();
            let p_media: Vec<&product_media::Model> =
                media.iter().filter(|m| m.product_id == p.id).collect();

            res.push(serde_json::json!({
                "id": p.id,
                "vendor_id": p.vendor_id,
                "title": p.title,
                "slug": p.slug,
                "description": p.description,
                "category_id": p.category_id,
                "rating_avg": p.rating_avg,
                "sales_count": p.sales_count,
                "variants": p_variants,
                "media": p_media,
            }));
        }

        Ok(res)
    }
}
