//! Marketplace & Native SDUI CMS GraphQL Resolvers
//!
//! Provides public storefront resolution, vendor studio layout & catalog mutations
//! with strict vendor RBAC, and unified cart with automatic multi-vendor checkout.

use async_graphql::*;
use rust_decimal::Decimal;
use std::str::FromStr;
use uuid::Uuid;

use crate::context::{Bearer, GqlContext};
use saleor_rustify_db::marketplace::{
    catalog_service::{CatalogService, CreateProductInput, UpdateProductInput},
    cart_service::CartService,
    marketplace_order_service::MarketplaceOrderService,
    storefront_cms_service::StorefrontCmsService,
    vendor_service::VendorService,
    product_media, product_variant, vendor,
};

// ============================================================================
// GraphQL Enums and Domain Output Types
// ============================================================================

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum FulfillmentStatus {
    Pending,
    Processing,
    Shipped,
    Delivered,
    Cancelled,
}

impl FromStr for FulfillmentStatus {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "PENDING" => Ok(Self::Pending),
            "PROCESSING" => Ok(Self::Processing),
            "SHIPPED" => Ok(Self::Shipped),
            "DELIVERED" => Ok(Self::Delivered),
            "CANCELLED" => Ok(Self::Cancelled),
            other => Err(format!("unknown fulfillment status: {other}")),
        }
    }
}

impl ToString for FulfillmentStatus {
    fn to_string(&self) -> String {
        match self {
            Self::Pending => "PENDING".into(),
            Self::Processing => "PROCESSING".into(),
            Self::Shipped => "SHIPPED".into(),
            Self::Delivered => "DELIVERED".into(),
            Self::Cancelled => "CANCELLED".into(),
        }
    }
}

