//! Checkout GraphQL: create / get / addLines / complete + voucher/promotion.
//! Every resolver is a thin call into `db::checkout_store` / `db::promotions`
//! / `db::order_promotions` — the same functions gRPC uses.

use async_graphql::*;
use uuid::Uuid;

use crate::{common::*, context::GqlContext, gen};

#[derive(SimpleObject, Clone)]
pub struct GqlCheckoutThumbnail {
    pub url: Option<String>,
    pub alt: Option<String>,
}

#[derive(SimpleObject, Clone)]
pub struct GqlCheckoutProduct {
    pub name: String,
    pub thumbnail: Option<GqlCheckoutThumbnail>,
}

#[derive(SimpleObject, Clone)]
pub struct GqlCheckoutVariantPricing {
    pub price: crate::order::GqlTaxedMoney,
    #[graphql(name = "priceUndiscounted")]
    pub price_undiscounted: crate::order::GqlTaxedMoney,
}

#[derive(SimpleObject, Clone)]
pub struct GqlCheckoutVariant {
    pub id: ID,
    pub name: String,
    pub product: GqlCheckoutProduct,
    pub pricing: GqlCheckoutVariantPricing,
    #[graphql(name = "quantityAvailable")]
    pub quantity_available: i32,
}

#[derive(SimpleObject, Clone)]
pub struct GqlCheckoutLine {
    pub id: ID,
    pub variant_id: ID,
    pub quantity: i32,
    pub unit_price: Money,
    #[graphql(name = "totalPrice")]
    pub total_price: crate::order::GqlTaxedMoney,
    pub is_gift: bool,
    pub variant: Option<GqlCheckoutVariant>,
}

#[derive(SimpleObject, Clone)]
pub struct GqlCheckoutVoucher {
    pub id: ID,
    pub code: String,
    pub name: Option<String>,
    #[graphql(name = "type")]
    pub voucher_type: String,
    #[graphql(name = "discountValueType")]
    pub discount_value_type: String,
    #[graphql(name = "discountValue")]
    pub discount_value: String,
}

/// Checkout-scoped delivery union. Member names mirror Saleor's
/// `DeliveryMethod = Warehouse | ShippingMethod` so caller fragments
/// (`... on ShippingMethod`) validate; the collection-point arm reuses the
/// existing hand type (callers never select it here).
#[derive(Union, Clone)]
pub enum GqlCheckoutDelivery {
    ShippingMethod(Box<gen::ShippingMethod>),
    CollectionPoint(GqlCollectionPoint),
}

#[derive(SimpleObject, Clone)]
pub struct GqlCheckout {
    pub id: ID,
    pub channel: String,
    pub email: String,
    pub currency: String,
    pub total: Money,
    pub lines: Vec<GqlCheckoutLine>,
    // Saleor-compat surface (custom-API GET_CART_QUERY and friends).
    pub token: String,
    #[graphql(name = "voucherCode")]
    pub voucher_code: Option<String>,
    pub voucher: Option<GqlCheckoutVoucher>,
    pub discount: Option<Money>,
    #[graphql(name = "billingAddress")]
    pub billing_address: Option<crate::order::GqlAddress>,
    #[graphql(name = "shippingAddress")]
    pub shipping_address: Option<crate::order::GqlAddress>,
    #[graphql(name = "availableShippingMethods")]
    pub available_shipping_methods: Vec<gen::ShippingMethod>,
    #[graphql(name = "deliveryMethod")]
    pub delivery_method: Option<GqlCheckoutDelivery>,
    #[graphql(name = "subtotalPrice")]
    pub subtotal_price: crate::order::GqlTaxedMoney,
    #[graphql(name = "shippingPrice")]
    pub shipping_price: crate::order::GqlTaxedMoney,
    #[graphql(name = "totalPrice")]
    pub total_price: crate::order::GqlTaxedMoney,
}

/// Saleor `checkoutComplete` payload shape (custom-API reconciliation reads
/// `.order` + `.errors`; `confirmationNeeded` is always false — this backend
/// has no redirect/3DS step, coverage is prepaid via transactionCreate).
#[derive(SimpleObject, Clone)]
pub struct GqlCheckoutCompleteOrder {
    pub id: ID,
    pub number: String,
    pub status: String,
    pub total: crate::order::GqlTaxedMoney,
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "CheckoutComplete")]
pub struct GqlCheckoutCompletePayload {
    pub order: Option<GqlCheckoutCompleteOrder>,
    #[graphql(name = "confirmationNeeded")]
    pub confirmation_needed: bool,
    pub errors: Vec<GqlCheckoutError>,
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "CheckoutLinesDelete")]
pub struct GqlCheckoutLinesDelete {
    pub checkout: Option<GqlCheckout>,
    pub errors: Vec<GqlCheckoutError>,
}

fn ckperr(message: String) -> GqlCheckoutError {
    GqlCheckoutError {
        field: None,
        message: Some(message),
        code: None,
        variants: vec![],
    }
}

/// Coded checkout failure (Saleor `CheckoutErrorCode` names). The custom
/// API's `classifyCheckoutErrors` keys off `code`, so validation failures
/// must carry one — never collapse to `ckerr` on the write path.
fn ckerr_code(message: String, code: &str) -> GqlCheckoutError {
    GqlCheckoutError {
        field: None,
        message: Some(message),
        code: Some(code.to_string()),
        variants: vec![],
    }
}

fn ckerr_code_variants(message: String, code: &str, variants: Vec<ID>) -> GqlCheckoutError {
    GqlCheckoutError {
        field: None,
        message: Some(message),
        code: Some(code.to_string()),
        variants,
    }
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "DeliveryOptionsCalculateError")]
pub struct GqlDeliveryOptionsCalculateError {
    pub field: Option<String>,
    pub message: Option<String>,
    pub code: Option<String>,
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "CheckoutPaymentCreate")]
pub struct GqlCheckoutPaymentCreate {
    pub checkout: Option<GqlCheckout>,
    pub payment: Option<gen::Payment>,
    pub errors: Vec<GqlCheckoutError>,
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "CheckoutCreateFromOrderUnavailableVariant")]
pub struct GqlUnavailableVariant {
    pub message: Option<String>,
    pub code: Option<String>,
    #[graphql(name = "variantId")]
    pub variant_id: Option<ID>,
    #[graphql(name = "lineId")]
    pub line_id: Option<ID>,
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "CheckoutCreateFromOrder")]
pub struct GqlCheckoutCreateFromOrder {
    #[graphql(name = "unavailableVariants")]
    pub unavailable_variants: Vec<GqlUnavailableVariant>,
    pub checkout: Option<GqlCheckout>,
    pub errors: Vec<GqlCheckoutError>,
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "CollectionPoint")]
pub struct GqlCollectionPoint {
    pub id: ID,
    pub name: Option<String>,
}

#[derive(Union, Clone)]
#[graphql(name = "Delivery")]
pub enum GqlDelivery {
    ShippingMethod(Box<gen::ShippingMethodType>),
    CollectionPoint(GqlCollectionPoint),
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "DeliveryOptionsCalculate")]
pub struct GqlDeliveryOptionsCalculate {
    pub deliveries: Vec<GqlDelivery>,
    pub errors: Vec<GqlDeliveryOptionsCalculateError>,
}

/// Legacy payment row → node (gateway/total/capture state).
fn assemble_legacy_payment(
    row: saleor_rustify_db::entities::payment_payment::Model,
) -> gen::Payment {
    let cur = row.currency.clone();
    gen::Payment {
        id: Some(ID(crate::common::gid("Payment", row.id))),
        private_metadata: vec![],
        metadata: vec![],
        gateway: Some(row.gateway.clone()),
        is_active: Some(row.is_active),
        modified: Some(row.modified_at.into()),
        payment_method_type: Some(row.payment_method_type.clone()),
        actions: vec![],
        total: Some(Money {
            amount: row.total.to_string(),
            currency: cur.clone(),
            fraction_digits: None,
        }),
        captured_amount: Some(Money {
            amount: row.captured_amount.to_string(),
            currency: cur,
            fraction_digits: None,
        }),
        transactions: vec![],
        available_capture_amount: None,
        available_refund_amount: None,
    }
}

fn taxed(amount: rust_decimal::Decimal, currency: &str) -> crate::order::GqlTaxedMoney {
    let m = Money {
        amount: amount.to_string(),
        currency: currency.to_string(),
        fraction_digits: None,
    };
    crate::order::GqlTaxedMoney {
        gross: m.clone(),
        net: m,
        tax: None,
        currency: Some(currency.to_string()),
    }
}

pub(crate) fn to_gql_checkout(
    co: &saleor_rustify_db::entities::checkout_checkout::Model,
    lines: &[saleor_rustify_db::entities::checkout_checkoutline::Model],
    channel: &str,
) -> GqlCheckout {
    let d = saleor_rustify_db::checkout_store::to_domain(co, lines, channel);
    let total = Money {
        amount: co.total_gross_amount.to_string(),
        currency: co.currency.clone(),
        fraction_digits: None,
    };
    GqlCheckout {
        id: ID(crate::common::gid("Checkout", &d.id)),
        channel: d.channel,
        email: d.email,
        currency: d.currency.clone(),
        total: total.clone(),
        lines: d
            .lines
            .into_iter()
            .map(|l| {
                let tot = if l.is_gift {
                    rust_decimal::Decimal::ZERO
                } else {
                    l.unit_price.amount * rust_decimal::Decimal::from(l.quantity)
                };
                let currency = l.unit_price.currency.clone();
                GqlCheckoutLine {
                    id: ID(crate::common::gid(
                        "CheckoutLine",
                        format!("{}:{}", d.id, l.variant_id),
                    )),
                    variant_id: ID(crate::common::gid("ProductVariant", &l.variant_id)),
                    quantity: l.quantity,
                    unit_price: l.unit_price.into(),
                    total_price: taxed(tot, &currency),
                    is_gift: l.is_gift,
                    variant: None,
                }
            })
            .collect(),
        token: d.id,
        voucher_code: co.voucher_code.clone(),
        voucher: None,
        discount: Some(Money {
            amount: co.discount_amount.to_string(),
            currency: co.currency.clone(),
            fraction_digits: None,
        }),
        billing_address: None,
        shipping_address: None,
        available_shipping_methods: vec![],
        delivery_method: None,
        subtotal_price: taxed(co.subtotal_gross_amount, &co.currency),
        shipping_price: taxed(co.shipping_price_gross_amount, &co.currency),
        total_price: taxed(co.total_gross_amount, &co.currency),
    }
}

#[derive(InputObject)]
pub struct AddLineInput {
    pub variant_id: ID,
    pub quantity: i32,
}

#[derive(SimpleObject, Clone)]
pub struct GqlCheckoutLineEdge {
    pub node: Option<GqlCheckoutLine>,
}

#[derive(SimpleObject, Clone)]
pub struct GqlCheckoutLineConnection {
    #[graphql(name = "totalCount")]
    pub total_count: Option<i32>,
    pub edges: Vec<GqlCheckoutLineEdge>,
    #[graphql(name = "pageInfo")]
    pub page_info: crate::common::PageInfo,
}

#[derive(SimpleObject, Clone)]
pub struct AgentCheckoutResult {
    pub checkout: GqlCheckout,
    pub notes: Vec<String>,
    pub questions: Vec<String>,
}

/// Saleor new-checkout-API error shape (field/message/code-as-string).
/// `variants` carries the offending variant ids for stock/availability
/// failures (the custom API reads `errors { ... variants }` on create).
#[derive(SimpleObject, Clone)]
pub struct GqlCheckoutError {
    pub field: Option<String>,
    pub message: Option<String>,
    pub code: Option<String>,
    pub variants: Vec<ID>,
}

macro_rules! new_api_payload {
    ($name:ident) => {
        #[derive(SimpleObject, Clone)]
        pub struct $name {
            pub checkout: Option<GqlCheckout>,
            pub errors: Vec<GqlCheckoutError>,
        }
    };
}

