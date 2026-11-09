//! Marketplace Domain Models and Business Invariants
//!
//! True Multi-Vendor Marketplace with Server-Driven UI (SDUI) CMS Engine,
//! unified cart, and automatic order-splitting logic.

use std::collections::HashMap;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{DomainError, Result};

// ============================================================================
// 1. Vendor Domain Models
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VendorDomain {
    pub id: Uuid,
    pub user_id: Uuid,
    pub slug: String,
    pub name: String,
    pub bio: Option<String>,
    pub logo_url: Option<String>,
    pub banner_url: Option<String>,
    pub commission_rate: Decimal,
    pub is_verified: bool,
    pub is_active: bool,
    pub payout_account_info: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl VendorDomain {
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(DomainError::Validation {
                field: "name".into(),
                message: "vendor name cannot be empty".into(),
            });
        }
        if self.slug.trim().is_empty() {
            return Err(DomainError::Validation {
                field: "slug".into(),
                message: "vendor slug cannot be empty".into(),
            });
        }
        if self.commission_rate < Decimal::ZERO || self.commission_rate > Decimal::ONE {
            return Err(DomainError::Validation {
                field: "commission_rate".into(),
                message: "commission rate must be between 0.0 and 1.0".into(),
            });
        }
        Ok(())
    }

    pub fn ensure_owner(&self, requester_user_id: &Uuid) -> Result<()> {
        if self.user_id != *requester_user_id {
            return Err(DomainError::Validation {
                field: "user_id".into(),
                message: "unauthorized: requester is not the owner of this vendor account".into(),
            });
        }
        Ok(())
    }
}

