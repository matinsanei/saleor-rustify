//! Contract tests for Marketplace & SDUI CMS GraphQL Schema and Resolvers

use saleor_rustify_db::{
    database_url,
    marketplace::apply_marketplace_migrations,
};
use saleor_rustify_graphql::{build_schema, build_schema_with_bearer};
use uuid::Uuid;

#[tokio::test]
async fn test_marketplace_sdl_registration() {
    let schema = build_schema(None);
    let sdl = schema.sdl();

    // Verify all required Public Storefront Queries are present in SDL
    assert!(sdl.contains("vendorBySlug"), "SDL must contain vendorBySlug query");
    assert!(sdl.contains("storefrontLayout"), "SDL must contain storefrontLayout query");
    assert!(sdl.contains("marketplaceSearch"), "SDL must contain marketplaceSearch query");
    assert!(sdl.contains("vendorSubOrders"), "SDL must contain vendorSubOrders query");

    // Verify Vendor Studio Mutations are present in SDL
    assert!(sdl.contains("vendorRegister"), "SDL must contain vendorRegister mutation");
    assert!(sdl.contains("vendorUpdateStoreLayout"), "SDL must contain vendorUpdateStoreLayout mutation");
    assert!(sdl.contains("productCreate"), "SDL must contain productCreate mutation");
    assert!(sdl.contains("productUpdate"), "SDL must contain productUpdate mutation");
    assert!(sdl.contains("productDelete"), "SDL must contain productDelete mutation");
    assert!(sdl.contains("vendorUpdateFulfillment"), "SDL must contain vendorUpdateFulfillment mutation");

    // Verify Buyer Marketplace Flow Mutations are present in SDL
    assert!(sdl.contains("cartAddItem"), "SDL must contain cartAddItem mutation");
    assert!(sdl.contains("cartRemoveItem"), "SDL must contain cartRemoveItem mutation");
    assert!(sdl.contains("cartCheckout"), "SDL must contain cartCheckout mutation");
}