new_api_payload!(CheckoutCreate);
new_api_payload!(CheckoutLinesAdd);
new_api_payload!(CheckoutLinesUpdate);
new_api_payload!(CheckoutLineDelete);
new_api_payload!(CheckoutDelete);
new_api_payload!(CheckoutEmailUpdate);
new_api_payload!(CheckoutCustomerAttach);
new_api_payload!(CheckoutCustomerDetach);
new_api_payload!(CheckoutCustomerNoteUpdate);
new_api_payload!(CheckoutShippingAddressUpdate);
new_api_payload!(CheckoutBillingAddressUpdate);
new_api_payload!(CheckoutShippingMethodUpdate);
new_api_payload!(CheckoutDeliveryMethodUpdate);
new_api_payload!(CheckoutLanguageCodeUpdate);
new_api_payload!(CheckoutAddPromoCode);
new_api_payload!(CheckoutRemovePromoCode);

#[derive(InputObject)]
pub struct CheckoutCreateInput {
    pub channel: Option<String>,
    pub lines: Vec<gen::CheckoutLineInput>,
    pub email: Option<String>,
    #[graphql(name = "shippingAddress")]
    pub shipping_address: Option<gen::AddressInput>,
    #[graphql(name = "billingAddress")]
    pub billing_address: Option<gen::AddressInput>,
    #[graphql(name = "languageCode")]
    pub language_code: Option<gen::LanguageCodeEnum>,
    pub metadata: Option<Vec<crate::common::MetadataInput>>,
}

fn ckerr(message: String) -> Vec<GqlCheckoutError> {
    vec![GqlCheckoutError {
        field: None,
        message: Some(message),
        code: None,
        variants: vec![],
    }]
}

fn ckerrs(message: String, code: &str) -> Vec<GqlCheckoutError> {
    vec![ckerr_code(message, code)]
}

/// Resolve the checkout token from the new-API id soup (`id` preferred,
/// deprecated `checkoutId`/`token` honored).
fn resolve_token(
    id: Option<ID>,
    checkout_id: Option<ID>,
    token: Option<String>,
) -> std::result::Result<Uuid, String> {
    for cand in [id, checkout_id] {
        if let Some(i) = cand {
            if let Some(t) = crate::common::parse_uuid_gid(&i.0) {
                return Ok(t);
            }
        }
    }
    if let Some(t) = token.and_then(|s| s.parse::<Uuid>().ok()) {
        return Ok(t);
    }
    Err("checkout id is required".to_string())
}

fn parse_price(p: &Option<gen::GenPositiveDecimal>) -> Option<rust_decimal::Decimal> {
    p.as_ref().and_then(|g| g.0.parse().ok())
}

async fn load_gql(db: &sea_orm::DatabaseConnection, token: Uuid, ch: &str) -> Result<GqlCheckout> {
    assemble_full_checkout(db, token, ch).await
}

/// Saleor write-path validation for checkout lines: thin GraphQL map
/// over the shared [`checkout_store::validate_lines`] choke point (same
/// checks serve gRPC writers). Returns a coded error (never codeless)
/// with the offending variant ids attached.
async fn validate_checkout_lines(
    db: &sea_orm::DatabaseConnection,
    ch_id: i32,
    items: &[(i32, i32)],
) -> std::result::Result<(), GqlCheckoutError> {
    match saleor_rustify_db::checkout_store::validate_lines(db, ch_id, items).await {
        Ok(()) => Ok(()),
        Err(inv) => Err(ckerr_code_variants(
            inv.message,
            inv.code,
            inv.variants
                .into_iter()
                .map(|v| ID(crate::common::gid("ProductVariant", v)))
                .collect(),
        )),
    }
}

/// Voucher summary for the checkout response (code → voucher + channel
/// listing value). Codes/types are uppercased to Saleor enum wire names.
async fn voucher_summary(
    db: &sea_orm::DatabaseConnection,
    code: &str,
    ch_id: i32,
) -> Option<GqlCheckoutVoucher> {
    use saleor_rustify_db::entities::{
        discount_voucher, discount_voucherchannellisting, discount_vouchercode,
    };
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let code_row = discount_vouchercode::Entity::find()
        .filter(discount_vouchercode::Column::Code.eq(code))
        .one(db)
        .await
        .ok()??;
    let v = discount_voucher::Entity::find_by_id(code_row.voucher_id)
        .one(db)
        .await
        .ok()??;
    let listing = discount_voucherchannellisting::Entity::find()
        .filter(discount_voucherchannellisting::Column::VoucherId.eq(v.id))
        .filter(discount_voucherchannellisting::Column::ChannelId.eq(ch_id))
        .one(db)
        .await
        .ok()??;
    Some(GqlCheckoutVoucher {
        id: ID(crate::common::gid("Voucher", v.id)),
        code: code.to_string(),
        name: v.name.clone(),
        voucher_type: v.r#type.to_uppercase(),
        discount_value_type: v.discount_value_type.to_uppercase(),
        discount_value: listing.discount_value.to_string(),
    })
}

/// Full Saleor-compat checkout assembly: voucher, addresses, shipping
/// methods, delivery method, priced lines with variant data.
async fn assemble_full_checkout(
    db: &sea_orm::DatabaseConnection,
    token: Uuid,
    ch: &str,
) -> Result<GqlCheckout> {
    use saleor_rustify_db::entities::{
        account_address, product_product, product_productmedia, product_productvariant,
        shipping_shippingmethod, shipping_shippingmethodchannellisting, warehouse_stock,
    };
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let (co, lines) = saleor_rustify_db::checkout_store::load_checkout(db, token)
        .await
        .map_err(|e| Error::new(e.to_string()))?
        .ok_or_else(|| Error::new("checkout not found"))?;
    let mut out = to_gql_checkout(&co, &lines, ch);
    // Voucher.
    if let Some(code) = co.voucher_code.clone().filter(|c| !c.is_empty()) {
        out.voucher = voucher_summary(db, &code, co.channel_id).await;
    }
    // Addresses.
    for (aid, slot) in [
        (co.billing_address_id, &mut out.billing_address),
        (co.shipping_address_id, &mut out.shipping_address),
    ] {
        if let Some(id) = aid {
            if let Ok(Some(row)) = account_address::Entity::find_by_id(id).one(db).await {
                *slot = Some(crate::account::to_gql_address(&row));
            }
        }
    }
    // Shipping methods with a listing on this channel.
    let listings = shipping_shippingmethodchannellisting::Entity::find()
        .filter(shipping_shippingmethodchannellisting::Column::ChannelId.eq(co.channel_id))
        .all(db)
        .await
        .map_err(|e| Error::new(e.to_string()))?;
    let mids: Vec<i32> = listings.iter().map(|l| l.shipping_method_id).collect();
    let methods = if mids.is_empty() {
        vec![]
    } else {
        shipping_shippingmethod::Entity::find()
            .filter(shipping_shippingmethod::Column::Id.is_in(mids))
            .all(db)
            .await
            .map_err(|e| Error::new(e.to_string()))?
    };
    let mname: std::collections::HashMap<i32, String> =
        methods.into_iter().map(|m| (m.id, m.name)).collect();
    out.available_shipping_methods = listings
        .into_iter()
        .map(|l| gen::ShippingMethod {
            id: Some(ID(crate::common::gid(
                "ShippingMethod",
                l.shipping_method_id,
            ))),
            private_metadata: vec![],
            metadata: vec![],
            name: mname.get(&l.shipping_method_id).cloned(),
            price: Some(crate::common::Money {
                amount: l.price_amount.to_string(),
                currency: l.currency.clone(),
                fraction_digits: None,
            }),
            active: Some(true),
            message: None,
        })
        .collect();
    // Delivery method.
    if let Some(mid) = co.shipping_method_id {
        let name = mname.get(&mid).cloned();
        out.delivery_method = Some(GqlCheckoutDelivery::ShippingMethod(Box::new(
            gen::ShippingMethod {
                id: Some(ID(crate::common::gid("ShippingMethod", mid))),
                private_metadata: vec![],
                metadata: vec![],
                name,
                price: None,
                active: Some(true),
                message: None,
            },
        )));
    } else if let Some(wid) = co.collection_point_id {
        // Warehouse PKs are ints; the stored UUID is click&collect parity —
        // callers only fragment on ShippingMethod here, name is best-effort.
        out.delivery_method = Some(GqlCheckoutDelivery::CollectionPoint(GqlCollectionPoint {
            id: ID(crate::common::gid("Warehouse", wid)),
            name: None,
        }));
    }
    // Line variant data (batched).
    if !lines.is_empty() {
        let vids: Vec<i32> = lines.iter().map(|l| l.variant_id).collect();
        let vars = product_productvariant::Entity::find()
            .filter(product_productvariant::Column::Id.is_in(vids.clone()))
            .all(db)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        let pids: Vec<i32> = vars.iter().map(|v| v.product_id).collect();
        let prods = if pids.is_empty() {
            vec![]
        } else {
            product_product::Entity::find()
                .filter(product_product::Column::Id.is_in(pids.clone()))
                .all(db)
                .await
                .map_err(|e| Error::new(e.to_string()))?
        };
        let pname: std::collections::HashMap<i32, String> =
            prods.into_iter().map(|p| (p.id, p.name)).collect();
        let thumbs: std::collections::HashMap<i32, (String, String)> = if pids.is_empty() {
            Default::default()
        } else {
            use sea_orm::QueryOrder;
            let rows = product_productmedia::Entity::find()
                .filter(product_productmedia::Column::ProductId.is_in(pids))
                .order_by_asc(product_productmedia::Column::SortOrder)
                .order_by_asc(product_productmedia::Column::Id)
                .all(db)
                .await
                .map_err(|e| Error::new(e.to_string()))?;
            let mut m = std::collections::HashMap::new();
            for r in rows {
                if let Some(pid) = r.product_id {
                    m.entry(pid).or_insert_with(|| {
                        (
                            crate::common::media_url(r.image.as_deref().unwrap_or_default()),
                            r.alt.clone(),
                        )
                    });
                }
            }
            m
        };
        let stock_rows = warehouse_stock::Entity::find()
            .filter(warehouse_stock::Column::ProductVariantId.is_in(vids))
            .all(db)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        let mut stock: std::collections::HashMap<i32, i32> = std::collections::HashMap::new();
        for s in stock_rows {
            *stock.entry(s.product_variant_id).or_insert(0) +=
                (s.quantity - s.quantity_allocated).max(0);
        }
        let vmap: std::collections::HashMap<
            i32,
            &saleor_rustify_db::entities::product_productvariant::Model,
        > = vars.iter().map(|v| (v.id, v)).collect();
        for (line, gline) in lines.iter().zip(out.lines.iter_mut()) {
            if let Some(v) = vmap.get(&line.variant_id) {
                let eff_unit = if line.quantity > 0 {
                    line.total_price_gross_amount / rust_decimal::Decimal::from(line.quantity)
                } else {
                    line.undiscounted_unit_price_amount
                };
                let cur = line.currency.clone();
                let (thumb_url, thumb_alt) = thumbs
                    .get(&v.product_id)
                    .cloned()
                    .unwrap_or((String::new(), String::new()));
                gline.variant = Some(GqlCheckoutVariant {
                    id: ID(crate::common::gid("ProductVariant", v.id)),
                    name: v.name.clone(),
                    product: GqlCheckoutProduct {
                        name: pname.get(&v.product_id).cloned().unwrap_or_default(),
                        thumbnail: if thumb_url.is_empty() {
                            None
                        } else {
                            Some(GqlCheckoutThumbnail {
                                url: Some(thumb_url),
                                alt: Some(thumb_alt),
                            })
                        },
                    },
                    pricing: GqlCheckoutVariantPricing {
                        price: taxed(eff_unit, &cur),
                        price_undiscounted: taxed(line.undiscounted_unit_price_amount, &cur),
                    },
                    quantity_available: stock.get(&v.id).copied().unwrap_or(0),
                });
            }
        }
    }
    Ok(out)
}

