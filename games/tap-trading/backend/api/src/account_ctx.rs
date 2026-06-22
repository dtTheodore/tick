//! Authenticated request context. Attached by `account_id` middleware.

#[derive(Debug, Clone)]
pub struct AccountCtx {
    pub id: i64,
    pub external_id: String,
}