// ============================================================================
// 2. Server-Driven UI (SDUI) Storefront CMS Models & Blocks
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HeroSlide {
    pub image_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DealProduct {
    pub product_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deal_price: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discount_badge: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GridPagination {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum StorefrontBlock {
    #[serde(alias = "HERO_SLIDER", alias = "HeroSlider")]
    HeroCarousel {
        #[serde(default)]
        slides: Vec<HeroSlide>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subtitle: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none", alias = "mediaUrl")]
        media_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none", alias = "ctaText")]
        cta_text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none", alias = "targetSlug")]
        target_slug: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        aspect_ratio: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auto_scroll_interval: Option<u32>,
    },
    #[serde(alias = "HOT_DEALS", alias = "HotDeals")]
    HotDeals {
        #[serde(default)]
        deal_products: Vec<DealProduct>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        end_timestamp: Option<DateTime<Utc>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        discount_badge: Option<String>,
    },
    #[serde(alias = "FEATURED_PRODUCTS", alias = "FeaturedProducts")]
    FeaturedProducts {
        #[serde(default)]
        product_ids: Vec<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subtitle: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_style: Option<String>,
    },
    #[serde(alias = "PRODUCT_GRID", alias = "ProductGrid")]
    ProductGrid {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        category_filter: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sort_by: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pagination: Option<GridPagination>,
    },
    #[serde(alias = "ANNOUNCEMENT_TICKER", alias = "AnnouncementTicker", alias = "MARQUEE")]
    Marquee {
        #[serde(default)]
        items: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        speed: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        direction: Option<String>,
    },
    #[serde(alias = "CATEGORY_GRID", alias = "CategoryGrid", alias = "CATEGORY_CHIPS")]
    CategoryChips {
        #[serde(default)]
        selected_categories: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subtitle: Option<String>,
    },
    #[serde(alias = "PROMO_BANNER", alias = "PromoBanner", alias = "PROMOTIONAL_BANNER")]
    PromotionalBanner {
        #[serde(default, alias = "mediaUrl")]
        image_url: String,
        #[serde(default, skip_serializing_if = "Option::is_none", alias = "targetSlug")]
        link_action: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none", alias = "title")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subtitle: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none", alias = "ctaText")]
        cta_text: Option<String>,
    },
    #[serde(alias = "VIDEO_REEL", alias = "VideoReel")]
    VideoReel {
        #[serde(default, alias = "videoUrl")]
        video_url: String,
        #[serde(default, skip_serializing_if = "Option::is_none", alias = "mediaUrl")]
        cover_image: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subtitle: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none", alias = "targetSlug")]
        action_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none", alias = "ctaText")]
        cta_text: Option<String>,
    },
    #[serde(alias = "BRAND_STORY", alias = "BrandStory")]
    BrandStory {
        #[serde(default)]
        title: String,
        #[serde(default, alias = "subtitle")]
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none", alias = "mediaUrl")]
        media_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none", alias = "targetSlug")]
        action_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none", alias = "ctaText")]
        cta_text: Option<String>,
    },
    #[serde(other)]
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StoreLayoutDomain {
    pub id: Uuid,
    pub vendor_id: Uuid,
    pub blocks: Vec<StorefrontBlock>,
    pub theme_tokens: serde_json::Value,
    pub is_published: bool,
    pub version: i32,
    pub updated_at: DateTime<Utc>,
}

/// Strictly validate layout blocks before persistence
pub fn validate_layout_blocks(blocks: &[StorefrontBlock]) -> Result<()> {
    if blocks.is_empty() {
        return Err(DomainError::Validation {
            field: "blocks".into(),
            message: "layout must contain at least one storefront block".into(),
        });
    }

    for (index, block) in blocks.iter().enumerate() {
        match block {
            StorefrontBlock::HeroCarousel {
                slides,
                media_url,
                auto_scroll_interval,
                ..
            } => {
                if slides.is_empty() && media_url.as_ref().map_or(true, |m| m.trim().is_empty()) {
                    return Err(DomainError::Validation {
                        field: format!("blocks[{index}].slides"),
                        message: "HeroCarousel must have at least one slide or a media_url".into(),
                    });
                }
                for (s_idx, slide) in slides.iter().enumerate() {
                    if slide.image_url.trim().is_empty() {
                        return Err(DomainError::Validation {
                            field: format!("blocks[{index}].slides[{s_idx}].image_url"),
                            message: "slide image_url cannot be empty".into(),
                        });
                    }
                }
                if let Some(interval) = auto_scroll_interval {
                    if *interval == 0 {
                        return Err(DomainError::Validation {
                            field: format!("blocks[{index}].auto_scroll_interval"),
                            message: "auto_scroll_interval must be greater than 0".into(),
                        });
                    }
                }
            }
            StorefrontBlock::HotDeals { deal_products, .. } => {
                if deal_products.is_empty() {
                    return Err(DomainError::Validation {
                        field: format!("blocks[{index}].deal_products"),
                        message: "HotDeals must contain at least one deal product".into(),
                    });
                }
                for (d_idx, deal) in deal_products.iter().enumerate() {
                    if let Some(price) = deal.deal_price {
                        if price < Decimal::ZERO {
                            return Err(DomainError::Validation {
                                field: format!("blocks[{index}].deal_products[{d_idx}].deal_price"),
                                message: "deal_price cannot be negative".into(),
                            });
                        }
                    }
                }
            }
            StorefrontBlock::FeaturedProducts { .. } => {
                // FeaturedProducts can be auto-resolved from catalog if product_ids empty
            }
            StorefrontBlock::ProductGrid { .. } => {
                // ProductGrid is valid with default parameters
            }
            StorefrontBlock::Marquee {
                items, title, direction, ..
            } => {
                if items.is_empty() && title.as_ref().map_or(true, |t| t.trim().is_empty()) {
                    return Err(DomainError::Validation {
                        field: format!("blocks[{index}].items"),
                        message: "Marquee must contain at least one text item or a title".into(),
                    });
                }
                for (m_idx, item) in items.iter().enumerate() {
                    if item.trim().is_empty() {
                        return Err(DomainError::Validation {
                            field: format!("blocks[{index}].items[{m_idx}]"),
                            message: "marquee item cannot be empty string".into(),
                        });
                    }
                }
                if let Some(dir) = direction {
                    if dir != "left" && dir != "right" {
                        return Err(DomainError::Validation {
                            field: format!("blocks[{index}].direction"),
                            message: "Marquee direction must be 'left' or 'right'".into(),
                        });
                    }
                }
            }
            StorefrontBlock::CategoryChips { .. } => {
                // CategoryChips is valid
            }
            StorefrontBlock::PromotionalBanner { image_url, text, .. } => {
                if image_url.trim().is_empty() && text.as_ref().map_or(true, |t| t.trim().is_empty()) {
                    return Err(DomainError::Validation {
                        field: format!("blocks[{index}].image_url"),
                        message: "PromotionalBanner requires image_url or title text".into(),
                    });
                }
            }
            StorefrontBlock::VideoReel { .. } => {
                // VideoReel is valid
            }
            StorefrontBlock::BrandStory { .. } => {
                // BrandStory is valid
            }
            StorefrontBlock::Custom => {
                // Extensible custom block is valid
            }
        }
    }

    Ok(())
}

// ============================================================================
// 3. Marketplace Catalog Models
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MarketplaceProductDomain {
    pub id: Uuid,
    pub vendor_id: Uuid,
    pub title: String,
    pub slug: String,
    pub description: Option<String>,
    pub category_id: Option<Uuid>,
    pub is_published: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub rating_avg: Decimal,
    pub sales_count: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MarketplaceVariantDomain {
    pub id: Uuid,
    pub product_id: Uuid,
    pub sku: String,
    pub name: String,
    pub price_amount: Decimal,
    pub currency: String,
    pub compare_at_price: Option<Decimal>,
    pub stock_quantity: i32,
    pub attributes: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MarketplaceMediaDomain {
    pub id: Uuid,
    pub product_id: Uuid,
    pub media_type: String, // "IMAGE" or "VIDEO"
    pub url: String,
    pub alt_text: Option<String>,
    pub sort_order: i32,
}

// ============================================================================
// 4. Multi-Vendor Cart & Automatic Order Splitting
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MarketplaceCartDomain {
    pub id: Uuid,
    pub user_id: Uuid,
    pub items: Vec<MarketplaceCartItemDomain>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MarketplaceCartItemDomain {
    pub id: Uuid,
    pub cart_id: Uuid,
    pub product_variant_id: Uuid,
    pub vendor_id: Uuid,
    pub quantity: i32,
    pub price_amount: Decimal,
    pub currency: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CartItemInput {
    pub product_variant_id: Uuid,
    pub vendor_id: Uuid,
    pub quantity: i32,
    pub unit_price_amount: Decimal,
    pub currency: String,
    pub product_title: String,
    pub variant_name: String,
    pub sku: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VendorSubOrderCalculation {
    pub vendor_id: Uuid,
    pub subtotal_amount: Decimal,
    pub commission_amount: Decimal,
    pub vendor_payout_amount: Decimal,
    pub items: Vec<CartItemInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OrderSplitCalculation {
    pub buyer_id: Uuid,
    pub total_gross_amount: Decimal,
    pub sub_orders: Vec<VendorSubOrderCalculation>,
}

/// Computes multi-vendor cart totals, calculates commission breakdowns,
/// and partitions order items into vendor-specific sub-orders.
pub fn calculate_order_splitting(
    buyer_id: Uuid,
    items: Vec<CartItemInput>,
    vendor_commissions: &HashMap<Uuid, Decimal>,
) -> Result<OrderSplitCalculation> {
    if items.is_empty() {
        return Err(DomainError::Validation {
            field: "items".into(),
            message: "cart is empty; cannot checkout".into(),
        });
    }

    // Group items by vendor_id
    let mut vendor_items_map: HashMap<Uuid, Vec<CartItemInput>> = HashMap::new();
    let first_currency = items[0].currency.clone();

    for item in items {
        if item.quantity <= 0 {
            return Err(DomainError::Validation {
                field: "quantity".into(),
                message: "item quantity must be positive".into(),
            });
        }
        if item.unit_price_amount < Decimal::ZERO {
            return Err(DomainError::Validation {
                field: "unit_price_amount".into(),
                message: "item unit price cannot be negative".into(),
            });
        }
        if item.currency != first_currency {
            return Err(DomainError::Validation {
                field: "currency".into(),
                message: "all cart items must share the same currency for unified checkout".into(),
            });
        }
        vendor_items_map
            .entry(item.vendor_id)
            .or_default()
            .push(item);
    }

    let mut sub_orders = Vec::new();
    let mut total_gross_amount = Decimal::ZERO;

    for (vendor_id, v_items) in vendor_items_map {
        let mut subtotal = Decimal::ZERO;
        for it in &v_items {
            subtotal += it.unit_price_amount * Decimal::from(it.quantity);
        }

        // Default platform commission: 10% if vendor has no custom rate
        let commission_rate = vendor_commissions
            .get(&vendor_id)
            .copied()
            .unwrap_or_else(|| Decimal::new(10, 2));

        let commission_amount = (subtotal * commission_rate).round_dp(2);
        let vendor_payout_amount = subtotal - commission_amount;

        total_gross_amount += subtotal;

        sub_orders.push(VendorSubOrderCalculation {
            vendor_id,
            subtotal_amount: subtotal,
            commission_amount,
            vendor_payout_amount,
            items: v_items,
        });
    }

    Ok(OrderSplitCalculation {
        buyer_id,
        total_gross_amount,
        sub_orders,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_validate_layout_blocks_valid() {
        let blocks = vec![
            StorefrontBlock::HeroCarousel {
                slides: vec![HeroSlide {
                    image_url: "https://cdn.example.com/slide1.jpg".into(),
                    title: Some("Summer Sale".into()),
                    subtitle: None,
                    action_url: None,
                }],
                title: None,
                subtitle: None,
                media_url: None,
                cta_text: None,
                target_slug: None,
                aspect_ratio: Some("16:9".into()),
                auto_scroll_interval: Some(5000),
            },
            StorefrontBlock::Marquee {
                items: vec!["FREE SHIPPING OVER $50".into()],
                title: None,
                speed: Some(30),
                direction: Some("left".into()),
            },
            StorefrontBlock::PromotionalBanner {
                image_url: "https://cdn.example.com/banner.png".into(),
                link_action: Some("/sale".into()),
                text: Some("50% OFF".into()),
                subtitle: None,
                cta_text: None,
            },
        ];
        assert!(validate_layout_blocks(&blocks).is_ok());
    }

    #[test]
    fn test_validate_layout_blocks_empty_carousel_fails() {
        let blocks = vec![StorefrontBlock::HeroCarousel {
            slides: vec![],
            title: None,
            subtitle: None,
            media_url: None,
            cta_text: None,
            target_slug: None,
            aspect_ratio: None,
            auto_scroll_interval: None,
        }];
        assert!(validate_layout_blocks(&blocks).is_err());
    }

    #[test]
    fn test_order_splitting_multi_vendor() {
        let buyer_id = Uuid::new_v4();
        let vendor_a = Uuid::new_v4();
        let vendor_b = Uuid::new_v4();

        let mut commissions = HashMap::new();
        commissions.insert(vendor_a, dec!(0.10)); // 10%
        commissions.insert(vendor_b, dec!(0.15)); // 15%

        let items = vec![
            CartItemInput {
                product_variant_id: Uuid::new_v4(),
                vendor_id: vendor_a,
                quantity: 2,
                unit_price_amount: dec!(50.00),
                currency: "USD".into(),
                product_title: "Product A1".into(),
                variant_name: "Default".into(),
                sku: "SKU-A1".into(),
            },
            CartItemInput {
                product_variant_id: Uuid::new_v4(),
                vendor_id: vendor_b,
                quantity: 1,
                unit_price_amount: dec!(200.00),
                currency: "USD".into(),
                product_title: "Product B1".into(),
                variant_name: "Default".into(),
                sku: "SKU-B1".into(),
            },
        ];

        let split = calculate_order_splitting(buyer_id, items, &commissions).unwrap();

        assert_eq!(split.total_gross_amount, dec!(300.00));
        assert_eq!(split.sub_orders.len(), 2);

        let sub_a = split.sub_orders.iter().find(|s| s.vendor_id == vendor_a).unwrap();
        assert_eq!(sub_a.subtotal_amount, dec!(100.00));
        assert_eq!(sub_a.commission_amount, dec!(10.00));
        assert_eq!(sub_a.vendor_payout_amount, dec!(90.00));

        let sub_b = split.sub_orders.iter().find(|s| s.vendor_id == vendor_b).unwrap();
        assert_eq!(sub_b.subtotal_amount, dec!(200.00));
        assert_eq!(sub_b.commission_amount, dec!(30.00));
        assert_eq!(sub_b.vendor_payout_amount, dec!(170.00));
    }
}