#[derive(SimpleObject, Clone)]
pub struct GqlVendor {
    pub id: ID,
    pub user_id: ID,
    pub slug: String,
    pub name: String,
    pub bio: Option<String>,
    pub logo_url: Option<String>,
    pub banner_url: Option<String>,
    pub commission_rate: Decimal,
    pub is_verified: bool,
    pub is_active: bool,
    pub payout_account_info: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl From<vendor::Model> for GqlVendor {
    fn from(v: vendor::Model) -> Self {
        Self {
            id: ID(v.id.to_string()),
            user_id: ID(v.user_id.to_string()),
            slug: v.slug,
            name: v.name,
            bio: v.bio,
            logo_url: v.logo_url,
            banner_url: v.banner_url,
            commission_rate: v.commission_rate,
            is_verified: v.is_verified,
            is_active: v.is_active,
            payout_account_info: v.payout_account_info.map(|j| j.to_string()),
            created_at: v.created_at.to_string(),
            updated_at: v.updated_at.to_string(),
        }
    }
}

#[derive(SimpleObject, Clone)]
pub struct GqlStoreLayout {
    pub id: ID,
    pub vendor_id: ID,
    pub vendor_slug: String,
    pub blocks: Json<serde_json::Value>,
    pub theme_tokens: Json<serde_json::Value>,
    pub is_published: bool,
    pub version: i32,
    pub updated_at: String,
}

#[derive(SimpleObject, Clone)]
pub struct GqlProduct {
    pub id: ID,
    pub vendor_id: ID,
    pub title: String,
    pub slug: String,
    pub description: Option<String>,
    pub category_id: Option<ID>,
    pub is_published: bool,
    pub rating_avg: Decimal,
    pub sales_count: i32,
    pub variants: Vec<GqlProductVariant>,
    pub media: Vec<GqlProductMedia>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(SimpleObject, Clone)]
pub struct GqlProductVariant {
    pub id: ID,
    pub product_id: ID,
    pub sku: String,
    pub name: String,
    pub price_amount: Decimal,
    pub currency: String,
    pub compare_at_price: Option<Decimal>,
    pub stock_quantity: i32,
    pub attributes: Json<serde_json::Value>,
}

impl From<product_variant::Model> for GqlProductVariant {
    fn from(v: product_variant::Model) -> Self {
        Self {
            id: ID(v.id.to_string()),
            product_id: ID(v.product_id.to_string()),
            sku: v.sku,
            name: v.name,
            price_amount: v.price_amount,
            currency: v.currency,
            compare_at_price: v.compare_at_price,
            stock_quantity: v.stock_quantity,
            attributes: Json(v.attributes),
        }
    }
}

#[derive(SimpleObject, Clone)]
pub struct GqlProductMedia {
    pub id: ID,
    pub product_id: ID,
    pub media_type: String,
    pub url: String,
    pub alt_text: Option<String>,
    pub sort_order: i32,
}

impl From<product_media::Model> for GqlProductMedia {
    fn from(m: product_media::Model) -> Self {
        Self {
            id: ID(m.id.to_string()),
            product_id: ID(m.product_id.to_string()),
            media_type: m.media_type,
            url: m.url,
            alt_text: m.alt_text,
            sort_order: m.sort_order,
        }
    }
}

#[derive(SimpleObject, Clone)]
pub struct ProductConnection {
    pub items: Vec<GqlProduct>,
    pub total_count: u64,
    pub page: i32,
    pub page_size: i32,
}

#[derive(SimpleObject, Clone)]
pub struct GqlSubOrder {
    pub id: ID,
    pub parent_order_id: ID,
    pub vendor_id: ID,
    pub subtotal_amount: Decimal,
    pub commission_amount: Decimal,
    pub vendor_payout_amount: Decimal,
    pub fulfillment_status: FulfillmentStatus,
    pub tracking_number: Option<String>,
    pub items: Vec<GqlOrderItem>,
    pub created_at: String,
}

#[derive(SimpleObject, Clone)]
pub struct GqlOrderItem {
    pub id: ID,
    pub sub_order_id: ID,
    pub product_variant_id: ID,
    pub product_title: String,
    pub variant_name: String,
    pub sku: String,
    pub quantity: i32,
    pub unit_price_amount: Decimal,
    pub total_price_amount: Decimal,
    pub currency: String,
}

#[derive(SimpleObject, Clone)]
pub struct SubOrderConnection {
    pub items: Vec<GqlSubOrder>,
    pub total_count: u64,
    pub page: i32,
}

#[derive(SimpleObject, Clone)]
pub struct GqlCart {
    pub id: ID,
    pub user_id: ID,
    pub items: Vec<GqlCartItem>,
    pub total_amount: Decimal,
    pub currency: String,
}

#[derive(SimpleObject, Clone)]
pub struct GqlCartItem {
    pub id: ID,
    pub product_variant_id: ID,
    pub vendor_id: ID,
    pub quantity: i32,
    pub price_amount: Decimal,
    pub currency: String,
}

// ============================================================================
// Mutation Inputs & Payloads
// ============================================================================

#[derive(InputObject)]
pub struct VendorRegisterInput {
    pub slug: String,
    pub name: String,
    pub bio: Option<String>,
    pub logo_url: Option<String>,
    pub banner_url: Option<String>,
    pub commission_rate: Option<Decimal>,
}

#[derive(SimpleObject)]
pub struct VendorPayload {
    pub vendor: Option<GqlVendor>,
    pub errors: Vec<String>,
}

#[derive(InputObject)]
pub struct UpdateStoreLayoutInput {
    pub vendor_id: Option<ID>,
    pub blocks: Json<serde_json::Value>,
    pub theme_tokens: Option<Json<serde_json::Value>>,
    pub is_published: Option<bool>,
}

#[derive(SimpleObject)]
pub struct StoreLayoutPayload {
    pub layout: Option<GqlStoreLayout>,
    pub errors: Vec<String>,
}

#[derive(InputObject)]
pub struct MarketplaceProductCreateInput {
    pub vendor_id: Option<ID>,
    pub title: String,
    pub slug: Option<String>,
    pub description: Option<String>,
    pub category_id: Option<ID>,
    pub is_published: Option<bool>,
    pub sku: String,
    pub variant_name: Option<String>,
    pub price_amount: Decimal,
    pub currency: Option<String>,
    pub compare_at_price: Option<Decimal>,
    pub stock_quantity: Option<i32>,
    pub attributes: Option<Json<serde_json::Value>>,
    pub media_urls: Option<Vec<String>>,
}

#[derive(InputObject)]
pub struct MarketplaceProductUpdateInput {
    pub id: ID,
    pub vendor_id: Option<ID>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub category_id: Option<ID>,
    pub is_published: Option<bool>,
    pub price_amount: Option<Decimal>,
    pub compare_at_price: Option<Decimal>,
    pub stock_quantity: Option<i32>,
}

#[derive(SimpleObject)]
pub struct ProductPayload {
    pub product: Option<GqlProduct>,
    pub errors: Vec<String>,
}

#[derive(SimpleObject)]
pub struct SuccessPayload {
    pub success: bool,
    pub message: Option<String>,
    pub errors: Vec<String>,
}

#[derive(SimpleObject)]
pub struct SubOrderPayload {
    pub sub_order: Option<GqlSubOrder>,
    pub errors: Vec<String>,
}

#[derive(InputObject)]
pub struct CartAddItemInput {
    pub buyer_id: Option<ID>,
    pub product_variant_id: ID,
    pub quantity: i32,
}

#[derive(InputObject)]
pub struct CartRemoveItemInput {
    pub buyer_id: Option<ID>,
    pub product_variant_id: ID,
}

#[derive(SimpleObject)]
pub struct CartPayload {
    pub cart: Option<GqlCart>,
    pub errors: Vec<String>,
}

#[derive(SimpleObject)]
pub struct CheckoutPayload {
    pub parent_order_id: Option<ID>,
    pub total_gross_amount: Option<Decimal>,
    pub sub_orders: Vec<GqlSubOrder>,
    pub errors: Vec<String>,
}

// ============================================================================
// Auth & Context Extraction Helper
// ============================================================================

fn resolve_user_id(ctx: &Context<'_>, explicit_id: Option<&ID>) -> Result<Uuid> {
    if let Some(id) = explicit_id {
        if let Ok(u) = Uuid::parse_str(&id.0) {
            return Ok(u);
        }
    }

    let token = ctx
        .data_opt::<Bearer>()
        .map(|b| b.0.clone())
        .or_else(|| ctx.data_opt::<GqlContext>().and_then(|g| g.bearer.clone()));

    if let Some(t) = token {
        // Direct UUID token support
        if let Ok(u) = Uuid::parse_str(&t) {
            return Ok(u);
        }
        // Try decoding JWT
        if let Ok(claims) = saleor_rustify_core::auth::decode(&t) {
            if let Ok(u) = Uuid::parse_str(&claims.user_id) {
                return Ok(u);
            }
            if let Ok(num) = claims.user_id.parse::<u128>() {
                return Ok(Uuid::from_u128(num));
            }
        }
    }

    // Default fallback in non-enforcing local dev
    Ok(Uuid::nil())
}

// ============================================================================
// Query Resolvers
// ============================================================================

#[derive(Default)]
pub struct MarketplaceQuery;

#[Object]
impl MarketplaceQuery {
    /// Public Storefront Query: resolve vendor details by unique slug
    async fn vendor_by_slug(
        &self,
        ctx: &Context<'_>,
        slug: String,
    ) -> Result<Option<GqlVendor>> {
        let db = ctx.data::<GqlContext>()?.db().map_err(|e| Error::new(e.to_string()))?;
        let vendor_svc = VendorService::new(db);
        let v_opt = vendor_svc.get_by_slug(&slug).await.map_err(|e| Error::new(e.to_string()))?;
        Ok(v_opt.map(GqlVendor::from))
    }

