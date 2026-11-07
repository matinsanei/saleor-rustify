use sea_orm::entity::prelude::*;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "product_variants")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub product_id: Uuid,
    #[sea_orm(unique)]
    pub sku: String,
    pub name: String,
    #[sea_orm(column_type = "Decimal(Some((12, 2)))")]
    pub price_amount: Decimal,
    pub currency: String,
    #[sea_orm(column_type = "Decimal(Some((12, 2)))", nullable)]
    pub compare_at_price: Option<Decimal>,
    pub stock_quantity: i32,
    #[sea_orm(column_type = "JsonBinary")]
    pub attributes: Json,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::product::Entity",
        from = "Column::ProductId",
        to = "super::product::Column::Id"
    )]
    Product,
    #[sea_orm(has_many = "super::marketplace_cart_item::Entity")]
    CartItems,
}

impl Related<super::product::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Product.def()
    }
}

impl Related<super::marketplace_cart_item::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::CartItems.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
