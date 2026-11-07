use sea_orm::entity::prelude::*;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "marketplace_cart_items")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub cart_id: Uuid,
    pub product_variant_id: Uuid,
    pub vendor_id: Uuid,
    pub quantity: i32,
    #[sea_orm(column_type = "Decimal(Some((12, 2)))")]
    pub price_amount: Decimal,
    pub currency: String,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::marketplace_cart::Entity",
        from = "Column::CartId",
        to = "super::marketplace_cart::Column::Id"
    )]
    Cart,
    #[sea_orm(
        belongs_to = "super::product_variant::Entity",
        from = "Column::ProductVariantId",
        to = "super::product_variant::Column::Id"
    )]
    Variant,
    #[sea_orm(
        belongs_to = "super::vendor::Entity",
        from = "Column::VendorId",
        to = "super::vendor::Column::Id"
    )]
    Vendor,
}

impl Related<super::marketplace_cart::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Cart.def()
    }
}

impl Related<super::product_variant::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Variant.def()
    }
}

impl Related<super::vendor::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Vendor.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
