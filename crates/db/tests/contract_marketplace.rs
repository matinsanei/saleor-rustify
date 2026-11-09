//! End-to-End Contract Tests for Unified Marketplace & SDUI CMS Engine
//!
//! Verifies:
//! 1. Single unified PostgreSQL database schema initialization (zero multi-tenancy bloat).
//! 2. Vendor registration, slug uniqueness, strict vendor RBAC.
//! 3. Native Server-Driven UI (SDUI) CMS block validation, versioning, and enriched JSON rendering.
//! 4. Unified multi-vendor cart with multi-merchant items.
//! 5. Automatic order-splitting transaction with platform commission & vendor payout breakdown.
//! 6. Vendor fulfillment updates and isolated order management.

use rust_decimal_macros::dec;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use saleor_rustify_core::marketplace::{
    HeroSlide, StorefrontBlock,
};
use saleor_rustify_db::{
    database_url,
    marketplace::{
        apply_marketplace_migrations,
        catalog_service::{CatalogService, CreateProductInput, UpdateProductInput},
        cart_service::CartService,
        marketplace_order_service::MarketplaceOrderService,
        storefront_cms_service::StorefrontCmsService,
        vendor_service::VendorService,
    },
};

async fn get_test_db() -> DatabaseConnection {
    let url = database_url();
    let db = saleor_rustify_db::connect(&url)
        .await
        .expect("PostgreSQL test database connection failed");

    // Apply marketplace tables and indexes idempotently
    apply_marketplace_migrations(&db)
        .await
        .expect("Failed to apply marketplace migrations");

    db
}

