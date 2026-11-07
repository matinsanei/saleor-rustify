use chrono::Utc;
use rust_decimal::Decimal;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait,
    QueryFilter, Set,
};
use uuid::Uuid;

use crate::marketplace::entities::vendor::{self, ActiveModel, Entity as Vendor};
use crate::{DbError, Result};

pub struct VendorService<'a> {
    db: &'a DatabaseConnection,
}

impl<'a> VendorService<'a> {
    pub fn new(db: &'a DatabaseConnection) -> Self {
        Self { db }
    }

    /// Register a new vendor profile linked to a platform user identity
    pub async fn register_vendor(
        &self,
        user_id: Uuid,
        slug: String,
        name: String,
        bio: Option<String>,
        logo_url: Option<String>,
        banner_url: Option<String>,
        commission_rate: Option<Decimal>,
        payout_account_info: Option<serde_json::Value>,
    ) -> Result<vendor::Model> {
        let clean_slug = slug.trim().to_lowercase();
        if clean_slug.is_empty() {
            return Err(DbError::Catalog("vendor slug cannot be empty".into()));
        }
        if name.trim().is_empty() {
            return Err(DbError::Catalog("vendor name cannot be empty".into()));
        }

        // Check if slug already exists
        let existing = Vendor::find()
            .filter(vendor::Column::Slug.eq(&clean_slug))
            .one(self.db)
            .await?;
        if existing.is_some() {
            return Err(DbError::Catalog(format!("vendor slug '{clean_slug}' is already taken")));
        }

        let rate = commission_rate.unwrap_or_else(|| Decimal::new(10, 2)); // default 10%
        let now = Utc::now().into();

        let new_vendor = ActiveModel {
            id: Set(Uuid::new_v4()),
            user_id: Set(user_id),
            slug: Set(clean_slug),
            name: Set(name.trim().to_string()),
            bio: Set(bio),
            logo_url: Set(logo_url),
            banner_url: Set(banner_url),
            commission_rate: Set(rate),
            is_verified: Set(false),
            is_active: Set(true),
            payout_account_info: Set(payout_account_info),
            created_at: Set(now),
            updated_at: Set(now),
        };

        let inserted = new_vendor.insert(self.db).await?;
        Ok(inserted)
    }

    /// Update vendor profile with strict ownership verification
    pub async fn update_profile(
        &self,
        vendor_id: Uuid,
        requester_user_id: Uuid,
        name: Option<String>,
        bio: Option<String>,
        logo_url: Option<String>,
        banner_url: Option<String>,
        payout_account_info: Option<serde_json::Value>,
    ) -> Result<vendor::Model> {
        let vendor = self.verify_ownership(vendor_id, requester_user_id).await?;

        let mut active: ActiveModel = vendor.into();
        if let Some(n) = name {
            if n.trim().is_empty() {
                return Err(DbError::Catalog("vendor name cannot be empty".into()));
            }
            active.name = Set(n.trim().to_string());
        }
        if let Some(b) = bio {
            active.bio = Set(Some(b));
        }
        if let Some(l) = logo_url {
            active.logo_url = Set(Some(l));
        }
        if let Some(bn) = banner_url {
            active.banner_url = Set(Some(bn));
        }
        if let Some(p) = payout_account_info {
            active.payout_account_info = Set(Some(p));
        }
        active.updated_at = Set(Utc::now().into());

        let updated = active.update(self.db).await?;
        Ok(updated)
    }

    /// Strict RBAC ownership check: ensures the vendor exists and belongs to the requester
    pub async fn verify_ownership(
        &self,
        vendor_id: Uuid,
        requester_user_id: Uuid,
    ) -> Result<vendor::Model> {
        let vendor = Vendor::find_by_id(vendor_id)
            .one(self.db)
            .await?
            .ok_or_else(|| DbError::Catalog(format!("vendor {vendor_id} not found")))?;

        if vendor.user_id != requester_user_id {
            return Err(DbError::Catalog(
                "unauthorized: requester is not the owner of this vendor account".into(),
            ));
        }

        if !vendor.is_active {
            return Err(DbError::Catalog("vendor account is deactivated".into()));
        }

        Ok(vendor)
    }

    pub async fn get_by_id(&self, vendor_id: Uuid) -> Result<Option<vendor::Model>> {
        let vendor = Vendor::find_by_id(vendor_id).one(self.db).await?;
        Ok(vendor)
    }

    pub async fn get_by_slug(&self, slug: &str) -> Result<Option<vendor::Model>> {
        let clean = slug.trim().to_lowercase();
        let vendor = Vendor::find()
            .filter(vendor::Column::Slug.eq(&clean))
            .one(self.db)
            .await?;
        Ok(vendor)
    }

    pub async fn get_by_user_id(&self, user_id: Uuid) -> Result<Option<vendor::Model>> {
        let vendor = Vendor::find()
            .filter(vendor::Column::UserId.eq(user_id))
            .one(self.db)
            .await?;
        Ok(vendor)
    }
}