/// Persist a storefront address input onto a checkout side (Django creates
/// an owned address row; validation/skipping rules honored minimally).
async fn save_checkout_address(
    db: &sea_orm::DatabaseConnection,
    token: Uuid,
    billing: bool,
    a: &gen::AddressInput,
) -> Result<(), String> {
    let (Some(first), Some(last), Some(street), Some(city), Some(postal), Some(country)) = (
        a.first_name.clone(),
        a.last_name.clone(),
        a.street_address1.clone(),
        a.city.clone(),
        a.postal_code.clone(),
        a.country.as_ref().map(|c| format!("{c:?}")),
    ) else {
        return Err(
            "first name, last name, street, city, postal code and country are required".into(),
        );
    };
    saleor_rustify_db::checkout_store::set_checkout_address(
        db, token, billing, &first, &last, &street, &city, &postal, &country,
    )
    .await
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// Price lines through channel listings; app price overrides honored only
/// with `handle_checkouts` (Django `CheckoutLineInput.price` rule).
async fn unit_prices(
    ctx: &Context<'_>,
    db: &sea_orm::DatabaseConnection,
    ch: &str,
    items: &[(i32, Option<rust_decimal::Decimal>)],
) -> Result<std::collections::HashMap<i32, rust_decimal::Decimal>> {
    let vids: Vec<i32> = items.iter().map(|(v, _)| *v).collect();
    let pricing = saleor_rustify_db::catalog::checkout_pricing(db, ch, &vids)
        .await
        .map_err(|e| Error::new(e.to_string()))?;
    let can_override = crate::account::require_perm(ctx, "handle_checkouts")
        .await
        .is_ok();
    Ok(items
        .iter()
        .map(|(vid, ov)| {
            let unit = match (can_override, ov) {
                (true, Some(p)) => *p,
                _ => pricing
                    .get(vid)
                    .map(|(m, _)| m.amount)
                    .unwrap_or(rust_decimal::Decimal::ZERO),
            };
            (*vid, unit)
        })
        .collect())
}

#[derive(Default)]
pub struct CheckoutQuery;

#[Object]
impl CheckoutQuery {
    async fn checkout(&self, ctx: &Context<'_>, id: ID) -> Result<Option<GqlCheckout>> {
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let token: Uuid =
            crate::common::parse_uuid_gid(&id.0).ok_or_else(|| Error::new("id must be UUID"))?;
        let ch = match saleor_rustify_db::checkout_store::load_checkout(db, token)
            .await
            .map_err(|e| Error::new(e.to_string()))?
        {
            Some((co, _)) => {
                saleor_rustify_db::catalog::channel_slug_for_id(db, co.channel_id)
                    .await
                    .map_err(|e| Error::new(e.to_string()))?
            }
            None => return Ok(None),
        };
        Ok(Some(assemble_full_checkout(db, token, &ch).await?))
    }

    /// Abandoned/active checkout list (Django `checkouts`; staff only).
    /// Slim select (token + created_at) ordered + paginated in memory, same
    /// pattern as `orders`/`customers`. Supports `channel` slug, `filter`
    /// (customer/search/created/updatedAt/channels/authorize/charge statuses)
    /// and `sortBy` (CREATION_DATE/CUSTOMER; PAYMENT/RANK fall back to
    /// creation date — no payment-rank column exists upstream either).
    /// `metadata` sub-filter stays accepted-ignored (no GIN index; documented).
    async fn checkouts(
        &self,
        ctx: &Context<'_>,
        #[graphql(name = "sortBy")] sort_by: Option<gen::CheckoutSortingInput>,
        filter: Option<gen::CheckoutFilterInput>,
        channel: Option<String>,
        before: Option<String>,
        after: Option<String>,
        first: Option<i32>,
        last: Option<i32>,
    ) -> Result<gen::CheckoutCountableConnection> {
        crate::account::require_any_perm(ctx, &["manage_orders", "manage_checkouts"]).await?;
        let _ = (before, last);
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let off = after
            .and_then(|c| crate::common::decode_cursor(&c))
            .unwrap_or(0);
        let lim = first.unwrap_or(20).clamp(1, 100) as usize;
        let empty = || gen::CheckoutCountableConnection {
            edges: vec![],
            page_info: Some(crate::common::PageInfo {
                has_next_page: false,
                has_previous_page: false,
                start_cursor: None,
                end_cursor: None,
            }),
        };
        use saleor_rustify_db::entities::checkout_checkout::{Column as CCol, Entity as CEnt};
        use sea_orm::{ColumnTrait, Condition, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
        let mut cond = Condition::all();
        if let Some(slug) = channel.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
            use saleor_rustify_db::entities::channel_channel::{Column as ChCol, Entity as ChEnt};
            let cid: Option<i32> = ChEnt::find()
                .select_only()
                .column(ChCol::Id)
                .filter(ChCol::Slug.eq(slug))
                .into_tuple::<i32>()
                .one(db)
                .await
                .map_err(|e| Error::new(e.to_string()))?;
            match cid {
                Some(id) => {
                    cond = cond.add(CCol::ChannelId.eq(id));
                }
                None => return Ok(empty()),
            }
        }
        if let Some(f) = filter.as_ref() {
            if let Some(c) = f
                .customer
                .as_ref()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
            {
                cond = cond.add(CCol::Email.like(format!("%{c}%")));
            }
            if let Some(s) = f
                .search
                .as_ref()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
            {
                let mut any = Condition::any().add(CCol::Email.like(format!("%{s}%")));
                if let Ok(u) = s.parse::<Uuid>() {
                    any = any.add(CCol::Token.eq(u));
                }
                cond = cond.add(any);
            }
            if let Some(r) = f.created.as_ref() {
                if let Some(gte) = r.gte {
                    cond = cond.add(CCol::CreatedAt.gte(gte));
                }
                if let Some(lte) = r.lte {
                    cond = cond.add(CCol::CreatedAt.lte(lte));
                }
            }
            if let Some(r) = f.updated_at.as_ref() {
                if let Some(gte) = r.gte {
                    cond = cond.add(CCol::LastChange.gte(gte));
                }
                if let Some(lte) = r.lte {
                    cond = cond.add(CCol::LastChange.lte(lte));
                }
            }
            let ch_ids: Vec<i32> = f
                .channels
                .clone()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|i| saleor_rustify_db::catalog::parse_gid(&i.0))
                .collect();
            if !ch_ids.is_empty() {
                cond = cond.add(CCol::ChannelId.is_in(ch_ids));
            }
            let auth: Vec<String> = f
                .authorize_status
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(|s| format!("{s:?}").to_lowercase())
                .collect();
            if !auth.is_empty() {
                cond = cond.add(CCol::AuthorizeStatus.is_in(auth));
            }
            let chg: Vec<String> = f
                .charge_status
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(|s| format!("{s:?}").to_lowercase())
                .collect();
            if !chg.is_empty() {
                cond = cond.add(CCol::ChargeStatus.is_in(chg));
            }
        }
        let asc = sort_by
            .as_ref()
            .map(|s| matches!(s.direction, gen::OrderDirection::ASC))
            .unwrap_or(false);
        let mut q = CEnt::find()
            .select_only()
            .column(CCol::Token)
            .column(CCol::CreatedAt)
            .filter(cond);
        q = match sort_by.as_ref().map(|s| &s.field) {
            Some(gen::CheckoutSortField::CUSTOMER) => {
                if asc {
                    q.order_by_asc(CCol::Email)
                } else {
                    q.order_by_desc(CCol::Email)
                }
            }
            _ => {
                if asc {
                    q.order_by_asc(CCol::CreatedAt)
                } else {
                    q.order_by_desc(CCol::CreatedAt)
                }
            }
        };
        let rows: Vec<(Uuid, chrono::DateTime<chrono::FixedOffset>)> = q
            .into_tuple()
            .all(db)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        let total = rows.len();
        let edges = rows
            .into_iter()
            .skip(off)
            .take(lim)
            .enumerate()
            .map(|(i, (token, created))| gen::CheckoutCountableEdge {
                node: Some(gen::Checkout {
                    id: Some(ID(crate::common::gid("Checkout", token))),
                    created: Some(created.with_timezone(&chrono::Utc)),
                }),
                cursor: Some(crate::common::encode_cursor(off + i)),
            })
            .collect();
        Ok(gen::CheckoutCountableConnection {
            edges,
            page_info: Some(crate::common::PageInfo {
                has_next_page: off + lim < total,
                has_previous_page: off > 0,
                start_cursor: None,
                end_cursor: None,
            }),
        })
    }

    /// Admin checkout-lines view (paginated across checkouts).
    async fn checkout_lines(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        before: Option<String>,
        last: Option<i32>,
    ) -> Result<GqlCheckoutLineConnection> {
        crate::account::require_perm(ctx, "manage_orders").await?;
        let _ = (before, last);
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let off = after
            .and_then(|c| crate::common::decode_cursor(&c))
            .unwrap_or(0);
        let lim = first.unwrap_or(20).clamp(1, 100) as usize;
        use sea_orm::{ConnectionTrait, Statement};
        let rows = db.query_all(Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT checkout_id, variant_id, quantity, currency, total_price_gross_amount, is_gift, undiscounted_unit_price_amount FROM checkout_checkoutline ORDER BY created_at DESC, id LIMIT 500".to_string(),
        )).await.map_err(|e| Error::new(e.to_string()))?;
        let total = rows.len();
        let edges = rows
            .into_iter()
            .skip(off)
            .take(lim)
            .map(|r| {
                let vid: i32 = r.try_get("", "variant_id").unwrap_or(0);
                let qty: i32 = r.try_get("", "quantity").unwrap_or(0);
                let cur: String = r.try_get("", "currency").unwrap_or_else(|_| "USD".into());
                let unit: rust_decimal::Decimal = r
                    .try_get("", "undiscounted_unit_price_amount")
                    .unwrap_or(rust_decimal::Decimal::ZERO);
                let tot: rust_decimal::Decimal = r
                    .try_get("", "total_price_gross_amount")
                    .unwrap_or(unit * rust_decimal::Decimal::from(qty));
                let gift: bool = r.try_get("", "is_gift").unwrap_or(false);
                GqlCheckoutLineEdge {
                    node: Some(GqlCheckoutLine {
                        id: ID(crate::common::gid("CheckoutLine", format!("{vid}:{qty}"))),
                        variant_id: ID(crate::common::gid("ProductVariant", vid)),
                        quantity: qty,
                        unit_price: crate::common::Money {
                            amount: unit.to_string(),
                            currency: cur.clone(),
                            fraction_digits: None,
                        },
                        total_price: taxed(tot, &cur),
                        is_gift: gift,
                        variant: None,
                    }),
                }
            })
            .collect();
        Ok(GqlCheckoutLineConnection {
            total_count: Some(total as i32),
            edges,
            page_info: crate::common::PageInfo {
                has_next_page: off + lim < total,
                has_previous_page: off > 0,
                start_cursor: None,
                end_cursor: None,
            },
        })
    }
}

#[derive(Default)]
pub struct CheckoutMutation;

