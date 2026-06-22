//! Tick migration runner CLI. Mirrors the library API in `lib.rs` so the
//! operator path (`docker run … tap-trading-migrate run`) and the test
//! path (`tap_trading_migrate::run_migrations(&pool)`) can never disagree.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use sqlx::postgres::PgPoolOptions;
use tap_trading_migrate::{run_migrations, MIGRATION_TABLE};

#[derive(Debug, Parser)]
#[command(name = "tap-trading-migrate", version, about = "Tick migration runner")]
struct Cli {
    /// Database URL. Reads `TAP_DB_URL` env var. `PLATFORM_DB_URL` is the fallback.
    #[arg(long, env = "TAP_DB_URL")]
    database_url: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Apply every pending Tick migration.
    Run,
    /// List migrations already applied to the target database.
    Info,
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

fn resolve_db_url(arg: Option<String>) -> Result<String> {
    // clap populates `arg` from --database-url or TAP_DB_URL env (via #[arg(env)]);
    // if neither was set, fall back to PLATFORM_DB_URL.
    if let Some(url) = arg {
        return Ok(url);
    }
    std::env::var("PLATFORM_DB_URL").context("set --database-url, TAP_DB_URL, or PLATFORM_DB_URL")
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();
    let url = resolve_db_url(cli.database_url)?;

    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .context("connect to database")?;

    match cli.command {
        Command::Run => {
            tracing::info!(table = MIGRATION_TABLE, "applying migrations");
            run_migrations(&pool).await.context("run_migrations")?;
            tracing::info!("migrations applied");
        }
        Command::Info => {
            let applied = tap_trading_migrate::list_applied(&pool)
                .await
                .context("list_applied")?;
            if applied.is_empty() {
                println!("no migrations applied yet (table {MIGRATION_TABLE} absent or empty)");
            } else {
                println!("{} applied migration(s):", applied.len());
                for row in &applied {
                    let status = if row.success { "ok" } else { "FAILED" };
                    println!(
                        "  [{status}] {ver:>20} {desc}",
                        ver = row.version,
                        desc = row.description,
                    );
                }
            }
        }
    }

    Ok(())
}
