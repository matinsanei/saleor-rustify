use sea_orm::entity::prelude::*;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "marketplace_orders")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub buyer_id: Uuid,
    #[sea_orm(column_type = "Decimal(Some((12, 2)))")]
    pub total_gross_amount: Decimal,
    pub payment_status: String,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::vendor_sub_order::Entity")]
    SubOrders,
}

impl Related<super::vendor_sub_order::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::SubOrders.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