#[Object]
impl CheckoutMutation {
    /// Legacy payment create on a checkout (Django `checkoutPaymentCreate`):
    /// book a gateway payment row for the checkout total (or the given
    /// amount). The transaction-based flows supersede this; the row stays
    /// queryable for legacy readers.
    async fn checkout_payment_create(
        &self,
        ctx: &Context<'_>,
        #[graphql(name = "checkoutId")] checkout_id: Option<ID>,
        id: Option<ID>,
        input: gen::PaymentInput,
        token: Option<String>,
    ) -> Result<GqlCheckoutPaymentCreate> {
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let err = |m: String| GqlCheckoutPaymentCreate {
            checkout: None,
            payment: None,
            errors: vec![ckperr(m)],
        };
        let ctoken = checkout_id
            .as_ref()
            .and_then(|c| crate::common::parse_uuid_gid(&c.0))
            .or_else(|| {
                id.as_ref()
                    .and_then(|c| crate::common::parse_uuid_gid(&c.0))
            })
            .or_else(|| token.as_ref().and_then(|t| t.parse::<Uuid>().ok()));
        let Some(ctoken) = ctoken else {
            return Ok(err("checkout id is required".into()));
        };
        let (co, _) = saleor_rustify_db::checkout_store::load_checkout(db, ctoken)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .ok_or_else(|| Error::new("checkout not found"))?;
        if input.gateway.trim().is_empty() {
            return Ok(err("gateway is required".into()));
        }
        let amount = match input
            .amount
            .as_ref()
            .map(|a| a.0.parse::<rust_decimal::Decimal>())
        {
            Some(Ok(a)) if a > rust_decimal::Decimal::ZERO => a,
            Some(_) => return Ok(err("amount must be positive".into())),
            None => co.total_gross_amount,
        };
        let row = saleor_rustify_db::entities::payment_payment::ActiveModel {
            gateway: sea_orm::Set(input.gateway.clone()),
            is_active: sea_orm::Set(true),
            created_at: sea_orm::Set(chrono::Utc::now().into()),
            modified_at: sea_orm::Set(chrono::Utc::now().into()),
            charge_status: sea_orm::Set("not-charged".to_string()),
            billing_first_name: sea_orm::Set(String::new()),
            billing_last_name: sea_orm::Set(String::new()),
            billing_company_name: sea_orm::Set(String::new()),
            billing_address_1: sea_orm::Set(String::new()),
            billing_address_2: sea_orm::Set(String::new()),
            billing_city: sea_orm::Set(String::new()),
            billing_city_area: sea_orm::Set(String::new()),
            billing_postal_code: sea_orm::Set(String::new()),
            billing_country_code: sea_orm::Set(String::new()),
            billing_country_area: sea_orm::Set(String::new()),
            billing_email: sea_orm::Set(co.email.clone().unwrap_or_default()),
            cc_brand: sea_orm::Set(String::new()),
            cc_first_digits: sea_orm::Set(String::new()),
            cc_last_digits: sea_orm::Set(String::new()),
            extra_data: sea_orm::Set(String::new()),
            token: sea_orm::Set(input.token.clone().unwrap_or_default()),
            currency: sea_orm::Set(co.currency.clone()),
            total: sea_orm::Set(amount),
            captured_amount: sea_orm::Set(rust_decimal::Decimal::ZERO),
            checkout_id: sea_orm::Set(Some(ctoken)),
            to_confirm: sea_orm::Set(false),
            payment_method_type: sea_orm::Set(String::new()),
            return_url: sea_orm::Set(input.return_url.clone()),
            partial: sea_orm::Set(false),
            metadata: sea_orm::Set(serde_json::json!({})),
            private_metadata: sea_orm::Set(serde_json::json!({})),
            store_payment_method: sea_orm::Set("none".to_string()),
            ..Default::default()
        };
        use sea_orm::ActiveModelTrait;
        let inserted = row
            .insert(db)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        // Never hardcode channel slugs (multi-channel correctness):
        // resolve the checkout's real channel from the database.
        // The checkout's own channel — resolved, never defaulted: a
        // missing channel row is data corruption and must error loudly,
        // not silently serve another shop's catalog.
        let ch = saleor_rustify_db::catalog::channel_slug_for_id(db, co.channel_id)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        let checkout = load_gql(db, ctoken, &ch).await?;
        Ok(GqlCheckoutPaymentCreate {
            checkout: Some(checkout),
            payment: Some(assemble_legacy_payment(inserted)),
            errors: vec![],
        })
    }

    /// Create a checkout from an order's lines (Django
    /// `checkoutCreateFromOrder`): variants that can't be listed are
    /// reported, the rest land on a fresh checkout.
    async fn checkout_create_from_order(
        &self,
        ctx: &Context<'_>,
        id: ID,
    ) -> Result<GqlCheckoutCreateFromOrder> {
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let err = |m: String| GqlCheckoutCreateFromOrder {
            unavailable_variants: vec![],
            checkout: None,
            errors: vec![ckperr(m)],
        };
        let oid = crate::common::parse_uuid_gid(&id.0).ok_or_else(|| Error::new("bad order id"))?;
        let (h, ls) = saleor_rustify_db::order_store::get_order_rows(db, oid)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .ok_or_else(|| Error::new("order not found"))?;
        let ch = saleor_rustify_db::catalog::channel_slug_for_id(db, h.channel_id)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        let (ch_id, currency) = saleor_rustify_db::catalog::channel_info(db, &ch)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        let token = saleor_rustify_db::checkout_store::create_checkout_row(
            db,
            ch_id,
            &currency,
            &h.user_email,
        )
        .await
        .map_err(|e| Error::new(e.to_string()))?;
        let vids: Vec<i32> = ls.iter().filter_map(|l| l.variant_id).collect();
        let pricing = saleor_rustify_db::catalog::checkout_pricing(db, &ch, &vids)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        let mut unavailable = vec![];
        let mut items = vec![];
        for l in &ls {
            match l
                .variant_id
                .and_then(|v| pricing.get(&v).map(|(m, _)| (v, m.amount)))
            {
                Some((vid, unit)) => items.push(saleor_rustify_db::checkout_store::NewLine {
                    variant_id: vid,
                    quantity: l.quantity,
                    unit_price: unit,
                    price_override: None,
                }),
                None => unavailable.push(GqlUnavailableVariant {
                    message: Some("variant is not available in this channel".to_string()),
                    code: None,
                    variant_id: l
                        .variant_id
                        .map(|v| ID(crate::common::gid("ProductVariant", v))),
                    line_id: Some(ID(crate::common::gid("OrderLine", l.id))),
                }),
            }
        }
        if !items.is_empty() {
            if let Err(e) =
                saleor_rustify_db::checkout_store::add_lines_tx(db, token, ch_id, &currency, &items)
                    .await
            {
                return Ok(err(e.to_string()));
            }
        }
        let checkout = load_gql(db, token, &ch).await?;
        Ok(GqlCheckoutCreateFromOrder {
            unavailable_variants: unavailable,
            checkout: Some(checkout),
            errors: vec![],
        })
    }

    /// Delivery options for a checkout (Django `deliveryOptionsCalculate`):
    /// channel shipping methods + pickup-enabled warehouses.
    async fn delivery_options_calculate(
        &self,
        ctx: &Context<'_>,
        id: ID,
    ) -> Result<GqlDeliveryOptionsCalculate> {
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let ctoken =
            crate::common::parse_uuid_gid(&id.0).ok_or_else(|| Error::new("bad checkout id"))?;
        let (co, _) = saleor_rustify_db::checkout_store::load_checkout(db, ctoken)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .ok_or_else(|| Error::new("checkout not found"))?;
        let mut deliveries = vec![];
        // Shipping methods with a listing on this channel.
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
        let methods = saleor_rustify_db::entities::shipping_shippingmethod::Entity::find()
            .all(db)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        for m in methods {
            let listed = saleor_rustify_db::entities::shipping_shippingmethodchannellisting::Entity::find()
                .filter(saleor_rustify_db::entities::shipping_shippingmethodchannellisting::Column::ShippingMethodId.eq(m.id))
                .filter(saleor_rustify_db::entities::shipping_shippingmethodchannellisting::Column::ChannelId.eq(co.channel_id))
                .one(db)
                .await
                .map_err(|e| Error::new(e.to_string()))?
                .is_some();
            if listed {
                match crate::commerce::assemble_method(db, m.id).await {
                    Ok(sm) => deliveries.push(GqlDelivery::ShippingMethod(Box::new(sm))),
                    Err(_) => {}
                }
            }
        }
        // Pickup points: channel warehouses with click&collect enabled.
        let whs = saleor_rustify_db::entities::warehouse_warehouse::Entity::find()
            .all(db)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        for w in whs {
            if w.click_and_collect_option == "disabled" {
                continue;
            }
            let linked = saleor_rustify_db::entities::warehouse_channelwarehouse::Entity::find()
                .filter(
                    saleor_rustify_db::entities::warehouse_channelwarehouse::Column::WarehouseId
                        .eq(w.id),
                )
                .filter(
                    saleor_rustify_db::entities::warehouse_channelwarehouse::Column::ChannelId
                        .eq(co.channel_id),
                )
                .one(db)
                .await
                .map_err(|e| Error::new(e.to_string()))?
                .is_some();
            if linked {
                deliveries.push(GqlDelivery::CollectionPoint(GqlCollectionPoint {
                    id: ID(crate::common::gid("Warehouse", w.id)),
                    name: Some(w.name.clone()),
                }));
            }
        }
        Ok(GqlDeliveryOptionsCalculate {
            deliveries,
            errors: vec![],
        })
    }

    /// Delete several checkout lines (Django `checkoutLinesDelete`).
    async fn checkout_lines_delete(
        &self,
        ctx: &Context<'_>,
        id: Option<ID>,
        #[graphql(name = "linesIds")] lines_ids: Vec<ID>,
        token: Option<String>,
    ) -> Result<GqlCheckoutLinesDelete> {
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let err = |m: String| GqlCheckoutLinesDelete {
            checkout: None,
            errors: vec![ckperr(m)],
        };
        let ctoken = id
            .as_ref()
            .and_then(|c| crate::common::parse_uuid_gid(&c.0))
            .or_else(|| token.as_ref().and_then(|t| t.parse::<Uuid>().ok()));
        let Some(ctoken) = ctoken else {
            return Ok(err("checkout id is required".into()));
        };
        let (co, _) = saleor_rustify_db::checkout_store::load_checkout(db, ctoken)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .ok_or_else(|| Error::new("checkout not found"))?;
        // Never hardcode channel slugs (multi-channel correctness):
        // resolve the checkout's real channel from the database.
        // The checkout's own channel — resolved, never defaulted: a
        // missing channel row is data corruption and must error loudly,
        // not silently serve another shop's catalog.
        let ch = saleor_rustify_db::catalog::channel_slug_for_id(db, co.channel_id)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        for lid in &lines_ids {
            let line_id =
                crate::common::parse_uuid_gid(&lid.0).ok_or_else(|| Error::new("bad line id"))?;
            if let Err(e) =
                saleor_rustify_db::checkout_store::delete_line(db, ctoken, line_id).await
            {
                return Ok(err(e.to_string()));
            }
        }
        let checkout = load_gql(db, ctoken, &ch).await?;
        Ok(GqlCheckoutLinesDelete {
            checkout: Some(checkout),
            errors: vec![],
        })
    }

    async fn create_checkout(
        &self,
        ctx: &Context<'_>,
        channel: Option<String>,
        email: String,
    ) -> Result<GqlCheckout> {
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let ch = channel.unwrap_or_else(saleor_rustify_db::catalog::fallback_channel_slug);
        let (ch_id, currency) = saleor_rustify_db::catalog::channel_info(db, &ch)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        let token =
            saleor_rustify_db::checkout_store::create_checkout_row(db, ch_id, &currency, &email)
                .await
                .map_err(|e| Error::new(e.to_string()))?;
        let (co, lines) = saleor_rustify_db::checkout_store::load_checkout(db, token)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .ok_or_else(|| Error::new("just-inserted checkout missing"))?;
        Ok(to_gql_checkout(&co, &lines, &ch))
    }