    /// Public Storefront Query: returns enriched layout blocks with live product data
    /// formatted for instant Server-Driven UI (SDUI) rendering.
    async fn storefront_layout(
        &self,
        ctx: &Context<'_>,
        vendor_slug: String,
    ) -> Result<Option<GqlStoreLayout>> {
        let db = ctx.data::<GqlContext>()?.db().map_err(|e| Error::new(e.to_string()))?;
        let cms_svc = StorefrontCmsService::new(db);

        let enriched = match cms_svc.get_enriched_layout_by_slug(&vendor_slug).await.map_err(|e| Error::new(e.to_string()))? {
            Some(e) => e,
            None => return Ok(None),
        };

        Ok(Some(GqlStoreLayout {
            id: ID(enriched.layout.id.to_string()),
            vendor_id: ID(enriched.vendor.id.to_string()),
            vendor_slug: enriched.vendor.slug,
            blocks: Json(enriched.enriched_blocks),
            theme_tokens: Json(enriched.layout.theme_tokens),
            is_published: enriched.layout.is_published,
            version: enriched.layout.version,
            updated_at: enriched.layout.updated_at.to_string(),
        }))
    }

    /// Public Storefront Query: search products across marketplace with categories and vendor filter
    async fn marketplace_search(
        &self,
        ctx: &Context<'_>,
        query: Option<String>,
        category: Option<String>,
        vendor_id: Option<ID>,
        page: Option<i32>,
    ) -> Result<ProductConnection> {
        let db = ctx.data::<GqlContext>()?.db().map_err(|e| Error::new(e.to_string()))?;
        let catalog_svc = CatalogService::new(db);

        let cat_uuid = category.and_then(|c| Uuid::parse_str(&c).ok());
        let v_uuid = vendor_id.and_then(|v| Uuid::parse_str(&v.0).ok());

        let (prods, total) = catalog_svc
            .search_marketplace_products(query, cat_uuid, v_uuid, page, Some(20))
            .await
            .map_err(|e| Error::new(e.to_string()))?;

        let mut items = Vec::new();
        for p in prods {
            let variants = catalog_svc
                .get_variants_for_product(p.id)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(GqlProductVariant::from)
                .collect();

            let media = catalog_svc
                .get_media_for_product(p.id)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(GqlProductMedia::from)
                .collect();

            items.push(GqlProduct {
                id: ID(p.id.to_string()),
                vendor_id: ID(p.vendor_id.to_string()),
                title: p.title,
                slug: p.slug,
                description: p.description,
                category_id: p.category_id.map(|c| ID(c.to_string())),
                is_published: p.is_published,
                rating_avg: p.rating_avg,
                sales_count: p.sales_count,
                variants,
                media,
                created_at: p.created_at.to_string(),
                updated_at: p.updated_at.to_string(),
            });
        }

        Ok(ProductConnection {
            items,
            total_count: total,
            page: page.unwrap_or(1),
            page_size: 20,
        })
    }

