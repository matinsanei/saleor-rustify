use std::collections::HashMap;
use chrono::Utc;
use rust_decimal::Decimal;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, Set, TransactionTrait,
};
use uuid::Uuid;

use crate::marketplace::entities::{
    marketplace_cart::{self, Entity as MarketplaceCart},
    marketplace_cart_item::{self, Entity as MarketplaceCartItem},
    marketplace_order::{self, ActiveModel as OrderActive},
    order_item::{self, ActiveModel as OrderItemActive, Entity as OrderItem},
    product::{self, Entity as Product},
    product_variant::{self, ActiveModel as VariantActive, Entity as ProductVariant},
    vendor::{self, Entity as Vendor},
    vendor_sub_order::{self, ActiveModel as SubOrderActive, Entity as VendorSubOrder},
};
use crate::{DbError, Result};
use saleor_rustify_core::marketplace::{calculate_order_splitting, CartItemInput};

pub struct MarketplaceOrderService<'a> {
    db: &'a DatabaseConnection,
}

pub struct CheckoutResult {
    pub parent_order: marketplace_order::Model,
    pub sub_orders: Vec<(vendor_sub_order::Model, Vec<order_item::Model>)>,
}

impl<'a> MarketplaceOrderService<'a> {
    pub fn new(db: &'a DatabaseConnection) -> Self {
        Self { db }
    }

    /// Checkout buyer cart with atomic transaction:
    /// 1. Validates stock and prices
    /// 2. Decrements stock quantities atomically
    /// 3. Computes vendor commissions and automatic order splitting
    /// 4. Creates parent MarketplaceOrder and child VendorSubOrders with OrderItems
    /// 5. Clears the cart items
    pub async fn checkout_cart_tx(&self, buyer_id: Uuid) -> Result<CheckoutResult> {
        let txn = self.db.begin().await?;

        // 1. Find buyer cart
        let cart = MarketplaceCart::find()
            .filter(marketplace_cart::Column::UserId.eq(buyer_id))
            .one(&txn)
            .await?
            .ok_or_else(|| DbError::Catalog("buyer does not have an active cart".into()))?;

        let cart_items = MarketplaceCartItem::find()
            .filter(marketplace_cart_item::Column::CartId.eq(cart.id))
            .all(&txn)
            .await?;

        if cart_items.is_empty() {
            return Err(DbError::Catalog("cart is empty; cannot proceed to checkout".into()));
        }

        // 2. Fetch variants & products for validation
        let variant_ids: Vec<Uuid> = cart_items.iter().map(|i| i.product_variant_id).collect();
        let variants = ProductVariant::find()
            .filter(product_variant::Column::Id.is_in(variant_ids))
            .all(&txn)
            .await?;

        let variant_map: HashMap<Uuid, product_variant::Model> =
            variants.into_iter().map(|v| (v.id, v)).collect();

        let product_ids: Vec<Uuid> = variant_map.values().map(|v| v.product_id).collect();
        let products = Product::find()
            .filter(product::Column::Id.is_in(product_ids))
            .all(&txn)
            .await?;

        let product_map: HashMap<Uuid, product::Model> =
            products.into_iter().map(|p| (p.id, p)).collect();

        // 3. Check stock & prepare inputs
        let mut cart_item_inputs = Vec::new();
        for item in &cart_items {
            let variant = variant_map.get(&item.product_variant_id).ok_or_else(|| {
                DbError::Catalog(format!("variant {} not found", item.product_variant_id))
            })?;

            if variant.stock_quantity < item.quantity {
                return Err(DbError::Catalog(format!(
                    "insufficient stock for variant '{}' (requested: {}, available: {})",
                    variant.sku, item.quantity, variant.stock_quantity
                )));
            }

            let prod = product_map.get(&variant.product_id).ok_or_else(|| {
                DbError::Catalog(format!("product {} not found", variant.product_id))
            })?;

            // Decrement variant stock
            let mut v_active: VariantActive = variant.clone().into();
            v_active.stock_quantity = Set(variant.stock_quantity - item.quantity);
            v_active.updated_at = Set(Utc::now().into());
            v_active.update(&txn).await?;

            cart_item_inputs.push(CartItemInput {
                product_variant_id: variant.id,
                vendor_id: item.vendor_id,
                quantity: item.quantity,
                unit_price_amount: variant.price_amount,
                currency: variant.currency.clone(),
                product_title: prod.title.clone(),
                variant_name: variant.name.clone(),
                sku: variant.sku.clone(),
            });
        }

        // 4. Fetch vendor commission rates
        let vendor_ids: Vec<Uuid> = cart_items.iter().map(|i| i.vendor_id).collect();
        let vendors = Vendor::find()
            .filter(vendor::Column::Id.is_in(vendor_ids))
            .all(&txn)
            .await?;

        let mut commission_map = HashMap::new();
        for v in vendors {
            commission_map.insert(v.id, v.commission_rate);
        }

        // 5. Calculate order splitting
        let split_calc = calculate_order_splitting(buyer_id, cart_item_inputs, &commission_map)
            .map_err(|e| DbError::Catalog(format!("order splitting calculation error: {e}")))?;

        // 6. Insert parent MarketplaceOrder
        let parent_order_id = Uuid::new_v4();
        let now = Utc::now().into();

        let parent_active = OrderActive {
            id: Set(parent_order_id),
            buyer_id: Set(buyer_id),
            total_gross_amount: Set(split_calc.total_gross_amount),
            payment_status: Set("PAID".to_string()),
            created_at: Set(now),
        };
        let parent_order = parent_active.insert(&txn).await?;

        // 7. Insert child VendorSubOrders and OrderItems
        let mut sub_order_results = Vec::new();
        for sub_calc in split_calc.sub_orders {
            let sub_order_id = Uuid::new_v4();
            let sub_active = SubOrderActive {
                id: Set(sub_order_id),
                parent_order_id: Set(parent_order_id),
                vendor_id: Set(sub_calc.vendor_id),
                subtotal_amount: Set(sub_calc.subtotal_amount),
                commission_amount: Set(sub_calc.commission_amount),
                vendor_payout_amount: Set(sub_calc.vendor_payout_amount),
                fulfillment_status: Set("PENDING".to_string()),
                tracking_number: Set(None),
                created_at: Set(now),
            };
            let sub_order = sub_active.insert(&txn).await?;

            let mut order_items = Vec::new();
            for item in sub_calc.items {
                let total_item_price = item.unit_price_amount * Decimal::from(item.quantity);
                let item_active = OrderItemActive {
                    id: Set(Uuid::new_v4()),
                    sub_order_id: Set(sub_order_id),
                    product_variant_id: Set(item.product_variant_id),
                    product_title: Set(item.product_title),
                    variant_name: Set(item.variant_name),
                    sku: Set(item.sku),
                    quantity: Set(item.quantity),
                    unit_price_amount: Set(item.unit_price_amount),
                    total_price_amount: Set(total_item_price),
                    currency: Set(item.currency),
                };
                let created_item = item_active.insert(&txn).await?;
                order_items.push(created_item);
            }

            sub_order_results.push((sub_order, order_items));
        }

        // 8. Clear cart items
        MarketplaceCartItem::delete_many()
            .filter(marketplace_cart_item::Column::CartId.eq(cart.id))
            .exec(&txn)
            .await?;

        txn.commit().await?;

        Ok(CheckoutResult {
            parent_order,
            sub_orders: sub_order_results,
        })
    }