    async fn checkout_add_lines(
        &self,
        ctx: &Context<'_>,
        checkout_id: ID,
        lines: Vec<AddLineInput>,
    ) -> Result<GqlCheckout> {
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let token: Uuid = crate::common::parse_uuid_gid(&checkout_id.0)
            .ok_or_else(|| Error::new("checkoutId must be UUID"))?;
        let Some((co, _)) = saleor_rustify_db::checkout_store::load_checkout(db, token)
            .await
            .map_err(|e| Error::new(e.to_string()))?
        else {
            return Err(Error::new("checkout not found"));
        };
        let ch = saleor_rustify_db::catalog::channel_slug_for_id(db, co.channel_id)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        let (ch_id, currency) = saleor_rustify_db::catalog::channel_info(db, &ch)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        // Price from channel listings (same as gRPC AddLines).
        let vids: Vec<i32> = lines
            .iter()
            .filter_map(|l| saleor_rustify_db::catalog::parse_gid(&l.variant_id.0))
            .collect();
        let pricing = saleor_rustify_db::catalog::checkout_pricing(db, &ch, &vids)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        let items: Vec<saleor_rustify_db::checkout_store::NewLine> = lines
            .into_iter()
            .map(|l| {
                let vid: i32 = saleor_rustify_db::catalog::parse_gid(&l.variant_id.0).unwrap_or(0);
                let unit = pricing
                    .get(&vid)
                    .map(|(m, _)| m.amount)
                    .unwrap_or(rust_decimal::Decimal::ZERO);
                saleor_rustify_db::checkout_store::NewLine {
                    variant_id: vid,
                    quantity: l.quantity,
                    unit_price: unit,
                    price_override: None,
                }
            })
            .collect();
        saleor_rustify_db::checkout_store::add_lines_tx(db, token, ch_id, &currency, &items)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        let (co2, ls2) = saleor_rustify_db::checkout_store::load_checkout(db, token)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .ok_or_else(|| Error::new("checkout vanished"))?;
        Ok(to_gql_checkout(&co2, &ls2, &ch))
    }

    /// Saleor `checkoutComplete(id, paymentData, redirectUrl)` compat: gates
    /// (email, lines, active channel, full payment coverage) then mints via
    /// the shared atomic `complete_checkout`. Returns `{order errors}` —
    /// never a bare id — plus `confirmationNeeded: false` (no 3DS step).
    async fn checkout_complete(
        &self,
        ctx: &Context<'_>,
        id: Option<ID>,
        #[graphql(name = "checkoutId")] checkout_id: Option<ID>,
        token: Option<String>,
        #[graphql(name = "paymentData")] payment_data: Option<gen::GenJSONString>,
        #[graphql(name = "redirectUrl")] redirect_url: Option<String>,
    ) -> Result<GqlCheckoutCompletePayload> {
        let _ = (payment_data, redirect_url);
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let fail = |code: &str, msg: String| GqlCheckoutCompletePayload {
            order: None,
            confirmation_needed: false,
            errors: vec![ckerr_code(msg, code)],
        };
        let t = match resolve_token(id, checkout_id, token) {
            Ok(t) => t,
            Err(e) => return Ok(fail("NOT_FOUND", e)),
        };
        let Some((co, lines)) = saleor_rustify_db::checkout_store::load_checkout(db, t)
            .await
            .map_err(|e| Error::new(e.to_string()))?
        else {
            // The checkout row is gone — but a completed order outlives it.
            // Django answers replays with the minted order, never 404.
            if let Some((oid, number, status, total, currency)) =
                saleor_rustify_db::complete::find_completed_order(db, t)
                    .await
                    .map_err(|e| Error::new(e.to_string()))?
            {
                return Ok(GqlCheckoutCompletePayload {
                    order: Some(GqlCheckoutCompleteOrder {
                        id: ID(crate::common::gid("Order", oid)),
                        number: number.to_string(),
                        status,
                        total: taxed(total, &currency),
                    }),
                    confirmation_needed: false,
                    errors: vec![],
                });
            }
            return Ok(fail("NOT_FOUND", "checkout not found".into()));
        };
        if co.email.clone().unwrap_or_default().trim().is_empty() {
            return Ok(fail(
                "EMAIL_NOT_SET",
                "email is required to complete checkout".into(),
            ));
        }
        if lines.is_empty() {
            return Ok(fail("NO_LINES", "checkout has no lines".into()));
        }
        // Active channel gate (Django CHANNEL_INACTIVE).
        let ch_active: bool = {
            use sea_orm::{EntityTrait, QuerySelect};
            saleor_rustify_db::entities::channel_channel::Entity::find_by_id(co.channel_id)
                .select_only()
                .column(saleor_rustify_db::entities::channel_channel::Column::IsActive)
                .into_tuple::<bool>()
                .one(db)
                .await
                .map_err(|e| Error::new(e.to_string()))?
                .unwrap_or(false)
        };
        if !ch_active {
            return Ok(fail("CHANNEL_INACTIVE", "channel is inactive".into()));
        }
        // Full-coverage gate (Django CHECKOUT_NOT_FULLY_PAID): sum of charged
        // across this checkout's items must cover the total, unless the
        // channel allows unpaid orders or the total is zero.
        let total = co.total_gross_amount;
        if total > rust_decimal::Decimal::ZERO {
            let allow_unpaid: bool = {
                use sea_orm::{EntityTrait, QuerySelect};
                saleor_rustify_db::entities::channel_channel::Entity::find_by_id(co.channel_id)
                    .select_only()
                    .column(saleor_rustify_db::entities::channel_channel::Column::AllowUnpaidOrders)
                    .into_tuple::<bool>()
                    .one(db)
                    .await
                    .map_err(|e| Error::new(e.to_string()))?
                    .unwrap_or(false)
            };
            if !allow_unpaid {
                use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
                let covered: rust_decimal::Decimal =
                    saleor_rustify_db::entities::payment_transactionitem::Entity::find()
                    .select_only()
                    .column(saleor_rustify_db::entities::payment_transactionitem::Column::ChargedValue)
                    .column(saleor_rustify_db::entities::payment_transactionitem::Column::AuthorizedValue)
                    .filter(saleor_rustify_db::entities::payment_transactionitem::Column::CheckoutId.eq(t))
                    .into_tuple::<(rust_decimal::Decimal, rust_decimal::Decimal)>()
                    .all(db)
                    .await
                    .map_err(|e| Error::new(e.to_string()))?
                    .into_iter()
                    .map(|(c, a)| c + a)
                    .sum();
                if covered < total {
                    return Ok(fail(
                        "CHECKOUT_NOT_FULLY_PAID",
                        format!("checkout is not fully paid: covered {covered} of {total}"),
                    ));
                }
            }
        }
        // Address + delivery gates (Django BILLING/SHIPPING_ADDRESS_NOT_SET,
        // SHIPPING_METHOD_NOT_SET): billing is always required; shipping
        // address and a delivery method are required when any line's product
        // type needs shipment.
        if co.billing_address_id.is_none() {
            return Ok(fail(
                "BILLING_ADDRESS_NOT_SET",
                "billing address is required to complete checkout".into(),
            ));
        }
        {
            use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
            let vids: Vec<i32> = lines.iter().map(|l| l.variant_id).collect();
            let mut needs_shipping = false;
            if !vids.is_empty() {
                let pids: Vec<i32> =
                    saleor_rustify_db::entities::product_productvariant::Entity::find()
                    .select_only()
                    .column(saleor_rustify_db::entities::product_productvariant::Column::ProductId)
                    .filter(saleor_rustify_db::entities::product_productvariant::Column::Id.is_in(vids))
                    .into_tuple()
                    .all(db)
                    .await
                    .map_err(|e| Error::new(e.to_string()))?;
                if !pids.is_empty() {
                    let tids: Vec<i32> =
                        saleor_rustify_db::entities::product_product::Entity::find()
                        .select_only()
                        .column(saleor_rustify_db::entities::product_product::Column::ProductTypeId)
                        .filter(saleor_rustify_db::entities::product_product::Column::Id.is_in(pids))
                        .into_tuple()
                        .all(db)
                        .await
                        .map_err(|e| Error::new(e.to_string()))?;
                    if !tids.is_empty() {
                        needs_shipping = saleor_rustify_db::entities::product_producttype::Entity::find()
                            .filter(saleor_rustify_db::entities::product_producttype::Column::Id.is_in(tids))
                            .filter(saleor_rustify_db::entities::product_producttype::Column::IsShippingRequired.eq(true))
                            .one(db)
                            .await
                            .map_err(|e| Error::new(e.to_string()))?
                            .is_some();
                    }
                }
            }
            if needs_shipping {
                if co.shipping_address_id.is_none() {
                    return Ok(fail(
                        "SHIPPING_ADDRESS_NOT_SET",
                        "shipping address is required to complete checkout".into(),
                    ));
                }
                if co.shipping_method_id.is_none() && co.collection_point_id.is_none() {
                    return Ok(fail(
                        "SHIPPING_METHOD_NOT_SET",
                        "delivery method is required to complete checkout".into(),
                    ));
                }
            }
        }
        let out = match saleor_rustify_db::complete::complete_checkout(db, t).await {
            Ok(o) => o,
            Err(e) => {
                let msg = e.to_string();
                // Map the atomic minter's failures to caller-readable codes.
                let code =
                    if msg.contains("INSUFFICIENT_STOCK") || msg.contains("insufficient stock") {
                        "INSUFFICIENT_STOCK"
                    } else if msg.contains("not found") {
                        "NOT_FOUND"
                    } else {
                        "CHECKOUT_NOT_FULLY_PAID"
                    };
                // Default branch is intentionally the payment code: any other
                // mint failure after the gates above is money-adjacent, and
                // the reconciler must retry rather than drop the order.
                return Ok(fail(code, msg));
            }
        };
        let header = saleor_rustify_db::order_store::get_order_rows(db, out.order_id)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .map(|(h, _)| h);
        let (status, total_gross, currency) = match header {
            Some(h) => (h.status.clone(), h.total_gross_amount, h.currency.clone()),
            None => (
                out.status.clone(),
                rust_decimal::Decimal::ZERO,
                co.currency.clone(),
            ),
        };
        // Order confirmation mail (Django `send_order_confirmation` on the
        // completed-checkout path): best-effort, never fails completion.
        crate::mail::notify_order(db, out.order_id, crate::mail::OrderNotice::Confirmation).await;
        Ok(GqlCheckoutCompletePayload {
            order: Some(GqlCheckoutCompleteOrder {
                id: ID(crate::common::gid("Order", out.order_id)),
                number: out.number.to_string(),
                status,
                total: taxed(total_gross, &currency),
            }),
            confirmation_needed: false,
            errors: vec![],
        })
    }

    async fn checkout_apply_voucher(
        &self,
        ctx: &Context<'_>,
        checkout_id: ID,
        code: String,
    ) -> Result<GqlCheckout> {
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let token: Uuid = crate::common::parse_uuid_gid(&checkout_id.0)
            .ok_or_else(|| Error::new("checkoutId must be UUID"))?;
        let ch = {
            let (co, _) = saleor_rustify_db::checkout_store::load_checkout(db, token)
                .await
                .map_err(|e| Error::new(e.to_string()))?
                .ok_or_else(|| Error::new("checkout not found"))?;
            saleor_rustify_db::catalog::channel_slug_for_id(db, co.channel_id)
                .await
                .map_err(|e| Error::new(e.to_string()))?
        };
        saleor_rustify_db::promotions::apply_voucher(db, token, &code, &ch)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        saleor_rustify_db::checkout_store::refresh_totals(db, token)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        let (co2, ls2) = saleor_rustify_db::checkout_store::load_checkout(db, token)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .ok_or_else(|| Error::new("checkout vanished"))?;
        Ok(to_gql_checkout(&co2, &ls2, &ch))
    }

    async fn refresh_order_promotion(&self, ctx: &Context<'_>, checkout_id: ID) -> Result<String> {
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let token: Uuid = crate::common::parse_uuid_gid(&checkout_id.0)
            .ok_or_else(|| Error::new("checkoutId must be UUID"))?;
        let out = saleor_rustify_db::order_promotions::refresh_order_promotion(db, token)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        Ok(match out {
            saleor_rustify_db::order_promotions::RefreshOutcome::Cleared => "none".into(),
            saleor_rustify_db::order_promotions::RefreshOutcome::Discount { rule_id, amount } => {
                format!("discount:{rule_id}:{amount}")
            }
            saleor_rustify_db::order_promotions::RefreshOutcome::Gift {
                rule_id,
                variant_id,
                ..
            } => format!("gift:{rule_id}:{variant_id}"),
        })
    }

