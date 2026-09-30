//! Request context: DB handle + raw Authorization header.

use sea_orm::DatabaseConnection;

pub const MANAGE_ORDERS: &str = "manage_orders";

#[derive(Clone, Debug)]
pub struct Bearer(pub String);

/// Request-scoped Saleor channel slug, set by the gateway from the
/// `x-saleor-channel` HTTP header. Nested resolvers (variant/product
/// pricing) have no channel argument in the schema, so this carries the
/// query's channel the way Saleor's ChannelContext does. Absent = the
/// caller didn't scope the request → default-channel fallback.
#[derive(Clone, Debug)]
pub struct RequestChannel(pub String);

#[derive(Clone)]
pub struct GqlContext {
    pub db: Option<DatabaseConnection>,
    pub bearer: Option<String>,
}

impl GqlContext {
    pub fn db(&self) -> Result<&DatabaseConnection, String> {
        self.db.as_ref().ok_or_else(|| "postgres unavailable".into())
    }
}