    /// Update fulfillment status and tracking number for a vendor's sub-order.
    /// Strictly verifies that the sub-order belongs to the specified vendor.
    pub async fn vendor_update_fulfillment_tx(
        &self,
        vendor_id: Uuid,
        sub_order_id: Uuid,
        status: String,
        tracking_number: Option<String>,
    ) -> Result<vendor_sub_order::Model> {
        let clean_status = status.to_uppercase();
        let allowed = ["PENDING", "PROCESSING", "SHIPPED", "DELIVERED", "CANCELLED"];
        if !allowed.contains(&clean_status.as_str()) {
            return Err(DbError::Catalog(format!(
                "invalid fulfillment status '{status}'; allowed: PENDING, PROCESSING, SHIPPED, DELIVERED, CANCELLED"
            )));
        }

        let txn = self.db.begin().await?;

        let sub_order = VendorSubOrder::find_by_id(sub_order_id)
            .one(&txn)
            .await?
            .ok_or_else(|| DbError::Catalog(format!("sub order {sub_order_id} not found")))?;

        if sub_order.vendor_id != vendor_id {
            return Err(DbError::Catalog(
                "unauthorized: this sub-order does not belong to your vendor account".into(),
            ));
        }

        let mut active: SubOrderActive = sub_order.into();
        active.fulfillment_status = Set(clean_status);
        if let Some(track) = tracking_number {
            active.tracking_number = Set(Some(track));
        }

        let updated = active.update(&txn).await?;
        txn.commit().await?;

        Ok(updated)
    }

    /// List sub-orders for a vendor with optional fulfillment status filter and pagination
    pub async fn list_vendor_sub_orders(
        &self,
        vendor_id: Uuid,
        status: Option<String>,
        page: Option<i32>,
        page_size: Option<i32>,
    ) -> Result<(Vec<vendor_sub_order::Model>, u64)> {
        let p = page.unwrap_or(1).max(1) as u64;
        let ps = page_size.unwrap_or(20).clamp(1, 100) as u64;

        let mut query = VendorSubOrder::find()
            .filter(vendor_sub_order::Column::VendorId.eq(vendor_id))
            .order_by_desc(vendor_sub_order::Column::CreatedAt);

        if let Some(s) = status {
            query = query.filter(vendor_sub_order::Column::FulfillmentStatus.eq(s.to_uppercase()));
        }

        let paginator = query.paginate(self.db, ps);
        let total = paginator.num_items().await?;
        let items = paginator.fetch_page(p - 1).await?;

        Ok((items, total))
    }

    pub async fn get_sub_order_items(
        &self,
        sub_order_id: Uuid,
    ) -> Result<Vec<order_item::Model>> {
        let items = OrderItem::find()
            .filter(order_item::Column::SubOrderId.eq(sub_order_id))
            .all(self.db)
            .await?;
        Ok(items)
    }
}