    /// Agentic buying: natural-language text → a real checkout with lines.
    /// Deterministic (no LLM): quantities + product phrases resolve against
    /// the catalog; unmatched phrases come back as `questions`, never as
    /// hallucinated lines. Custom root (not in the Saleor schema, so
    /// codegen never touches it).
    // ------------------------------------------------------------------
    // Saleor new-checkout-API names (storefront parity; same tables/logic
    // as the old-style roots above). All public (Django leaves checkout
    // mutations permission-free); price overrides need handle_checkouts.
    // ------------------------------------------------------------------

    async fn checkout_create(
        &self,
        ctx: &Context<'_>,
        input: CheckoutCreateInput,
    ) -> Result<CheckoutCreate> {
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let ch = input
            .channel
            .clone()
            .unwrap_or_else(saleor_rustify_db::catalog::fallback_channel_slug);
        let (ch_id, currency) = saleor_rustify_db::catalog::channel_info(db, &ch)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        if input.lines.is_empty() {
            return Ok(CheckoutCreate {
                checkout: None,
                errors: ckerr("at least one line is required".into()),
            });
        }
        // Saleor write-path validation (stock/publish/limits/channel) with
        // coded errors — unparseable ids are NOT_FOUND, not silent skips.
        let raw: Vec<Option<(i32, i32)>> = input
            .lines
            .iter()
            .map(|l| {
                saleor_rustify_db::catalog::parse_gid(&l.variant_id.0).map(|v| (v, l.quantity))
            })
            .collect();
        if raw.iter().any(|r| r.is_none()) {
            return Ok(CheckoutCreate {
                checkout: None,
                errors: ckerrs("unknown variants".into(), "NOT_FOUND"),
            });
        }
        let raw: Vec<(i32, i32)> = raw.into_iter().flatten().collect();
        if let Err(e) = validate_checkout_lines(db, ch_id, &raw).await {
            return Ok(CheckoutCreate {
                checkout: None,
                errors: vec![e],
            });
        }
        let token = saleor_rustify_db::checkout_store::create_checkout_row(
            db,
            ch_id,
            &currency,
            input.email.as_deref().unwrap_or(""),
        )
        .await
        .map_err(|e| Error::new(e.to_string()))?;
        // `raw` is parse-validated above, so indices align 1:1 here.
        let items: Vec<(i32, Option<rust_decimal::Decimal>)> = input
            .lines
            .iter()
            .zip(raw.iter())
            .map(|(l, (v, _))| (*v, parse_price(&l.price)))
            .collect();
        let units = unit_prices(ctx, db, &ch, &items).await?;
        for ((vid, qty), (_, _)) in raw.iter().zip(items.iter()) {
            let unit = units
                .get(vid)
                .copied()
                .unwrap_or(rust_decimal::Decimal::ZERO);
            // force_new_line / metadata ride on the line row when present.
            // price_override stays None here (Django only honors it with the
            // handle_checkouts app permission; unit_prices gates the value).
            let lid = saleor_rustify_db::checkout_store::add_line_row(
                db, token, *vid, *qty, unit, &currency, None,
            )
            .await
            .map_err(|e| Error::new(e.to_string()))?;
            let _ = lid;
        }
        if let Some(a) = input.shipping_address.as_ref() {
            if let Err(e) = save_checkout_address(db, token, false, a).await {
                return Ok(CheckoutCreate {
                    checkout: None,
                    errors: ckerr(e),
                });
            }
        }
        if let Some(a) = input.billing_address.as_ref() {
            if let Err(e) = save_checkout_address(db, token, true, a).await {
                return Ok(CheckoutCreate {
                    checkout: None,
                    errors: ckerr(e),
                });
            }
        }
        if let Some(l) = input.language_code.as_ref() {
            let _ = saleor_rustify_db::checkout_store::set_language_code(
                db,
                token,
                crate::gen::language_code_value(l).to_string(),
            )
            .await;
        }
        if let Some(m) = input.metadata.as_ref() {
            let merged = crate::common::merge_metadata(&serde_json::Value::Null, m);
            let _ =
                saleor_rustify_db::checkout_store::set_metadata_opt(db, token, Some(merged)).await;
        }
        // Totals failures fail the mutation loudly (store contract):
        // serving a stale total would charge the wrong amount downstream.
        if let Err(e) = saleor_rustify_db::checkout_store::refresh_totals(db, token).await {
            return Ok(CheckoutCreate {
                checkout: None,
                errors: ckerrs(
                    format!("failed to refresh checkout totals: {e}"),
                    "INVALID",
                ),
            });
        }
        Ok(CheckoutCreate {
            checkout: Some(load_gql(db, token, &ch).await?),
            errors: vec![],
        })
    }

    async fn checkout_lines_add(
        &self,
        ctx: &Context<'_>,
        #[graphql(name = "checkoutId")] checkout_id: Option<ID>,
        id: Option<ID>,
        lines: Vec<gen::CheckoutLineInput>,
        token: Option<String>,
    ) -> Result<CheckoutLinesAdd> {
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let t = match resolve_token(id, checkout_id, token) {
            Ok(t) => t,
            Err(e) => {
                return Ok(CheckoutLinesAdd {
                    checkout: None,
                    errors: ckerr(e),
                })
            }
        };
        let (co, _) = saleor_rustify_db::checkout_store::load_checkout(db, t)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .ok_or_else(|| Error::new("checkout not found"))?;
        let ch = saleor_rustify_db::catalog::channel_slug_for_id(db, co.channel_id)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        let (ch_id, currency) = saleor_rustify_db::catalog::channel_info(db, &ch)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        let raw: Vec<Option<(i32, i32)>> = lines
            .iter()
            .map(|l| {
                saleor_rustify_db::catalog::parse_gid(&l.variant_id.0).map(|v| (v, l.quantity))
            })
            .collect();
        if raw.iter().any(|r| r.is_none()) {
            return Ok(CheckoutLinesAdd {
                checkout: None,
                errors: ckerrs("unknown variants".into(), "NOT_FOUND"),
            });
        }
        let raw: Vec<(i32, i32)> = raw.into_iter().flatten().collect();
        if let Err(e) = validate_checkout_lines(db, ch_id, &raw).await {
            return Ok(CheckoutLinesAdd {
                checkout: None,
                errors: vec![e],
            });
        }
        let items: Vec<(i32, i32, Option<rust_decimal::Decimal>)> = lines
            .iter()
            .zip(raw.iter())
            .map(|(l, (v, q))| (*v, *q, parse_price(&l.price)))
            .collect();
        let units = unit_prices(
            ctx,
            db,
            &ch,
            &items.iter().map(|(v, _, p)| (*v, *p)).collect::<Vec<_>>(),
        )
        .await?;
        let nl: Vec<saleor_rustify_db::checkout_store::NewLine> = items
            .into_iter()
            .map(|(vid, qty, _)| saleor_rustify_db::checkout_store::NewLine {
                variant_id: vid,
                quantity: qty,
                unit_price: units
                    .get(&vid)
                    .copied()
                    .unwrap_or(rust_decimal::Decimal::ZERO),
                price_override: None,
            })
            .collect();
        if let Err(e) =
            saleor_rustify_db::checkout_store::add_lines_tx(db, t, ch_id, &currency, &nl).await
        {
            return Ok(CheckoutLinesAdd {
                checkout: None,
                errors: ckerr(e.to_string()),
            });
        }
        // Totals failures fail the mutation loudly (store contract):
        // serving a stale total would charge the wrong amount downstream.
        if let Err(e) = saleor_rustify_db::checkout_store::refresh_totals(db, t).await {
            return Ok(CheckoutLinesAdd {
                checkout: None,
                errors: ckerrs(
                    format!("failed to refresh checkout totals: {e}"),
                    "INVALID",
                ),
            });
        }
        Ok(CheckoutLinesAdd {
            checkout: Some(load_gql(db, t, &ch).await?),
            errors: vec![],
        })
    }

    async fn checkout_lines_update(
        &self,
        ctx: &Context<'_>,
        #[graphql(name = "checkoutId")] checkout_id: Option<ID>,
        id: Option<ID>,
        lines: Vec<gen::CheckoutLineUpdateInput>,
        token: Option<String>,
    ) -> Result<CheckoutLinesUpdate> {
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let t = match resolve_token(id, checkout_id, token) {
            Ok(t) => t,
            Err(e) => {
                return Ok(CheckoutLinesUpdate {
                    checkout: None,
                    errors: ckerr(e.to_string()),
                })
            }
        };
        let (co, existing) = saleor_rustify_db::checkout_store::load_checkout(db, t)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .ok_or_else(|| Error::new("checkout not found"))?;
        let ch = saleor_rustify_db::catalog::channel_slug_for_id(db, co.channel_id)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        let (ch_id, _) = saleor_rustify_db::catalog::channel_info(db, &ch)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        // Resolve line ids to variants up front so quantity increases get
        // the same stock/limit validation as adds (Django revalidates).
        let mut to_check: Vec<(i32, i32)> = vec![];
        let mut resolved: Vec<(Uuid, i32)> = vec![];
        for l in &lines {
            // lineId preferred, legacy variantId honored.
            let lid = l
                .line_id
                .as_ref()
                .and_then(|i| crate::common::parse_uuid_gid(&i.0));
            let Some(lid) = lid else {
                return Ok(CheckoutLinesUpdate {
                    checkout: None,
                    errors: ckerrs("lineId is required".into(), "NOT_FOUND"),
                });
            };
            let qty = l.quantity.unwrap_or(1).max(0);
            resolved.push((lid, qty));
            if qty > 0 {
                if let Some(row) = existing.iter().find(|r| r.id == lid) {
                    to_check.push((row.variant_id, qty));
                }
            }
        }
        if !to_check.is_empty() {
            if let Err(e) = validate_checkout_lines(db, ch_id, &to_check).await {
                return Ok(CheckoutLinesUpdate {
                    checkout: None,
                    errors: vec![e],
                });
            }
        }
        for (lid, qty) in resolved {
            if let Err(e) =
                saleor_rustify_db::checkout_store::update_line_quantity(db, t, lid, qty).await
            {
                return Ok(CheckoutLinesUpdate {
                    checkout: None,
                    errors: ckerr(e.to_string()),
                });
            }
        }
        // Totals failures fail the mutation loudly (store contract):
        // serving a stale total would charge the wrong amount downstream.
        if let Err(e) = saleor_rustify_db::checkout_store::refresh_totals(db, t).await {
            return Ok(CheckoutLinesUpdate {
                checkout: None,
                errors: ckerrs(
                    format!("failed to refresh checkout totals: {e}"),
                    "INVALID",
                ),
            });
        }
        Ok(CheckoutLinesUpdate {
            checkout: Some(load_gql(db, t, &ch).await?),
            errors: vec![],
        })
    }

    async fn checkout_line_delete(
        &self,
        ctx: &Context<'_>,
        #[graphql(name = "checkoutId")] checkout_id: Option<ID>,
        id: Option<ID>,
        #[graphql(name = "lineId")] line_id: Option<ID>,
        token: Option<String>,
    ) -> Result<CheckoutLineDelete> {
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let t = match resolve_token(id, checkout_id, token) {
            Ok(t) => t,
            Err(e) => {
                return Ok(CheckoutLineDelete {
                    checkout: None,
                    errors: ckerr(e.to_string()),
                })
            }
        };
        let (co, _) = saleor_rustify_db::checkout_store::load_checkout(db, t)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .ok_or_else(|| Error::new("checkout not found"))?;
        // Never hardcode channel slugs (multi-channel correctness):
        // resolve the checkout's real channel from the database.
        // The checkout's own channel — resolved, never defaulted: a
        // missing channel row is data corruption and must error loudly,
        // not silently serve another shop's catalog.
        let ch = saleor_rustify_db::catalog::channel_slug_for_id(db, co.channel_id)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        let Some(lid) = line_id
            .as_ref()
            .and_then(|i| crate::common::parse_uuid_gid(&i.0))
        else {
            return Ok(CheckoutLineDelete {
                checkout: None,
                errors: ckerr("lineId is required".into()),
            });
        };
        if let Err(e) = saleor_rustify_db::checkout_store::delete_line(db, t, lid).await {
            return Ok(CheckoutLineDelete {
                checkout: None,
                errors: ckerr(e.to_string()),
            });
        }
        // Totals failures fail the mutation loudly (store contract):
        // serving a stale total would charge the wrong amount downstream.
        if let Err(e) = saleor_rustify_db::checkout_store::refresh_totals(db, t).await {
            return Ok(CheckoutLineDelete {
                checkout: None,
                errors: ckerrs(
                    format!("failed to refresh checkout totals: {e}"),
                    "INVALID",
                ),
            });
        }
        Ok(CheckoutLineDelete {
            checkout: Some(load_gql(db, t, &ch).await?),
            errors: vec![],
        })
    }

