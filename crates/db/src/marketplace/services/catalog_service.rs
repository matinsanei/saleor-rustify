use chrono::Utc;
use rust_decimal::Decimal;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, Set, TransactionTrait,
};
use uuid::Uuid;

use crate::marketplace::entities::{
    product::{self, ActiveModel as ProductActive, Entity as Product},
    product_media::{self, ActiveModel as MediaActive, Entity as ProductMedia},
    product_variant::{self, ActiveModel as VariantActive, Entity as ProductVariant},
};
use crate::{DbError, Result};

pub struct CatalogService<'a> {
    db: &'a DatabaseConnection,
}

pub struct CreateProductInput {
    pub vendor_id: Uuid,
    pub title: String,
    pub slug: Option<String>,
    pub description: Option<String>,
    pub category_id: Option<Uuid>,
    pub is_published: Option<bool>,
    // Initial variant info
    pub sku: String,
    pub variant_name: Option<String>,
    pub price_amount: Decimal,
    pub currency: Option<String>,
    pub compare_at_price: Option<Decimal>,
    pub stock_quantity: Option<i32>,
    pub attributes: Option<serde_json::Value>,
    // Initial media info
    pub media_urls: Option<Vec<String>>,
}

pub struct UpdateProductInput {
    pub title: Option<String>,
    pub description: Option<String>,
    pub category_id: Option<Uuid>,
    pub is_published: Option<bool>,
    pub price_amount: Option<Decimal>,
    pub compare_at_price: Option<Decimal>,
    pub stock_quantity: Option<i32>,
}

impl<'a> CatalogService<'a> {
    pub fn new(db: &'a DatabaseConnection) -> Self {
        Self { db }
    }

    /// Create a new marketplace product with initial variant and optional media
    pub async fn create_product(&self, input: CreateProductInput) -> Result<product::Model> {
        if input.title.trim().is_empty() {
            return Err(DbError::Catalog("product title cannot be empty".into()));
        }
        if input.sku.trim().is_empty() {
            return Err(DbError::Catalog("product variant SKU cannot be empty".into()));
        }

        let slug = input.slug.unwrap_or_else(|| {
            format!(
                "{}-{}",
                input.title.to_lowercase().replace(' ', "-"),
                &Uuid::new_v4().to_string()[..8]
            )
        });

        let txn = self.db.begin().await?;

        // Ensure unique slug
        let existing_slug = Product::find()
            .filter(product::Column::Slug.eq(&slug))
            .one(&txn)
            .await?;
        if existing_slug.is_some() {
            return Err(DbError::Catalog(format!("product slug '{slug}' already exists")));
        }

        // Ensure unique SKU
        let existing_sku = ProductVariant::find()
            .filter(product_variant::Column::Sku.eq(&input.sku))
            .one(&txn)
            .await?;
        if existing_sku.is_some() {
            return Err(DbError::Catalog(format!("variant sku '{}' already exists", input.sku)));
        }

        let product_id = Uuid::new_v4();
        let now = Utc::now().into();

        let prod_active = ProductActive {
            id: Set(product_id),
            vendor_id: Set(input.vendor_id),
            title: Set(input.title.trim().to_string()),
            slug: Set(slug),
            description: Set(input.description),
            category_id: Set(input.category_id),
            is_published: Set(input.is_published.unwrap_or(true)),
            created_at: Set(now),
            updated_at: Set(now),
            rating_avg: Set(Decimal::new(500, 2)), // 5.00
            sales_count: Set(0),
        };
        let created_prod = prod_active.insert(&txn).await?;

        // Create default variant
        let variant_active = VariantActive {
            id: Set(Uuid::new_v4()),
            product_id: Set(product_id),
            sku: Set(input.sku.trim().to_string()),
            name: Set(input.variant_name.unwrap_or_else(|| "Default".to_string())),
            price_amount: Set(input.price_amount),
            currency: Set(input.currency.unwrap_or_else(|| "USD".to_string())),
            compare_at_price: Set(input.compare_at_price),
            stock_quantity: Set(input.stock_quantity.unwrap_or(0)),
            attributes: Set(input.attributes.unwrap_or_else(|| serde_json::json!({}))),
            created_at: Set(now),
            updated_at: Set(now),
        };
        variant_active.insert(&txn).await?;

        // Create media if provided
        if let Some(media_urls) = input.media_urls {
            for (order, url) in media_urls.into_iter().enumerate() {
                let media_active = MediaActive {
                    id: Set(Uuid::new_v4()),
                    product_id: Set(product_id),
                    media_type: Set("IMAGE".to_string()),
                    url: Set(url),
                    alt_text: Set(None),
                    sort_order: Set(order as i32),
                };
                media_active.insert(&txn).await?;
            }
        }

        txn.commit().await?;
        Ok(created_prod)
    }