    /// Vendor Studio Query: list sub-orders assigned to the vendor
    async fn vendor_sub_orders(
        &self,
        ctx: &Context<'_>,
        vendor_id: Option<ID>,
        status: Option<FulfillmentStatus>,
        page: Option<i32>,
    ) -> Result<SubOrderConnection> {
        let db = ctx.data::<GqlContext>()?.db().map_err(|e| Error::new(e.to_string()))?;
        let vendor_svc = VendorService::new(db);
        let user_id = resolve_user_id(ctx, None)?;

        // Resolve vendor
        let vendor = if let Some(vid) = vendor_id {
            let v_uuid = Uuid::parse_str(&vid.0).map_err(|_| Error::new("invalid vendor ID"))?;
            vendor_svc.verify_ownership(v_uuid, user_id).await.map_err(|e| Error::new(e.to_string()))?
        } else {
            vendor_svc
                .get_by_user_id(user_id)
                .await
                .map_err(|e| Error::new(e.to_string()))?
                .ok_or_else(|| Error::new("no vendor profile found for current user"))?
        };

        let order_svc = MarketplaceOrderService::new(db);
        let status_str = status.map(|s| s.to_string());
        let (sub_orders, total) = order_svc
            .list_vendor_sub_orders(vendor.id, status_str, page, Some(20))
            .await
            .map_err(|e| Error::new(e.to_string()))?;

        let mut gql_subs = Vec::new();
        for s in sub_orders {
            let items = order_svc
                .get_sub_order_items(s.id)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|it| GqlOrderItem {
                    id: ID(it.id.to_string()),
                    sub_order_id: ID(it.sub_order_id.to_string()),
                    product_variant_id: ID(it.product_variant_id.to_string()),
                    product_title: it.product_title,
                    variant_name: it.variant_name,
                    sku: it.sku,
                    quantity: it.quantity,
                    unit_price_amount: it.unit_price_amount,
                    total_price_amount: it.total_price_amount,
                    currency: it.currency,
                })
                .collect();

            let st = FulfillmentStatus::from_str(&s.fulfillment_status).unwrap_or(FulfillmentStatus::Pending);

            gql_subs.push(GqlSubOrder {
                id: ID(s.id.to_string()),
                parent_order_id: ID(s.parent_order_id.to_string()),
                vendor_id: ID(s.vendor_id.to_string()),
                subtotal_amount: s.subtotal_amount,
                commission_amount: s.commission_amount,
                vendor_payout_amount: s.vendor_payout_amount,
                fulfillment_status: st,
                tracking_number: s.tracking_number,
                items,
                created_at: s.created_at.to_string(),
            });
        }

        Ok(SubOrderConnection {
            items: gql_subs,
            total_count: total,
            page: page.unwrap_or(1),
        })
    }

    /// Buyer Cart Query
    async fn cart(&self, ctx: &Context<'_>, buyer_id: Option<ID>) -> Result<Option<GqlCart>> {
        let db = ctx.data::<GqlContext>()?.db().map_err(|e| Error::new(e.to_string()))?;
        let user_id = resolve_user_id(ctx, buyer_id.as_ref())?;

        let cart_svc = CartService::new(db);
        let pair = cart_svc.get_cart_with_items(user_id).await.map_err(|e| Error::new(e.to_string()))?;

        match pair {
            Some((cart, items)) => {
                let mut total = Decimal::ZERO;
                let currency = items.first().map(|i| i.currency.clone()).unwrap_or_else(|| "USD".into());

                let gql_items = items
                    .into_iter()
                    .map(|it| {
                        total += it.price_amount * Decimal::from(it.quantity);
                        GqlCartItem {
                            id: ID(it.id.to_string()),
                            product_variant_id: ID(it.product_variant_id.to_string()),
                            vendor_id: ID(it.vendor_id.to_string()),
                            quantity: it.quantity,
                            price_amount: it.price_amount,
                            currency: it.currency,
                        }
                    })
                    .collect();

                Ok(Some(GqlCart {
                    id: ID(cart.id.to_string()),
                    user_id: ID(cart.user_id.to_string()),
                    items: gql_items,
                    total_amount: total,
                    currency,
                }))
            }
            None => Ok(None),
        }
    }
}

