use sea_orm::entity::prelude::*;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "vendor_sub_orders")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub parent_order_id: Uuid,
    pub vendor_id: Uuid,
    #[sea_orm(column_type = "Decimal(Some((12, 2)))")]
    pub subtotal_amount: Decimal,
    #[sea_orm(column_type = "Decimal(Some((12, 2)))")]
    pub commission_amount: Decimal,
    #[sea_orm(column_type = "Decimal(Some((12, 2)))")]
    pub vendor_payout_amount: Decimal,
    pub fulfillment_status: String,
    pub tracking_number: Option<String>,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::marketplace_order::Entity",
        from = "Column::ParentOrderId",
        to = "super::marketplace_order::Column::Id"
    )]
    ParentOrder,
    #[sea_orm(
        belongs_to = "super::vendor::Entity",
        from = "Column::VendorId",
        to = "super::vendor::Column::Id"
    )]
    Vendor,
    #[sea_orm(has_many = "super::order_item::Entity")]
    OrderItems,
}

impl Related<super::marketplace_order::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ParentOrder.def()
    }
}

impl Related<super::vendor::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Vendor.def()
    }
}

impl Related<super::order_item::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::OrderItems.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