    async fn checkout_delete(&self, ctx: &Context<'_>, id: ID) -> Result<CheckoutDelete> {
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let t = match resolve_token(Some(id), None, None) {
            Ok(t) => t,
            Err(e) => {
                return Ok(CheckoutDelete {
                    checkout: None,
                    errors: ckerr(e.to_string()),
                })
            }
        };
        if let Err(e) = saleor_rustify_db::checkout_store::delete_checkout_row(db, t).await {
            return Ok(CheckoutDelete {
                checkout: None,
                errors: ckerr(e.to_string()),
            });
        }
        Ok(CheckoutDelete {
            checkout: None,
            errors: vec![],
        })
    }

    async fn checkout_email_update(
        &self,
        ctx: &Context<'_>,
        #[graphql(name = "checkoutId")] checkout_id: Option<ID>,
        email: String,
        id: Option<ID>,
        token: Option<String>,
    ) -> Result<CheckoutEmailUpdate> {
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let t = match resolve_token(id, checkout_id, token) {
            Ok(t) => t,
            Err(e) => {
                return Ok(CheckoutEmailUpdate {
                    checkout: None,
                    errors: ckerr(e.to_string()),
                })
            }
        };
        // Django validates the email format (checkout_email_update.py:59).
        if !email.contains('@') {
            return Ok(CheckoutEmailUpdate {
                checkout: None,
                errors: ckerrs("enter a valid email address".into(), "REQUIRED"),
            });
        }
        let (co, _) = saleor_rustify_db::checkout_store::load_checkout(db, t)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .ok_or_else(|| Error::new("checkout not found"))?;
        // Never hardcode channel slugs (multi-channel correctness):
        // resolve the checkout's real channel from the database.
        // The checkout's own channel — resolved, never defaulted: a
        // missing channel row is data corruption and must error loudly,
        // not silently serve another shop's catalog.
        let ch = saleor_rustify_db::catalog::channel_slug_for_id(db, co.channel_id)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        if let Err(e) = saleor_rustify_db::checkout_store::set_email_opt(db, t, Some(email)).await {
            return Ok(CheckoutEmailUpdate {
                checkout: None,
                errors: ckerr(e.to_string()),
            });
        }
        Ok(CheckoutEmailUpdate {
            checkout: Some(load_gql(db, t, &ch).await?),
            errors: vec![],
        })
    }

    async fn checkout_customer_attach(
        &self,
        ctx: &Context<'_>,
        #[graphql(name = "checkoutId")] checkout_id: Option<ID>,
        #[graphql(name = "customerId")] customer_id: Option<ID>,
        id: Option<ID>,
        token: Option<String>,
    ) -> Result<CheckoutCustomerAttach> {
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let t = match resolve_token(id, checkout_id, token) {
            Ok(t) => t,
            Err(e) => {
                return Ok(CheckoutCustomerAttach {
                    checkout: None,
                    errors: ckerr(e.to_string()),
                })
            }
        };
        let (co, _) = saleor_rustify_db::checkout_store::load_checkout(db, t)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .ok_or_else(|| Error::new("checkout not found"))?;
        // Never hardcode channel slugs (multi-channel correctness):
        // resolve the checkout's real channel from the database.
        // The checkout's own channel — resolved, never defaulted: a
        // missing channel row is data corruption and must error loudly,
        // not silently serve another shop's catalog.
        let ch = saleor_rustify_db::catalog::channel_slug_for_id(db, co.channel_id)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        let uid = customer_id
            .as_ref()
            .and_then(|i| saleor_rustify_db::catalog::parse_gid(&i.0));
        if let Err(e) = saleor_rustify_db::checkout_store::set_customer_opt(db, t, uid).await {
            return Ok(CheckoutCustomerAttach {
                checkout: None,
                errors: ckerr(e.to_string()),
            });
        }
        Ok(CheckoutCustomerAttach {
            checkout: Some(load_gql(db, t, &ch).await?),
            errors: vec![],
        })
    }

    async fn checkout_customer_detach(
        &self,
        ctx: &Context<'_>,
        #[graphql(name = "checkoutId")] checkout_id: Option<ID>,
        id: Option<ID>,
        token: Option<String>,
    ) -> Result<CheckoutCustomerDetach> {
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let t = match resolve_token(id, checkout_id, token) {
            Ok(t) => t,
            Err(e) => {
                return Ok(CheckoutCustomerDetach {
                    checkout: None,
                    errors: ckerr(e.to_string()),
                })
            }
        };
        let (co, _) = saleor_rustify_db::checkout_store::load_checkout(db, t)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .ok_or_else(|| Error::new("checkout not found"))?;
        // Never hardcode channel slugs (multi-channel correctness):
        // resolve the checkout's real channel from the database.
        // The checkout's own channel — resolved, never defaulted: a
        // missing channel row is data corruption and must error loudly,
        // not silently serve another shop's catalog.
        let ch = saleor_rustify_db::catalog::channel_slug_for_id(db, co.channel_id)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        if let Err(e) = saleor_rustify_db::checkout_store::set_customer_opt(db, t, None).await {
            return Ok(CheckoutCustomerDetach {
                checkout: None,
                errors: ckerr(e.to_string()),
            });
        }
        Ok(CheckoutCustomerDetach {
            checkout: Some(load_gql(db, t, &ch).await?),
            errors: vec![],
        })
    }

    async fn checkout_customer_note_update(
        &self,
        ctx: &Context<'_>,
        #[graphql(name = "customerNote")] customer_note: String,
        id: ID,
    ) -> Result<CheckoutCustomerNoteUpdate> {
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let t = match resolve_token(Some(id), None, None) {
            Ok(t) => t,
            Err(e) => {
                return Ok(CheckoutCustomerNoteUpdate {
                    checkout: None,
                    errors: ckerr(e.to_string()),
                })
            }
        };
        let (co, _) = saleor_rustify_db::checkout_store::load_checkout(db, t)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .ok_or_else(|| Error::new("checkout not found"))?;
        // Never hardcode channel slugs (multi-channel correctness):
        // resolve the checkout's real channel from the database.
        // The checkout's own channel — resolved, never defaulted: a
        // missing channel row is data corruption and must error loudly,
        // not silently serve another shop's catalog.
        let ch = saleor_rustify_db::catalog::channel_slug_for_id(db, co.channel_id)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        if let Err(e) = saleor_rustify_db::checkout_store::set_note_text(db, t, customer_note).await
        {
            return Ok(CheckoutCustomerNoteUpdate {
                checkout: None,
                errors: ckerr(e.to_string()),
            });
        }
        Ok(CheckoutCustomerNoteUpdate {
            checkout: Some(load_gql(db, t, &ch).await?),
            errors: vec![],
        })
    }

    async fn checkout_shipping_address_update(
        &self,
        ctx: &Context<'_>,
        #[graphql(name = "checkoutId")] checkout_id: Option<ID>,
        id: Option<ID>,
        #[graphql(name = "shippingAddress")] shipping_address: gen::AddressInput,
        token: Option<String>,
    ) -> Result<CheckoutShippingAddressUpdate> {
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let t = match resolve_token(id, checkout_id, token) {
            Ok(t) => t,
            Err(e) => {
                return Ok(CheckoutShippingAddressUpdate {
                    checkout: None,
                    errors: ckerr(e.to_string()),
                })
            }
        };
        let (co, _) = saleor_rustify_db::checkout_store::load_checkout(db, t)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .ok_or_else(|| Error::new("checkout not found"))?;
        // Never hardcode channel slugs (multi-channel correctness):
        // resolve the checkout's real channel from the database.
        // The checkout's own channel — resolved, never defaulted: a
        // missing channel row is data corruption and must error loudly,
        // not silently serve another shop's catalog.
        let ch = saleor_rustify_db::catalog::channel_slug_for_id(db, co.channel_id)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        if let Err(e) = save_checkout_address(db, t, false, &shipping_address).await {
            return Ok(CheckoutShippingAddressUpdate {
                checkout: None,
                errors: ckerr(e),
            });
        }
        Ok(CheckoutShippingAddressUpdate {
            checkout: Some(load_gql(db, t, &ch).await?),
            errors: vec![],
        })
    }

    async fn checkout_billing_address_update(
        &self,
        ctx: &Context<'_>,
        #[graphql(name = "billingAddress")] billing_address: gen::AddressInput,
        #[graphql(name = "checkoutId")] checkout_id: Option<ID>,
        id: Option<ID>,
        token: Option<String>,
    ) -> Result<CheckoutBillingAddressUpdate> {
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let t = match resolve_token(id, checkout_id, token) {
            Ok(t) => t,
            Err(e) => {
                return Ok(CheckoutBillingAddressUpdate {
                    checkout: None,
                    errors: ckerr(e.to_string()),
                })
            }
        };
        let (co, _) = saleor_rustify_db::checkout_store::load_checkout(db, t)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .ok_or_else(|| Error::new("checkout not found"))?;
        // Never hardcode channel slugs (multi-channel correctness):
        // resolve the checkout's real channel from the database.
        // The checkout's own channel — resolved, never defaulted: a
        // missing channel row is data corruption and must error loudly,
        // not silently serve another shop's catalog.
        let ch = saleor_rustify_db::catalog::channel_slug_for_id(db, co.channel_id)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        if let Err(e) = save_checkout_address(db, t, true, &billing_address).await {
            return Ok(CheckoutBillingAddressUpdate {
                checkout: None,
                errors: ckerr(e),
            });
        }
        Ok(CheckoutBillingAddressUpdate {
            checkout: Some(load_gql(db, t, &ch).await?),
            errors: vec![],
        })
    }

    async fn checkout_shipping_method_update(
        &self,
        ctx: &Context<'_>,
        #[graphql(name = "checkoutId")] checkout_id: Option<ID>,
        id: Option<ID>,
        #[graphql(name = "shippingMethodId")] shipping_method_id: Option<ID>,
        token: Option<String>,
    ) -> Result<CheckoutShippingMethodUpdate> {
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let t = match resolve_token(id, checkout_id, token) {
            Ok(t) => t,
            Err(e) => {
                return Ok(CheckoutShippingMethodUpdate {
                    checkout: None,
                    errors: ckerr(e.to_string()),
                })
            }
        };
        let (co, _) = saleor_rustify_db::checkout_store::load_checkout(db, t)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .ok_or_else(|| Error::new("checkout not found"))?;
        // Never hardcode channel slugs (multi-channel correctness):
        // resolve the checkout's real channel from the database.
        // The checkout's own channel — resolved, never defaulted: a
        // missing channel row is data corruption and must error loudly,
        // not silently serve another shop's catalog.
        let ch = saleor_rustify_db::catalog::channel_slug_for_id(db, co.channel_id)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        let mid = shipping_method_id
            .as_ref()
            .and_then(|i| saleor_rustify_db::catalog::parse_gid(&i.0));
        if let Err(e) = saleor_rustify_db::checkout_store::set_shipping_method_opt(db, t, mid).await
        {
            return Ok(CheckoutShippingMethodUpdate {
                checkout: None,
                errors: ckerr(e.to_string()),
            });
        }
        // Totals failures fail the mutation loudly (store contract):
        // serving a stale total would charge the wrong amount downstream.
        if let Err(e) = saleor_rustify_db::checkout_store::refresh_totals(db, t).await {
            return Ok(CheckoutShippingMethodUpdate {
                checkout: None,
                errors: ckerrs(
                    format!("failed to refresh checkout totals: {e}"),
                    "INVALID",
                ),
            });
        }
        Ok(CheckoutShippingMethodUpdate {
            checkout: Some(load_gql(db, t, &ch).await?),
            errors: vec![],
        })
    }