#[tokio::test]
async fn test_marketplace_full_e2e_lifecycle() {
    let db = get_test_db().await;

    // ------------------------------------------------------------------------
    // Step 1: Vendor Registration & Profile Updates (Strict RBAC)
    // ------------------------------------------------------------------------
    let vendor_svc = VendorService::new(&db);

    let user_a_id = Uuid::new_v4();
    let slug_a = format!("creator-{}", &Uuid::new_v4().to_string()[..6]);

    let vendor_a = vendor_svc
        .register_vendor(
            user_a_id,
            slug_a.clone(),
            "Studio Tokyo".into(),
            Some("Handcrafted ceramics from Tokyo".into()),
            Some("https://cdn.example.com/tokyo-logo.png".into()),
            Some("https://cdn.example.com/tokyo-banner.jpg".into()),
            Some(dec!(0.10)), // 10% commission
            Some(serde_json::json!({"bank_account": "tokyo_jp_12345"})),
        )
        .await
        .expect("Failed to register vendor A");

    assert_eq!(vendor_a.name, "Studio Tokyo");
    assert_eq!(vendor_a.commission_rate, dec!(0.10));
    assert!(vendor_a.is_active);

    // Slug duplicate rejection
    let duplicate_slug_res = vendor_svc
        .register_vendor(
            Uuid::new_v4(),
            slug_a.clone(),
            "Impostor".into(),
            None,
            None,
            None,
            None,
            None,
        )
        .await;
    assert!(duplicate_slug_res.is_err(), "Duplicate slug must be rejected");

    // Vendor B registration
    let user_b_id = Uuid::new_v4();
    let slug_b = format!("artisan-{}", &Uuid::new_v4().to_string()[..6]);
    let vendor_b = vendor_svc
        .register_vendor(
            user_b_id,
            slug_b.clone(),
            "Kyoto Leather".into(),
            Some("Bespoke leather goods".into()),
            None,
            None,
            Some(dec!(0.15)), // 15% commission
            None,
        )
        .await
        .expect("Failed to register vendor B");

    // Ownership check: User B cannot modify Vendor A's profile
    let unauthorized_update = vendor_svc
        .update_profile(
            vendor_a.id,
            user_b_id, // Wrong user
            Some("Hacked Tokyo".into()),
            None,
            None,
            None,
            None,
        )
        .await;
    assert!(unauthorized_update.is_err(), "Non-owner update must be rejected");

    // Valid update by User A
    let updated_vendor_a = vendor_svc
        .update_profile(
            vendor_a.id,
            user_a_id,
            Some("Studio Tokyo Premium".into()),
            Some("Updated bio".into()),
            None,
            None,
            None,
        )
        .await
        .expect("Owner profile update should succeed");
    assert_eq!(updated_vendor_a.name, "Studio Tokyo Premium");

    // ------------------------------------------------------------------------
    // Step 2: Native SDUI Storefront CMS (Server-Driven UI)
    // ------------------------------------------------------------------------
    let cms_svc = StorefrontCmsService::new(&db);

    let sample_blocks = vec![
        StorefrontBlock::HeroCarousel {
            slides: vec![HeroSlide {
                image_url: "https://cdn.example.com/hero1.jpg".into(),
                title: Some("Spring Ceramic Collection".into()),
                subtitle: Some("Limited batches".into()),
                action_url: Some("/shop/ceramics".into()),
            }],
            title: None,
            subtitle: None,
            media_url: None,
            cta_text: None,
            target_slug: None,
            aspect_ratio: Some("16:9".into()),
            auto_scroll_interval: Some(4000),
        },
        StorefrontBlock::Marquee {
            items: vec!["WORLDWIDE EXPRESS SHIPPING".into(), "HANDMADE IN JAPAN".into()],
            title: None,
            speed: Some(40),
            direction: Some("left".into()),
        },
        StorefrontBlock::CategoryChips {
            selected_categories: vec!["Vases".into(), "Bowls".into(), "Teaware".into()],
            title: None,
            subtitle: None,
        },
        StorefrontBlock::PromotionalBanner {
            image_url: "https://cdn.example.com/promo.png".into(),
            link_action: Some("/discount".into()),
            text: Some("Use code SPRING10 for 10% off".into()),
            subtitle: None,
            cta_text: None,
        },
    ];

    let blocks_json = serde_json::to_value(&sample_blocks).unwrap();
    let theme_tokens = serde_json::json!({
        "primaryColor": "#1A1A1A",
        "accentColor": "#D97706",
        "borderRadius": "8px"
    });

    // Save layout for Vendor A (Version 1)
    let layout_v1 = cms_svc
        .update_store_layout(
            vendor_a.id,
            user_a_id,
            blocks_json.clone(),
            Some(theme_tokens.clone()),
            Some(true),
        )
        .await
        .expect("Store layout v1 write should succeed");

    assert_eq!(layout_v1.version, 1);
    assert_eq!(layout_v1.vendor_id, vendor_a.id);

    // Update layout for Vendor A (Version 2 increment)
    let layout_v2 = cms_svc
        .update_store_layout(
            vendor_a.id,
            user_a_id,
            blocks_json,
            Some(theme_tokens),
            Some(true),
        )
        .await
        .expect("Store layout v2 update should succeed");

    assert_eq!(layout_v2.version, 2);

    // Retrieve enriched layout for mobile SDUI rendering
    let enriched = cms_svc
        .get_enriched_layout_by_slug(&slug_a)
        .await
        .expect("Enriched layout query should succeed")
        .expect("Vendor A layout should exist");

    assert_eq!(enriched.vendor.id, vendor_a.id);
    assert_eq!(enriched.layout.version, 2);

    // ------------------------------------------------------------------------
    // Step 3: Marketplace Catalog Management (Vendors A & B)
    // ------------------------------------------------------------------------
    let catalog_svc = CatalogService::new(&db);

    // Vendor A creates Product 1
    let sku_a1 = format!("V-A1-{}", &Uuid::new_v4().to_string()[..6]);
    let prod_a1 = catalog_svc
        .create_product(CreateProductInput {
            vendor_id: vendor_a.id,
            title: "Artisan Ceramic Bowl".into(),
            slug: None,
            description: Some("Handcrafted glazed ceramic bowl".into()),
            category_id: None,
            is_published: Some(true),
            sku: sku_a1.clone(),
            variant_name: Some("White Glaze".into()),
            price_amount: dec!(50.00),
            currency: Some("USD".into()),
            compare_at_price: Some(dec!(65.00)),
            stock_quantity: Some(20),
            attributes: Some(serde_json::json!({"color": "white", "finish": "matte"})),
            media_urls: Some(vec!["https://cdn.example.com/bowl.jpg".into()]),
        })
        .await
        .expect("Product A1 creation should succeed");

    // Vendor B creates Product 2
    let sku_b1 = format!("V-B1-{}", &Uuid::new_v4().to_string()[..6]);
    let prod_b1 = catalog_svc
        .create_product(CreateProductInput {
            vendor_id: vendor_b.id,
            title: "Handmade Leather Wallet".into(),
            slug: None,
            description: Some("Full grain vegetable tanned leather wallet".into()),
            category_id: None,
            is_published: Some(true),
            sku: sku_b1.clone(),
            variant_name: Some("Cognac Brown".into()),
            price_amount: dec!(120.00),
            currency: Some("USD".into()),
            compare_at_price: Some(dec!(150.00)),
            stock_quantity: Some(15),
            attributes: Some(serde_json::json!({"color": "cognac"})),
            media_urls: Some(vec!["https://cdn.example.com/wallet.jpg".into()]),
        })
        .await
        .expect("Product B1 creation should succeed");

    // Strict RBAC: Vendor A cannot update Vendor B's product
    let unauthorized_prod_update = catalog_svc
        .update_product(
            prod_b1.id,
            vendor_a.id, // Vendor A attempting to edit Vendor B's product
            UpdateProductInput {
                title: Some("Hacked Wallet".into()),
                description: None,
                category_id: None,
                is_published: None,
                price_amount: None,
                compare_at_price: None,
                stock_quantity: None,
            },
        )
        .await;
    assert!(unauthorized_prod_update.is_err(), "Cross-vendor product edit must fail");

    // Search marketplace products
    let (search_results, total_count) = catalog_svc
        .search_marketplace_products(Some("Ceramic".into()), None, None, Some(1), Some(10))
        .await
        .expect("Search should succeed");
    assert!(total_count >= 1);
    assert!(search_results.iter().any(|p| p.id == prod_a1.id));

    // ------------------------------------------------------------------------
    // Step 4: Multi-Vendor Cart & Automatic Order Splitting
    // ------------------------------------------------------------------------
    let cart_svc = CartService::new(&db);
    let order_svc = MarketplaceOrderService::new(&db);
    let buyer_id = Uuid::new_v4();

    // Fetch variant IDs
    let variants_a = catalog_svc.get_variants_for_product(prod_a1.id).await.unwrap();
    let variant_a_id = variants_a[0].id;

    let variants_b = catalog_svc.get_variants_for_product(prod_b1.id).await.unwrap();
    let variant_b_id = variants_b[0].id;

    // Buyer adds 2x Product A (Vendor A, $50 each = $100)
    let item_a = cart_svc
        .add_item(buyer_id, variant_a_id, 2)
        .await
        .expect("Add item A to cart should succeed");
    assert_eq!(item_a.quantity, 2);
    assert_eq!(item_a.vendor_id, vendor_a.id);

    // Buyer adds 1x Product B (Vendor B, $120 each = $120) to the SAME unified cart
    let item_b = cart_svc
        .add_item(buyer_id, variant_b_id, 1)
        .await
        .expect("Add item B to cart should succeed");
    assert_eq!(item_b.quantity, 1);
    assert_eq!(item_b.vendor_id, vendor_b.id);

    // Verify cart contains both items from different vendors
    let (_cart, cart_items) = cart_svc
        .get_cart_with_items(buyer_id)
        .await
        .expect("Cart query should succeed")
        .expect("Cart should exist");
    assert_eq!(cart_items.len(), 2);

    // ------------------------------------------------------------------------
    // Step 5: Atomic Multi-Vendor Checkout & Order Splitting Transaction
    // ------------------------------------------------------------------------
    let checkout_res = order_svc
        .checkout_cart_tx(buyer_id)
        .await
        .expect("Atomic checkout transaction should succeed");

    // Parent Order Assertions:
    // Total Gross = (2 * $50) + (1 * $120) = $100 + $120 = $220.00
    assert_eq!(checkout_res.parent_order.buyer_id, buyer_id);
    assert_eq!(checkout_res.parent_order.total_gross_amount, dec!(220.00));
    assert_eq!(checkout_res.parent_order.payment_status, "PAID");

    // Sub-Orders Assertions (Split into 2 independent child orders):
    assert_eq!(checkout_res.sub_orders.len(), 2);

    // Vendor A Sub-Order (10% commission):
    // Subtotal: $100.00
    // Commission: $10.00
    // Vendor Payout: $90.00
    let (sub_a, items_a) = checkout_res
        .sub_orders
        .iter()
        .find(|(s, _)| s.vendor_id == vendor_a.id)
        .expect("Vendor A sub-order must exist");

    assert_eq!(sub_a.parent_order_id, checkout_res.parent_order.id);
    assert_eq!(sub_a.subtotal_amount, dec!(100.00));
    assert_eq!(sub_a.commission_amount, dec!(10.00));
    assert_eq!(sub_a.vendor_payout_amount, dec!(90.00));
    assert_eq!(sub_a.fulfillment_status, "PENDING");
    assert_eq!(items_a.len(), 1);
    assert_eq!(items_a[0].quantity, 2);

    // Vendor B Sub-Order (15% commission):
    // Subtotal: $120.00
    // Commission: $18.00 (15% of 120)
    // Vendor Payout: $102.00
    let (sub_b, items_b) = checkout_res
        .sub_orders
        .iter()
        .find(|(s, _)| s.vendor_id == vendor_b.id)
        .expect("Vendor B sub-order must exist");

    assert_eq!(sub_b.parent_order_id, checkout_res.parent_order.id);
    assert_eq!(sub_b.subtotal_amount, dec!(120.00));
    assert_eq!(sub_b.commission_amount, dec!(18.00));
    assert_eq!(sub_b.vendor_payout_amount, dec!(102.00));
    assert_eq!(sub_b.fulfillment_status, "PENDING");
    assert_eq!(items_b.len(), 1);
    assert_eq!(items_b[0].quantity, 1);

    // Verify stock quantities were decremented atomically
    let updated_v_a = catalog_svc.get_variants_for_product(prod_a1.id).await.unwrap();
    assert_eq!(updated_v_a[0].stock_quantity, 18); // was 20, purchased 2

    let updated_v_b = catalog_svc.get_variants_for_product(prod_b1.id).await.unwrap();
    assert_eq!(updated_v_b[0].stock_quantity, 14); // was 15, purchased 1

    // Verify cart was cleared after successful checkout
    let cart_after = cart_svc.get_cart_with_items(buyer_id).await.unwrap().unwrap();
    assert_eq!(cart_after.1.len(), 0, "Cart items must be cleared after checkout");

    // ------------------------------------------------------------------------
    // Step 6: Isolated Vendor Fulfillment Flow
    // ------------------------------------------------------------------------
    // Vendor A updates fulfillment to SHIPPED
    let shipped_sub_a = order_svc
        .vendor_update_fulfillment_tx(
            vendor_a.id,
            sub_a.id,
            "SHIPPED".into(),
            Some("JP-POST-998877".into()),
        )
        .await
        .expect("Vendor A fulfillment update should succeed");

    assert_eq!(shipped_sub_a.fulfillment_status, "SHIPPED");
    assert_eq!(shipped_sub_a.tracking_number, Some("JP-POST-998877".into()));

    // Strict RBAC: Vendor B CANNOT update Vendor A's sub-order
    let unauthorized_fulfillment = order_svc
        .vendor_update_fulfillment_tx(
            vendor_b.id, // Wrong vendor
            sub_a.id,
            "DELIVERED".into(),
            None,
        )
        .await;
    assert!(unauthorized_fulfillment.is_err(), "Cross-vendor fulfillment mutation must be rejected");

    // Vendor query sub-orders with status filter
    let (vendor_a_orders, total_a_orders) = order_svc
        .list_vendor_sub_orders(vendor_a.id, Some("SHIPPED".into()), Some(1), Some(10))
        .await
        .expect("List vendor sub orders should succeed");

    assert_eq!(total_a_orders, 1);
    assert_eq!(vendor_a_orders[0].id, sub_a.id);
}
