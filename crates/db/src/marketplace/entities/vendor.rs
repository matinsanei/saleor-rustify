use sea_orm::entity::prelude::*;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "vendors")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub user_id: Uuid,
    #[sea_orm(unique)]
    pub slug: String,
    pub name: String,
    pub bio: Option<String>,
    pub logo_url: Option<String>,
    pub banner_url: Option<String>,
    #[sea_orm(column_type = "Decimal(Some((5, 4)))")]
    pub commission_rate: Decimal,
    pub is_verified: bool,
    pub is_active: bool,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub payout_account_info: Option<Json>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_one = "super::store_layout::Entity")]
    StoreLayout,
    #[sea_orm(has_many = "super::product::Entity")]
    Products,
    #[sea_orm(has_many = "super::vendor_sub_order::Entity")]
    SubOrders,
}

impl Related<super::store_layout::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::StoreLayout.def()
    }
}

impl Related<super::product::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Products.def()
    }
}

impl Related<super::vendor_sub_order::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::SubOrders.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
