use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait,
    QueryFilter, Set, TransactionTrait,
};
use uuid::Uuid;

use crate::marketplace::entities::{
    marketplace_cart::{self, ActiveModel as CartActive, Entity as MarketplaceCart},
    marketplace_cart_item::{self, ActiveModel as CartItemActive, Entity as MarketplaceCartItem},
    product::Entity as Product,
    product_variant::Entity as ProductVariant,
};
use crate::{DbError, Result};

pub struct CartService<'a> {
    db: &'a DatabaseConnection,
}

impl<'a> CartService<'a> {
    pub fn new(db: &'a DatabaseConnection) -> Self {
        Self { db }
    }

    /// Retrieve or initialize a persistent unified cart for a platform user
    pub async fn get_or_create_cart(&self, user_id: Uuid) -> Result<marketplace_cart::Model> {
        let existing = MarketplaceCart::find()
            .filter(marketplace_cart::Column::UserId.eq(user_id))
            .one(self.db)
            .await?;

        if let Some(cart) = existing {
            return Ok(cart);
        }

        let now = Utc::now().into();
        let new_cart = CartActive {
            id: Set(Uuid::new_v4()),
            user_id: Set(user_id),
            created_at: Set(now),
            updated_at: Set(now),
        };

        let inserted = new_cart.insert(self.db).await?;
        Ok(inserted)
    }

    /// Add a product variant to the unified cart. Retains vendor_id for automatic splitting.
    pub async fn add_item(
        &self,
        user_id: Uuid,
        product_variant_id: Uuid,
        quantity: i32,
    ) -> Result<marketplace_cart_item::Model> {
        if quantity <= 0 {
            return Err(DbError::Catalog("item quantity must be greater than zero".into()));
        }

        let txn = self.db.begin().await?;

        // 1. Get or create cart
        let cart = match MarketplaceCart::find()
            .filter(marketplace_cart::Column::UserId.eq(user_id))
            .one(&txn)
            .await?
        {
            Some(c) => c,
            None => {
                let now = Utc::now().into();
                let active = CartActive {
                    id: Set(Uuid::new_v4()),
                    user_id: Set(user_id),
                    created_at: Set(now),
                    updated_at: Set(now),
                };
                active.insert(&txn).await?
            }
        };

        // 2. Fetch variant and its product to obtain vendor_id and pricing
        let variant = ProductVariant::find_by_id(product_variant_id)
            .one(&txn)
            .await?
            .ok_or_else(|| DbError::Catalog(format!("variant {product_variant_id} not found")))?;

        let product = Product::find_by_id(variant.product_id)
            .one(&txn)
            .await?
            .ok_or_else(|| DbError::Catalog(format!("product {} not found", variant.product_id)))?;

        // 3. Upsert cart item
        let existing_item = MarketplaceCartItem::find()
            .filter(marketplace_cart_item::Column::CartId.eq(cart.id))
            .filter(marketplace_cart_item::Column::ProductVariantId.eq(product_variant_id))
            .one(&txn)
            .await?;

        let now = Utc::now().into();
        let saved_item = match existing_item {
            Some(item) => {
                let mut active: CartItemActive = item.clone().into();
                let new_qty = item.quantity + quantity;
                active.quantity = Set(new_qty);
                active.price_amount = Set(variant.price_amount);
                active.update(&txn).await?
            }
            None => {
                let active = CartItemActive {
                    id: Set(Uuid::new_v4()),
                    cart_id: Set(cart.id),
                    product_variant_id: Set(product_variant_id),
                    vendor_id: Set(product.vendor_id),
                    quantity: Set(quantity),
                    price_amount: Set(variant.price_amount),
                    currency: Set(variant.currency),
                    created_at: Set(now),
                };
                active.insert(&txn).await?
            }
        };

        // Update cart's updated_at
        let mut cart_active: CartActive = cart.into();
        cart_active.updated_at = Set(now);
        cart_active.update(&txn).await?;

        txn.commit().await?;
        Ok(saved_item)
    }

    /// Remove a product variant item from the cart
    pub async fn remove_item(&self, user_id: Uuid, product_variant_id: Uuid) -> Result<()> {
        let txn = self.db.begin().await?;

        let cart = MarketplaceCart::find()
            .filter(marketplace_cart::Column::UserId.eq(user_id))
            .one(&txn)
            .await?
            .ok_or_else(|| DbError::Catalog("cart not found".into()))?;

        MarketplaceCartItem::delete_many()
            .filter(marketplace_cart_item::Column::CartId.eq(cart.id))
            .filter(marketplace_cart_item::Column::ProductVariantId.eq(product_variant_id))
            .exec(&txn)
            .await?;

        let mut cart_active: CartActive = cart.into();
        cart_active.updated_at = Set(Utc::now().into());
        cart_active.update(&txn).await?;

        txn.commit().await?;
        Ok(())
    }

    /// Fetch unified cart and its constituent items for user
    pub async fn get_cart_with_items(
        &self,
        user_id: Uuid,
    ) -> Result<Option<(marketplace_cart::Model, Vec<marketplace_cart_item::Model>)>> {
        let cart = match MarketplaceCart::find()
            .filter(marketplace_cart::Column::UserId.eq(user_id))
            .one(self.db)
            .await?
        {
            Some(c) => c,
            None => return Ok(None),
        };

        let items = MarketplaceCartItem::find()
            .filter(marketplace_cart_item::Column::CartId.eq(cart.id))
            .all(self.db)
            .await?;

        Ok(Some((cart, items)))
    }
}