// ============================================================================
// Mutation Resolvers
// ============================================================================

#[derive(Default)]
pub struct MarketplaceMutation;

#[Object]
impl MarketplaceMutation {
    /// Vendor Registration Mutation
    async fn vendor_register(
        &self,
        ctx: &Context<'_>,
        input: VendorRegisterInput,
    ) -> Result<VendorPayload> {
        let db = ctx.data::<GqlContext>()?.db().map_err(|e| Error::new(e.to_string()))?;
        let user_id = resolve_user_id(ctx, None)?;
        let vendor_svc = VendorService::new(db);

        match vendor_svc
            .register_vendor(
                user_id,
                input.slug,
                input.name,
                input.bio,
                input.logo_url,
                input.banner_url,
                input.commission_rate,
                None,
            )
            .await
        {
            Ok(v) => Ok(VendorPayload {
                vendor: Some(GqlVendor::from(v)),
                errors: vec![],
            }),
            Err(e) => Ok(VendorPayload {
                vendor: None,
                errors: vec![e.to_string()],
            }),
        }
    }

    /// Vendor Studio CMS Mutation: update Server-Driven UI storefront blocks
    async fn vendor_update_store_layout(
        &self,
        ctx: &Context<'_>,
        input: UpdateStoreLayoutInput,
    ) -> Result<StoreLayoutPayload> {
        let db = ctx.data::<GqlContext>()?.db().map_err(|e| Error::new(e.to_string()))?;
        let user_id = resolve_user_id(ctx, None)?;
        let vendor_svc = VendorService::new(db);

        let vendor = if let Some(vid) = &input.vendor_id {
            let v_uuid = Uuid::parse_str(&vid.0).map_err(|_| Error::new("invalid vendor ID"))?;
            vendor_svc.verify_ownership(v_uuid, user_id).await.map_err(|e| Error::new(e.to_string()))?
        } else {
            vendor_svc
                .get_by_user_id(user_id)
                .await
                .map_err(|e| Error::new(e.to_string()))?
                .ok_or_else(|| Error::new("no vendor profile found for current user"))?
        };

        let cms_svc = StorefrontCmsService::new(db);
        match cms_svc
            .update_store_layout(
                vendor.id,
                user_id,
                input.blocks.0,
                input.theme_tokens.map(|t| t.0),
                input.is_published,
            )
            .await
        {
            Ok(saved) => Ok(StoreLayoutPayload {
                layout: Some(GqlStoreLayout {
                    id: ID(saved.id.to_string()),
                    vendor_id: ID(saved.vendor_id.to_string()),
                    vendor_slug: vendor.slug,
                    blocks: Json(saved.blocks),
                    theme_tokens: Json(saved.theme_tokens),
                    is_published: saved.is_published,
                    version: saved.version,
                    updated_at: saved.updated_at.to_string(),
                }),
                errors: vec![],
            }),
            Err(e) => Ok(StoreLayoutPayload {
                layout: None,
                errors: vec![e.to_string()],
            }),
        }
    }

