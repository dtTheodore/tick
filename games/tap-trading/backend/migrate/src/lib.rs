//! Tick migration runner — library API consumed by Plan C/D/E integration
//! tests and by each service's startup as a "migrate-or-fail" boot guard.

use std::borrow::Cow;

use sqlx::PgPool;
use thiserror::Error;

/// Postgres table where applied migrations are recorded.
///
/// Overridden away from sqlx's default `_sqlx_migrations` because the same
/// database hosts the platform-side migrations (which use the default name).
/// Two migration sets sharing one record table would cross-corrupt checksums.
pub const MIGRATION_TABLE: &str = "_tap_sqlx_migrations";

/// One row from `_tap_sqlx_migrations` projected into a public type so callers
/// don't depend on `sqlx::migrate` internals. Populated in Task 3.
#[derive(Debug, Clone)]
pub struct AppliedMigration {
    pub version: i64,
    pub description: String,
    pub installed_on_ms: i64,
    /// Duration in nanoseconds (sqlx stores nanoseconds in `execution_time`).
    pub execution_time_ns: i64,
    pub success: bool,
}

/// Errors `run_migrations` and `list_applied` surface to callers.
#[derive(Debug, Error)]
pub enum MigrateError {
    #[error("sqlx migrate: {0}")]
    Sqlx(#[from] sqlx::migrate::MigrateError),
    #[error("sqlx: {0}")]
    Driver(#[from] sqlx::Error),
}

fn tick_migrator() -> sqlx::migrate::Migrator {
    // sqlx::migrate!() returns &'static Migrator; Migrator does not implement Clone.
    // We construct an owned Migrator from the public fields, copying migrations from
    // the static ref (Migration: Clone, so Cow<'static, [Migration]>: Clone).
    // dangerous_set_table_name is a &mut self method and cannot be called on the
    // &'static reference, so struct-literal construction is the only clean path.
    let src = sqlx::migrate!("../migrations");
    sqlx::migrate::Migrator {
        migrations: src.migrations.clone(),
        table_name: Cow::Borrowed(MIGRATION_TABLE),
        ..sqlx::migrate::Migrator::DEFAULT
    }
}

/// Apply every Tick migration to `pool`. Idempotent: rerunning is a no-op
/// once every migration's checksum matches what's already in
/// `_tap_sqlx_migrations`.
pub async fn run_migrations(pool: &PgPool) -> Result<(), MigrateError> {
    tick_migrator().run(pool).await?;
    Ok(())
}

/// List applied migrations from `_tap_sqlx_migrations`.
///
/// Returns an empty vec if the migration table doesn't exist yet (fresh
/// database before the first `run_migrations` call). This lets operators run
/// `info` against a brand-new database without an error.
pub async fn list_applied(pool: &PgPool) -> Result<Vec<AppliedMigration>, MigrateError> {
    let table_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM information_schema.tables
             WHERE table_schema = 'public' AND table_name = $1
         )",
    )
    .bind(MIGRATION_TABLE)
    .fetch_one(pool)
    .await?;

    if !table_exists {
        return Ok(Vec::new());
    }

    // Table name is a compile-time constant we control — no user input concatenated.
    let sql = sqlx::AssertSqlSafe(format!(
        "SELECT version,
                description,
                (EXTRACT(EPOCH FROM installed_on) * 1000)::BIGINT AS installed_on_ms,
                execution_time AS execution_time_ns,
                success
         FROM {MIGRATION_TABLE}
         ORDER BY version ASC"
    ));
    let rows = sqlx::query(sql).fetch_all(pool).await?;

    let mut applied = Vec::with_capacity(rows.len());
    for row in rows {
        use sqlx::Row;
        applied.push(AppliedMigration {
            version: row.try_get("version")?,
            description: row.try_get("description")?,
            installed_on_ms: row.try_get("installed_on_ms")?,
            execution_time_ns: row.try_get("execution_time_ns")?,
            success: row.try_get("success")?,
        });
    }
    Ok(applied)
}