#[tokio::test]
async fn test_marketplace_graphql_resolvers_flow() {
    let url = database_url();
    let db = match saleor_rustify_db::connect(&url).await {
        Ok(c) => c,
        Err(_) => return, // Skip test if database is not reachable
    };

    apply_marketplace_migrations(&db)
        .await
        .expect("Marketplace migration must succeed");

    let test_user_id = Uuid::new_v4();
    let schema = build_schema_with_bearer(Some(db), Some(test_user_id.to_string()));

    let vendor_slug = format!("gql-vendor-{}", &Uuid::new_v4().to_string()[..6]);

    // 1. Vendor Registration
    let register_mutation = format!(
        r#"
        mutation {{
            vendorRegister(input: {{
                slug: "{vendor_slug}",
                name: "GQL Artisan Shop",
                bio: "Handmade through GraphQL",
                commissionRate: "0.12"
            }}) {{
                vendor {{
                    id
                    slug
                    name
                    commissionRate
                }}
                errors
            }}
        }}
        "#
    );

    let reg_res = schema.execute(&register_mutation).await;
    assert!(reg_res.errors.is_empty(), "Register mutation errors: {:?}", reg_res.errors);
    let reg_data = reg_res.data.to_string();
    assert!(reg_data.contains("GQL Artisan Shop"));

    // 2. Public Storefront Query: vendorBySlug
    let vendor_query = format!(
        r#"
        query {{
            vendorBySlug(slug: "{vendor_slug}") {{
                id
                slug
                name
            }}
        }}
        "#
    );
    let v_res = schema.execute(&vendor_query).await;
    assert!(v_res.errors.is_empty(), "vendorBySlug errors: {:?}", v_res.errors);
    assert!(v_res.data.to_string().contains(&vendor_slug));

    // 3. Vendor Studio: vendorUpdateStoreLayout (SDUI)
    let update_layout_mutation = r##"
        mutation {
            vendorUpdateStoreLayout(input: {
                blocks: [
                    {
                        type: "HeroCarousel",
                        slides: [
                            {
                                image_url: "https://cdn.example.com/slide1.jpg",
                                title: "Welcome to GraphQL Shop"
                            }
                        ],
                        aspect_ratio: "16:9",
                        auto_scroll_interval: 3000
                    },
                    {
                        type: "Marquee",
                        items: ["24/7 SUPPORT", "INSTANT SHIPPING"],
                        speed: 50,
                        direction: "left"
                    }
                ],
                themeTokens: {
                    accent: "#FF0055"
                },
                isPublished: true
            }) {
                layout {
                    id
                    version
                    isPublished
                }
                errors
            }
        }
    "##;
    let layout_res = schema.execute(update_layout_mutation).await;
    assert!(layout_res.errors.is_empty(), "Layout mutation errors: {:?}", layout_res.errors);
    assert!(layout_res.data.to_string().contains("version"));

    // 4. Public Storefront Query: storefrontLayout
    let storefront_query = format!(
        r#"
        query {{
            storefrontLayout(vendorSlug: "{vendor_slug}") {{
                vendorSlug
                version
                isPublished
                blocks
            }}
        }}
        "#
    );
    let sf_res = schema.execute(&storefront_query).await;
    assert!(sf_res.errors.is_empty(), "storefrontLayout errors: {:?}", sf_res.errors);
    assert!(sf_res.data.to_string().contains(&vendor_slug));
    assert!(sf_res.data.to_string().contains("HeroCarousel"));

    // 5. Vendor Studio: productCreate
    let sku = format!("SKU-GQL-{}", &Uuid::new_v4().to_string()[..6]);
    let prod_create_mutation = format!(
        r#"
        mutation {{
            productCreate(input: {{
                title: "GraphQL Leather Belt",
                sku: "{sku}",
                priceAmount: "75.00",
                currency: "USD",
                stockQuantity: 50,
                description: "Full grain Italian leather"
            }}) {{
                product {{
                    id
                    title
                    variants {{
                        id
                        sku
                        priceAmount
                        stockQuantity
                    }}
                }}
                errors
            }}
        }}
        "#
    );
    let p_res = schema.execute(&prod_create_mutation).await;
    assert!(p_res.errors.is_empty(), "productCreate errors: {:?}", p_res.errors);
    assert!(p_res.data.to_string().contains("GraphQL Leather Belt"));

    // Extract product and variant ID
    let p_json: serde_json::Value = p_res.data.into_json().expect("Must convert GraphQL response into JSON");
    let variant_id = p_json["productCreate"]["product"]["variants"][0]["id"]
        .as_str()
        .expect("Variant ID must be present");

    // 6. Public Marketplace Search
    let search_query = r#"
        query {
            marketplaceSearch(query: "Leather", page: 1) {
                totalCount
                items {
                    id
                    title
                }
            }
        }
    "#;
    let s_res = schema.execute(search_query).await;
    assert!(s_res.errors.is_empty(), "marketplaceSearch errors: {:?}", s_res.errors);
    assert!(s_res.data.to_string().contains("GraphQL Leather Belt"));

    // 7. Buyer Flow: cartAddItem
    let add_item_mutation = format!(
        r#"
        mutation {{
            cartAddItem(input: {{
                productVariantId: "{variant_id}",
                quantity: 2
            }}) {{
                cart {{
                    id
                    totalAmount
                    items {{
                        quantity
                        priceAmount
                    }}
                }}
                errors
            }}
        }}
        "#
    );
    let cart_res = schema.execute(&add_item_mutation).await;
    assert!(cart_res.errors.is_empty(), "cartAddItem errors: {:?}", cart_res.errors);
    assert!(cart_res.data.to_string().contains("150.00")); // 2 * $75

    // 8. Buyer Flow: cartCheckout (Atomic Multi-Vendor Splitting)
    let checkout_mutation = r#"
        mutation {
            cartCheckout {
                parentOrderId
                totalGrossAmount
                subOrders {
                    id
                    vendorId
                    subtotalAmount
                    commissionAmount
                    vendorPayoutAmount
                    fulfillmentStatus
                    items {
                        productTitle
                        quantity
                    }
                }
                errors
            }
        }
    "#;
    let co_res = schema.execute(checkout_mutation).await;
    assert!(co_res.errors.is_empty(), "cartCheckout errors: {:?}", co_res.errors);
    let co_str = co_res.data.to_string();
    assert!(co_str.contains("150.00")); // Total gross
    assert!(co_str.contains("18.00"));  // 12% commission of $150
    assert!(co_str.contains("132.00")); // Vendor payout: 150 - 18
}
