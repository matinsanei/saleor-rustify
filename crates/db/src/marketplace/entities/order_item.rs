use sea_orm::entity::prelude::*;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "order_items")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub sub_order_id: Uuid,
    pub product_variant_id: Uuid,
    pub product_title: String,
    pub variant_name: String,
    pub sku: String,
    pub quantity: i32,
    #[sea_orm(column_type = "Decimal(Some((12, 2)))")]
    pub unit_price_amount: Decimal,
    #[sea_orm(column_type = "Decimal(Some((12, 2)))")]
    pub total_price_amount: Decimal,
    pub currency: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::vendor_sub_order::Entity",
        from = "Column::SubOrderId",
        to = "super::vendor_sub_order::Column::Id"
    )]
    SubOrder,
}

impl Related<super::vendor_sub_order::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::SubOrder.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
