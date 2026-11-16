use anyhow::{Context, ensure};
use sqlx::postgres::PgPoolOptions;
use std::{net::SocketAddr, time::Duration};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let address: SocketAddr = std::env::var("ZIBBLU_BIND")
        .unwrap_or_else(|_| "127.0.0.1:7841".into())
        .parse()
        .context("ZIBBLU_BIND must be a socket address")?;
    ensure!(
        address.ip().is_loopback()
            || (std::env::var("ZIBBLU_CONTAINER").as_deref() == Ok("1")
                && address == SocketAddr::from(([0, 0, 0, 0], 7841))),
        "Bind the service to loopback and use an HTTPS proxy for remote clients"
    );
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("Could not register the SIGTERM handler")?;
    let database_url = std::env::var("DATABASE_URL")
        .context("Set DATABASE_URL to a Zibblu PostgreSQL database")?;
    let db = PgPoolOptions::new()
        .max_connections(12)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&database_url)
        .await
        .map_err(|_| anyhow::anyhow!("Could not connect to the Zibblu PostgreSQL database"))?;
    let retention = zibblu_online::retention::Retention::from_env()?;
    let app = zibblu_online::router_with_retention(db.clone(), retention).await?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    if let Some(interval) = retention.sweep_interval {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                if zibblu_online::retention::sweep(&db).await.is_err() {
                    eprintln!("Local-First cleanup batch failed; it will be retried.");
                }
            }
        });
    }
    println!(
        "Zibblu online service listening at {}",
        listener.local_addr()?
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            #[cfg(unix)]
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = terminate.recv() => {},
            }
            #[cfg(not(unix))]
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