    /// Vendor Studio Mutation: create a product with variants and media
    async fn product_create(
        &self,
        ctx: &Context<'_>,
        input: MarketplaceProductCreateInput,
    ) -> Result<ProductPayload> {
        let db = ctx.data::<GqlContext>()?.db().map_err(|e| Error::new(e.to_string()))?;
        let user_id = resolve_user_id(ctx, None)?;
        let vendor_svc = VendorService::new(db);

        let vendor = if let Some(vid) = &input.vendor_id {
            let v_uuid = Uuid::parse_str(&vid.0).map_err(|_| Error::new("invalid vendor ID"))?;
            vendor_svc.verify_ownership(v_uuid, user_id).await.map_err(|e| Error::new(e.to_string()))?
        } else {
            vendor_svc
                .get_by_user_id(user_id)
                .await
                .map_err(|e| Error::new(e.to_string()))?
                .ok_or_else(|| Error::new("no vendor profile found for current user"))?
        };

        let category_id = input.category_id.and_then(|c| Uuid::parse_str(&c.0).ok());
        let catalog_svc = CatalogService::new(db);

        let create_input = CreateProductInput {
            vendor_id: vendor.id,
            title: input.title,
            slug: input.slug,
            description: input.description,
            category_id,
            is_published: input.is_published,
            sku: input.sku,
            variant_name: input.variant_name,
            price_amount: input.price_amount,
            currency: input.currency,
            compare_at_price: input.compare_at_price,
            stock_quantity: input.stock_quantity,
            attributes: input.attributes.map(|a| a.0),
            media_urls: input.media_urls,
        };

        match catalog_svc.create_product(create_input).await {
            Ok(p) => {
                let variants = catalog_svc
                    .get_variants_for_product(p.id)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(GqlProductVariant::from)
                    .collect();

                let media = catalog_svc
                    .get_media_for_product(p.id)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(GqlProductMedia::from)
                    .collect();

                Ok(ProductPayload {
                    product: Some(GqlProduct {
                        id: ID(p.id.to_string()),
                        vendor_id: ID(p.vendor_id.to_string()),
                        title: p.title,
                        slug: p.slug,
                        description: p.description,
                        category_id: p.category_id.map(|c| ID(c.to_string())),
                        is_published: p.is_published,
                        rating_avg: p.rating_avg,
                        sales_count: p.sales_count,
                        variants,
                        media,
                        created_at: p.created_at.to_string(),
                        updated_at: p.updated_at.to_string(),
                    }),
                    errors: vec![],
                })
            }
            Err(e) => Ok(ProductPayload {
                product: None,
                errors: vec![e.to_string()],
            }),
        }
    }

    /// Vendor Studio Mutation: update a product with vendor RBAC
    async fn product_update(
        &self,
        ctx: &Context<'_>,
        input: MarketplaceProductUpdateInput,
    ) -> Result<ProductPayload> {
        let db = ctx.data::<GqlContext>()?.db().map_err(|e| Error::new(e.to_string()))?;
        let user_id = resolve_user_id(ctx, None)?;
        let vendor_svc = VendorService::new(db);

        let vendor = if let Some(vid) = &input.vendor_id {
            let v_uuid = Uuid::parse_str(&vid.0).map_err(|_| Error::new("invalid vendor ID"))?;
            vendor_svc.verify_ownership(v_uuid, user_id).await.map_err(|e| Error::new(e.to_string()))?
        } else {
            vendor_svc
                .get_by_user_id(user_id)
                .await
                .map_err(|e| Error::new(e.to_string()))?
                .ok_or_else(|| Error::new("no vendor profile found for current user"))?
        };

        let product_id = Uuid::parse_str(&input.id.0).map_err(|_| Error::new("invalid product ID"))?;
        let category_id = input.category_id.and_then(|c| Uuid::parse_str(&c.0).ok());
        let catalog_svc = CatalogService::new(db);

        let update_input = UpdateProductInput {
            title: input.title,
            description: input.description,
            category_id,
            is_published: input.is_published,
            price_amount: input.price_amount,
            compare_at_price: input.compare_at_price,
            stock_quantity: input.stock_quantity,
        };

        match catalog_svc.update_product(product_id, vendor.id, update_input).await {
            Ok(p) => {
                let variants = catalog_svc
                    .get_variants_for_product(p.id)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(GqlProductVariant::from)
                    .collect();

                let media = catalog_svc
                    .get_media_for_product(p.id)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(GqlProductMedia::from)
                    .collect();

                Ok(ProductPayload {
                    product: Some(GqlProduct {
                        id: ID(p.id.to_string()),
                        vendor_id: ID(p.vendor_id.to_string()),
                        title: p.title,
                        slug: p.slug,
                        description: p.description,
                        category_id: p.category_id.map(|c| ID(c.to_string())),
                        is_published: p.is_published,
                        rating_avg: p.rating_avg,
                        sales_count: p.sales_count,
                        variants,
                        media,
                        created_at: p.created_at.to_string(),
                        updated_at: p.updated_at.to_string(),
                    }),
                    errors: vec![],
                })
            }
            Err(e) => Ok(ProductPayload {
                product: None,
                errors: vec![e.to_string()],
            }),
        }
    }