    /// Update product fields with strict vendor RBAC verification
    pub async fn update_product(
        &self,
        product_id: Uuid,
        vendor_id: Uuid,
        input: UpdateProductInput,
    ) -> Result<product::Model> {
        let txn = self.db.begin().await?;

        let prod = Product::find_by_id(product_id)
            .one(&txn)
            .await?
            .ok_or_else(|| DbError::Catalog(format!("product {product_id} not found")))?;

        if prod.vendor_id != vendor_id {
            return Err(DbError::Catalog(
                "unauthorized: product does not belong to your vendor account".into(),
            ));
        }

        let mut prod_active: ProductActive = prod.into();
        if let Some(t) = input.title {
            if t.trim().is_empty() {
                return Err(DbError::Catalog("product title cannot be empty".into()));
            }
            prod_active.title = Set(t.trim().to_string());
        }
        if let Some(d) = input.description {
            prod_active.description = Set(Some(d));
        }
        if let Some(c) = input.category_id {
            prod_active.category_id = Set(Some(c));
        }
        if let Some(pub_flag) = input.is_published {
            prod_active.is_published = Set(pub_flag);
        }
        prod_active.updated_at = Set(Utc::now().into());

        let updated_prod = prod_active.update(&txn).await?;

        // If price or stock quantity updated, update the first/default variant
        if input.price_amount.is_some() || input.stock_quantity.is_some() || input.compare_at_price.is_some() {
            let first_variant = ProductVariant::find()
                .filter(product_variant::Column::ProductId.eq(product_id))
                .one(&txn)
                .await?;

            if let Some(v) = first_variant {
                let mut v_active: VariantActive = v.into();
                if let Some(price) = input.price_amount {
                    v_active.price_amount = Set(price);
                }
                if let Some(compare) = input.compare_at_price {
                    v_active.compare_at_price = Set(Some(compare));
                }
                if let Some(stock) = input.stock_quantity {
                    v_active.stock_quantity = Set(stock);
                }
                v_active.updated_at = Set(Utc::now().into());
                v_active.update(&txn).await?;
            }
        }

        txn.commit().await?;
        Ok(updated_prod)
    }

    /// Delete a product with strict vendor RBAC verification
    pub async fn delete_product(&self, product_id: Uuid, vendor_id: Uuid) -> Result<()> {
        let prod = Product::find_by_id(product_id)
            .one(self.db)
            .await?
            .ok_or_else(|| DbError::Catalog(format!("product {product_id} not found")))?;

        if prod.vendor_id != vendor_id {
            return Err(DbError::Catalog(
                "unauthorized: product does not belong to your vendor account".into(),
            ));
        }

        Product::delete_by_id(product_id).exec(self.db).await?;
        Ok(())
    }

    /// Public marketplace search across products with filters
    pub async fn search_marketplace_products(
        &self,
        query_str: Option<String>,
        category_id: Option<Uuid>,
        vendor_id: Option<Uuid>,
        page: Option<i32>,
        page_size: Option<i32>,
    ) -> Result<(Vec<product::Model>, u64)> {
        let p = page.unwrap_or(1).max(1) as u64;
        let ps = page_size.unwrap_or(20).clamp(1, 100) as u64;

        let mut query = Product::find()
            .filter(product::Column::IsPublished.eq(true))
            .order_by_desc(product::Column::CreatedAt);

        if let Some(q) = query_str {
            let search_term = format!("%{}%", q.trim());
            query = query.filter(
                product::Column::Title
                    .like(&search_term)
                    .or(product::Column::Description.like(&search_term)),
            );
        }

        if let Some(cat) = category_id {
            query = query.filter(product::Column::CategoryId.eq(cat));
        }

        if let Some(v) = vendor_id {
            query = query.filter(product::Column::VendorId.eq(v));
        }

        let paginator = query.paginate(self.db, ps);
        let total = paginator.num_items().await?;
        let items = paginator.fetch_page(p - 1).await?;

        Ok((items, total))
    }

    pub async fn get_product_by_id(&self, product_id: Uuid) -> Result<Option<product::Model>> {
        let prod = Product::find_by_id(product_id).one(self.db).await?;
        Ok(prod)
    }

    pub async fn get_product_by_slug(&self, slug: &str) -> Result<Option<product::Model>> {
        let prod = Product::find()
            .filter(product::Column::Slug.eq(slug))
            .one(self.db)
            .await?;
        Ok(prod)
    }

    pub async fn get_variants_for_product(
        &self,
        product_id: Uuid,
    ) -> Result<Vec<product_variant::Model>> {
        let variants = ProductVariant::find()
            .filter(product_variant::Column::ProductId.eq(product_id))
            .all(self.db)
            .await?;
        Ok(variants)
    }

    pub async fn get_media_for_product(
        &self,
        product_id: Uuid,
    ) -> Result<Vec<product_media::Model>> {
        let media = ProductMedia::find()
            .filter(product_media::Column::ProductId.eq(product_id))
            .order_by_asc(product_media::Column::SortOrder)
            .all(self.db)
            .await?;
        Ok(media)
    }
}