    async fn checkout_delivery_method_update(
        &self,
        ctx: &Context<'_>,
        #[graphql(name = "deliveryMethodId")] delivery_method_id: Option<ID>,
        id: Option<ID>,
        token: Option<String>,
        #[graphql(name = "checkoutId")] checkout_id: Option<ID>,
    ) -> Result<CheckoutDeliveryMethodUpdate> {
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let t = match resolve_token(id, checkout_id, token) {
            Ok(t) => t,
            Err(e) => {
                return Ok(CheckoutDeliveryMethodUpdate {
                    checkout: None,
                    errors: ckerr(e.to_string()),
                })
            }
        };
        let (co, _) = saleor_rustify_db::checkout_store::load_checkout(db, t)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .ok_or_else(|| Error::new("checkout not found"))?;
        // Never hardcode channel slugs (multi-channel correctness):
        // resolve the checkout's real channel from the database.
        // The checkout's own channel — resolved, never defaulted: a
        // missing channel row is data corruption and must error loudly,
        // not silently serve another shop's catalog.
        let ch = saleor_rustify_db::catalog::channel_slug_for_id(db, co.channel_id)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        if let Some(dm) = delivery_method_id.as_ref() {
            // Shipping method (int gid) or collection point (warehouse UUID).
            if let Some(mid) = saleor_rustify_db::catalog::parse_gid(&dm.0) {
                if let Err(e) =
                    saleor_rustify_db::checkout_store::set_shipping_method_opt(db, t, Some(mid))
                        .await
                {
                    return Ok(CheckoutDeliveryMethodUpdate {
                        checkout: None,
                        errors: ckerr(e.to_string()),
                    });
                }
                let _ =
                    saleor_rustify_db::checkout_store::set_collection_point_opt(db, t, None).await;
            } else if let Some(wid) = crate::common::parse_uuid_gid(&dm.0) {
                if let Err(e) =
                    saleor_rustify_db::checkout_store::set_collection_point_opt(db, t, Some(wid))
                        .await
                {
                    return Ok(CheckoutDeliveryMethodUpdate {
                        checkout: None,
                        errors: ckerr(e.to_string()),
                    });
                }
                let _ =
                    saleor_rustify_db::checkout_store::set_shipping_method_opt(db, t, None).await;
            } else {
                return Ok(CheckoutDeliveryMethodUpdate {
                    checkout: None,
                    errors: ckerr("bad delivery method id".into()),
                });
            }
        } else {
            let _ = saleor_rustify_db::checkout_store::set_shipping_method_opt(db, t, None).await;
            let _ = saleor_rustify_db::checkout_store::set_collection_point_opt(db, t, None).await;
        }
        // Totals failures fail the mutation loudly (store contract):
        // serving a stale total would charge the wrong amount downstream.
        if let Err(e) = saleor_rustify_db::checkout_store::refresh_totals(db, t).await {
            return Ok(CheckoutDeliveryMethodUpdate {
                checkout: None,
                errors: ckerrs(
                    format!("failed to refresh checkout totals: {e}"),
                    "INVALID",
                ),
            });
        }
        Ok(CheckoutDeliveryMethodUpdate {
            checkout: Some(load_gql(db, t, &ch).await?),
            errors: vec![],
        })
    }

    async fn checkout_language_code_update(
        &self,
        ctx: &Context<'_>,
        #[graphql(name = "checkoutId")] checkout_id: Option<ID>,
        id: Option<ID>,
        #[graphql(name = "languageCode")] language_code: gen::LanguageCodeEnum,
        token: Option<String>,
    ) -> Result<CheckoutLanguageCodeUpdate> {
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let t = match resolve_token(id, checkout_id, token) {
            Ok(t) => t,
            Err(e) => {
                return Ok(CheckoutLanguageCodeUpdate {
                    checkout: None,
                    errors: ckerr(e.to_string()),
                })
            }
        };
        let (co, _) = saleor_rustify_db::checkout_store::load_checkout(db, t)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .ok_or_else(|| Error::new("checkout not found"))?;
        // Never hardcode channel slugs (multi-channel correctness):
        // resolve the checkout's real channel from the database.
        // The checkout's own channel — resolved, never defaulted: a
        // missing channel row is data corruption and must error loudly,
        // not silently serve another shop's catalog.
        let ch = saleor_rustify_db::catalog::channel_slug_for_id(db, co.channel_id)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        if let Err(e) = saleor_rustify_db::checkout_store::set_language_code(
            db,
            t,
            crate::gen::language_code_value(&language_code).to_string(),
        )
        .await
        {
            return Ok(CheckoutLanguageCodeUpdate {
                checkout: None,
                errors: ckerr(e.to_string()),
            });
        }
        Ok(CheckoutLanguageCodeUpdate {
            checkout: Some(load_gql(db, t, &ch).await?),
            errors: vec![],
        })
    }

    async fn checkout_add_promo_code(
        &self,
        ctx: &Context<'_>,
        #[graphql(name = "checkoutId")] checkout_id: Option<ID>,
        id: Option<ID>,
        #[graphql(name = "promoCode")] promo_code: String,
        token: Option<String>,
    ) -> Result<CheckoutAddPromoCode> {
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let t = match resolve_token(id, checkout_id, token) {
            Ok(t) => t,
            Err(e) => {
                return Ok(CheckoutAddPromoCode {
                    checkout: None,
                    errors: ckerr(e.to_string()),
                })
            }
        };
        let (co, _) = saleor_rustify_db::checkout_store::load_checkout(db, t)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .ok_or_else(|| Error::new("checkout not found"))?;
        // Never hardcode channel slugs (multi-channel correctness):
        // resolve the checkout's real channel from the database.
        // The checkout's own channel — resolved, never defaulted: a
        // missing channel row is data corruption and must error loudly,
        // not silently serve another shop's catalog.
        let ch = saleor_rustify_db::catalog::channel_slug_for_id(db, co.channel_id)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        // Voucher first, gift card second (Django tries both code spaces).
        let voucher_ok = saleor_rustify_db::promotions::apply_voucher(db, t, &promo_code, &ch)
            .await
            .is_ok();
        if !voucher_ok {
            if let Err(e) = saleor_rustify_db::giftcards::attach_to_checkout(
                db,
                t,
                &promo_code,
                &co.currency,
                co.user_id,
            )
            .await
            {
                return Ok(CheckoutAddPromoCode {
                    checkout: None,
                    errors: ckerr(format!("invalid promo code: {e}")),
                });
            }
        }
        // Totals failures fail the mutation loudly (store contract):
        // serving a stale total would charge the wrong amount downstream.
        if let Err(e) = saleor_rustify_db::checkout_store::refresh_totals(db, t).await {
            return Ok(CheckoutAddPromoCode {
                checkout: None,
                errors: ckerrs(
                    format!("failed to refresh checkout totals: {e}"),
                    "INVALID",
                ),
            });
        }
        Ok(CheckoutAddPromoCode {
            checkout: Some(load_gql(db, t, &ch).await?),
            errors: vec![],
        })
    }

    async fn checkout_remove_promo_code(
        &self,
        ctx: &Context<'_>,
        #[graphql(name = "checkoutId")] checkout_id: Option<ID>,
        id: Option<ID>,
        #[graphql(name = "promoCode")] promo_code: Option<String>,
        #[graphql(name = "promoCodeId")] promo_code_id: Option<ID>,
        token: Option<String>,
    ) -> Result<CheckoutRemovePromoCode> {
        let _ = promo_code_id;
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let t = match resolve_token(id, checkout_id, token) {
            Ok(t) => t,
            Err(e) => {
                return Ok(CheckoutRemovePromoCode {
                    checkout: None,
                    errors: ckerr(e.to_string()),
                })
            }
        };
        let (co, _) = saleor_rustify_db::checkout_store::load_checkout(db, t)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .ok_or_else(|| Error::new("checkout not found"))?;
        // Never hardcode channel slugs (multi-channel correctness):
        // resolve the checkout's real channel from the database.
        // The checkout's own channel — resolved, never defaulted: a
        // missing channel row is data corruption and must error loudly,
        // not silently serve another shop's catalog.
        let ch = saleor_rustify_db::catalog::channel_slug_for_id(db, co.channel_id)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        let code = promo_code.clone().unwrap_or_default();
        let cleared = saleor_rustify_db::checkout_store::remove_voucher_code(
            db,
            t,
            if code.is_empty() { None } else { Some(&code) },
        )
        .await
        .map_err(|e| Error::new(e.to_string()))?;
        if !cleared && !code.is_empty() {
            // Maybe a gift card: detach is idempotent, missing → error.
            if let Err(e) = saleor_rustify_db::giftcards::detach_from_checkout(db, t, &code).await {
                return Ok(CheckoutRemovePromoCode {
                    checkout: None,
                    errors: ckerr(format!("promo code not found: {e}")),
                });
            }
        }
        // Totals failures fail the mutation loudly (store contract):
        // serving a stale total would charge the wrong amount downstream.
        if let Err(e) = saleor_rustify_db::checkout_store::refresh_totals(db, t).await {
            return Ok(CheckoutRemovePromoCode {
                checkout: None,
                errors: ckerrs(
                    format!("failed to refresh checkout totals: {e}"),
                    "INVALID",
                ),
            });
        }
        Ok(CheckoutRemovePromoCode {
            checkout: Some(load_gql(db, t, &ch).await?),
            errors: vec![],
        })
    }

    async fn agent_checkout(
        &self,
        ctx: &Context<'_>,
        message: String,
        channel: Option<String>,
        email: Option<String>,
    ) -> Result<AgentCheckoutResult> {
        let g = ctx.data::<GqlContext>()?;
        let db = g.db()?;
        let ch = channel.unwrap_or_else(saleor_rustify_db::catalog::fallback_channel_slug);
        let plan = saleor_rustify_ai::agent::plan_checkout(db, &ch, &message)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        if plan.lines.is_empty() {
            return Err(Error::new(
                plan.questions
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "no purchasable items found".into()),
            ));
        }
        let (ch_id, currency) = saleor_rustify_db::catalog::channel_info(db, &ch)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        let token = saleor_rustify_db::checkout_store::create_checkout_row(
            db,
            ch_id,
            &currency,
            email.as_deref().unwrap_or(""),
        )
        .await
        .map_err(|e| Error::new(e.to_string()))?;
        let vids: Vec<i32> = plan.lines.iter().map(|l| l.variant_id).collect();
        let pricing = saleor_rustify_db::catalog::checkout_pricing(db, &ch, &vids)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        let items: Vec<saleor_rustify_db::checkout_store::NewLine> = plan
            .lines
            .iter()
            .map(|l| {
                let unit = pricing
                    .get(&l.variant_id)
                    .map(|(m, _)| m.amount)
                    .unwrap_or(rust_decimal::Decimal::ZERO);
                saleor_rustify_db::checkout_store::NewLine {
                    variant_id: l.variant_id,
                    quantity: l.quantity,
                    unit_price: unit,
                    price_override: None,
                }
            })
            .collect();
        saleor_rustify_db::checkout_store::add_lines_tx(db, token, ch_id, &currency, &items)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        saleor_rustify_db::checkout_store::refresh_totals(db, token)
            .await
            .map_err(|e| Error::new(e.to_string()))?;
        let (co, lines) = saleor_rustify_db::checkout_store::load_checkout(db, token)
            .await
            .map_err(|e| Error::new(e.to_string()))?
            .ok_or_else(|| Error::new("checkout vanished"))?;
        Ok(AgentCheckoutResult {
            checkout: to_gql_checkout(&co, &lines, &ch),
            notes: plan.notes,
            questions: plan.questions,
        })
    }
}