    /// Vendor Studio Mutation: delete product
    async fn product_delete(&self, ctx: &Context<'_>, id: ID) -> Result<SuccessPayload> {
        let db = ctx.data::<GqlContext>()?.db().map_err(|e| Error::new(e.to_string()))?;
        let user_id = resolve_user_id(ctx, None)?;
        let vendor_svc = VendorService::new(db);
        let vendor = vendor_svc
            .get_by_user_id(user_id)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .ok_or_else(|| Error::new("no vendor profile found for current user"))?;

        let product_id = Uuid::parse_str(&id.0).map_err(|_| Error::new("invalid product ID"))?;
        let catalog_svc = CatalogService::new(db);

        match catalog_svc.delete_product(product_id, vendor.id).await {
            Ok(_) => Ok(SuccessPayload {
                success: true,
                message: Some("product deleted successfully".into()),
                errors: vec![],
            }),
            Err(e) => Ok(SuccessPayload {
                success: false,
                message: None,
                errors: vec![e.to_string()],
            }),
        }
    }

    /// Vendor Studio Mutation: update fulfillment status and tracking info
    async fn vendor_update_fulfillment(
        &self,
        ctx: &Context<'_>,
        sub_order_id: ID,
        status: FulfillmentStatus,
        tracking: Option<String>,
    ) -> Result<SubOrderPayload> {
        let db = ctx.data::<GqlContext>()?.db().map_err(|e| Error::new(e.to_string()))?;
        let user_id = resolve_user_id(ctx, None)?;
        let vendor_svc = VendorService::new(db);
        let vendor = vendor_svc
            .get_by_user_id(user_id)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .ok_or_else(|| Error::new("no vendor profile found for current user"))?;

        let sub_id = Uuid::parse_str(&sub_order_id.0).map_err(|_| Error::new("invalid sub-order ID"))?;
        let order_svc = MarketplaceOrderService::new(db);

        match order_svc
            .vendor_update_fulfillment_tx(vendor.id, sub_id, status.to_string(), tracking)
            .await
        {
            Ok(s) => {
                let items = order_svc
                    .get_sub_order_items(s.id)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|it| GqlOrderItem {
                        id: ID(it.id.to_string()),
                        sub_order_id: ID(it.sub_order_id.to_string()),
                        product_variant_id: ID(it.product_variant_id.to_string()),
                        product_title: it.product_title,
                        variant_name: it.variant_name,
                        sku: it.sku,
                        quantity: it.quantity,
                        unit_price_amount: it.unit_price_amount,
                        total_price_amount: it.total_price_amount,
                        currency: it.currency,
                    })
                    .collect();

                let st = FulfillmentStatus::from_str(&s.fulfillment_status).unwrap_or(FulfillmentStatus::Pending);

                Ok(SubOrderPayload {
                    sub_order: Some(GqlSubOrder {
                        id: ID(s.id.to_string()),
                        parent_order_id: ID(s.parent_order_id.to_string()),
                        vendor_id: ID(s.vendor_id.to_string()),
                        subtotal_amount: s.subtotal_amount,
                        commission_amount: s.commission_amount,
                        vendor_payout_amount: s.vendor_payout_amount,
                        fulfillment_status: st,
                        tracking_number: s.tracking_number,
                        items,
                        created_at: s.created_at.to_string(),
                    }),
                    errors: vec![],
                })
            }
            Err(e) => Ok(SubOrderPayload {
                sub_order: None,
                errors: vec![e.to_string()],
            }),
        }
    }

    /// Buyer Cart Flow: add item to unified cart
    async fn cart_add_item(
        &self,
        ctx: &Context<'_>,
        input: CartAddItemInput,
    ) -> Result<CartPayload> {
        let db = ctx.data::<GqlContext>()?.db().map_err(|e| Error::new(e.to_string()))?;
        let user_id = resolve_user_id(ctx, input.buyer_id.as_ref())?;

        let variant_id = Uuid::parse_str(&input.product_variant_id.0)
            .map_err(|_| Error::new("invalid variant ID"))?;

        let cart_svc = CartService::new(db);
        if let Err(e) = cart_svc.add_item(user_id, variant_id, input.quantity).await {
            return Ok(CartPayload {
                cart: None,
                errors: vec![e.to_string()],
            });
        }

        // Return updated cart
        if let Ok(Some((cart, items))) = cart_svc.get_cart_with_items(user_id).await {
            let mut total = Decimal::ZERO;
            let currency = items.first().map(|i| i.currency.clone()).unwrap_or_else(|| "USD".into());

            let gql_items = items
                .into_iter()
                .map(|it| {
                    total += it.price_amount * Decimal::from(it.quantity);
                    GqlCartItem {
                        id: ID(it.id.to_string()),
                        product_variant_id: ID(it.product_variant_id.to_string()),
                        vendor_id: ID(it.vendor_id.to_string()),
                        quantity: it.quantity,
                        price_amount: it.price_amount,
                        currency: it.currency,
                    }
                })
                .collect();

            Ok(CartPayload {
                cart: Some(GqlCart {
                    id: ID(cart.id.to_string()),
                    user_id: ID(cart.user_id.to_string()),
                    items: gql_items,
                    total_amount: total,
                    currency,
                }),
                errors: vec![],
            })
        } else {
            Ok(CartPayload {
                cart: None,
                errors: vec!["failed to load updated cart".into()],
            })
        }
    }

    /// Buyer Cart Flow: remove item from unified cart
    async fn cart_remove_item(
        &self,
        ctx: &Context<'_>,
        input: CartRemoveItemInput,
    ) -> Result<CartPayload> {
        let db = ctx.data::<GqlContext>()?.db().map_err(|e| Error::new(e.to_string()))?;
        let user_id = resolve_user_id(ctx, input.buyer_id.as_ref())?;

        let variant_id = Uuid::parse_str(&input.product_variant_id.0)
            .map_err(|_| Error::new("invalid variant ID"))?;

        let cart_svc = CartService::new(db);
        if let Err(e) = cart_svc.remove_item(user_id, variant_id).await {
            return Ok(CartPayload {
                cart: None,
                errors: vec![e.to_string()],
            });
        }

        // Return updated cart
        if let Ok(Some((cart, items))) = cart_svc.get_cart_with_items(user_id).await {
            let mut total = Decimal::ZERO;
            let currency = items.first().map(|i| i.currency.clone()).unwrap_or_else(|| "USD".into());

            let gql_items = items
                .into_iter()
                .map(|it| {
                    total += it.price_amount * Decimal::from(it.quantity);
                    GqlCartItem {
                        id: ID(it.id.to_string()),
                        product_variant_id: ID(it.product_variant_id.to_string()),
                        vendor_id: ID(it.vendor_id.to_string()),
                        quantity: it.quantity,
                        price_amount: it.price_amount,
                        currency: it.currency,
                    }
                })
                .collect();

            Ok(CartPayload {
                cart: Some(GqlCart {
                    id: ID(cart.id.to_string()),
                    user_id: ID(cart.user_id.to_string()),
                    items: gql_items,
                    total_amount: total,
                    currency,
                }),
                errors: vec![],
            })
        } else {
            Ok(CartPayload {
                cart: None,
                errors: vec![],
            })
        }
    }

    /// Buyer Checkout Flow: atomic checkout with multi-vendor order splitting
    async fn cart_checkout(
        &self,
        ctx: &Context<'_>,
        buyer_id: Option<ID>,
    ) -> Result<CheckoutPayload> {
        let db = ctx.data::<GqlContext>()?.db().map_err(|e| Error::new(e.to_string()))?;
        let user_id = resolve_user_id(ctx, buyer_id.as_ref())?;

        let order_svc = MarketplaceOrderService::new(db);
        match order_svc.checkout_cart_tx(user_id).await {
            Ok(res) => {
                let mut gql_subs = Vec::new();
                for (sub, items) in res.sub_orders {
                    let gql_items = items
                        .into_iter()
                        .map(|it| GqlOrderItem {
                            id: ID(it.id.to_string()),
                            sub_order_id: ID(it.sub_order_id.to_string()),
                            product_variant_id: ID(it.product_variant_id.to_string()),
                            product_title: it.product_title,
                            variant_name: it.variant_name,
                            sku: it.sku,
                            quantity: it.quantity,
                            unit_price_amount: it.unit_price_amount,
                            total_price_amount: it.total_price_amount,
                            currency: it.currency,
                        })
                        .collect();

                    let st = FulfillmentStatus::from_str(&sub.fulfillment_status).unwrap_or(FulfillmentStatus::Pending);

                    gql_subs.push(GqlSubOrder {
                        id: ID(sub.id.to_string()),
                        parent_order_id: ID(sub.parent_order_id.to_string()),
                        vendor_id: ID(sub.vendor_id.to_string()),
                        subtotal_amount: sub.subtotal_amount,
                        commission_amount: sub.commission_amount,
                        vendor_payout_amount: sub.vendor_payout_amount,
                        fulfillment_status: st,
                        tracking_number: sub.tracking_number,
                        items: gql_items,
                        created_at: sub.created_at.to_string(),
                    });
                }

                Ok(CheckoutPayload {
                    parent_order_id: Some(ID(res.parent_order.id.to_string())),
                    total_gross_amount: Some(res.parent_order.total_gross_amount),
                    sub_orders: gql_subs,
                    errors: vec![],
                })
            }
            Err(e) => Ok(CheckoutPayload {
                parent_order_id: None,
                total_gross_amount: None,
                sub_orders: vec![],
                errors: vec![e.to_string()],
            }),
        }
    }
}
