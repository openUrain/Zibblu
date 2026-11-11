use anyhow::{Context, ensure};
use sqlx::postgres::PgPoolOptions;
use std::{net::SocketAddr, time::Duration};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let address: SocketAddr = std::env::var("ZIBBLU_BIND")
        .unwrap_or_else(|_| "127.0.0.1:7841".into())
        .parse()
        .context("ZIBBLU_BIND must be a loopback address and port")?;
    ensure!(
        address.ip().is_loopback(),
        "Bind the service to loopback and use an HTTPS proxy for remote clients"
    );
    let database_url = std::env::var("DATABASE_URL")
        .context("Set DATABASE_URL to a Zibblu PostgreSQL database")?;
    let db = PgPoolOptions::new()
        .max_connections(12)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&database_url)
        .await
        .map_err(|_| anyhow::anyhow!("Could not connect to the Zibblu PostgreSQL database"))?;
    let app = zibblu_online::router(db).await?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    println!(
        "Zibblu online service listening at {}",
        listener.local_addr()?
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
