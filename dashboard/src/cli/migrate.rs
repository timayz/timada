use anyhow::Result;

use crate::config::Config;
use crate::db;

/// Creates the store and the read-model database if needed and applies
/// their pending migrations.
pub async fn migrate(config: Config) -> Result<()> {
    tracing::info!("running database migrations");
    // Opening the store migrates it; closed again right away.
    drop(db::open_store(&config.database.path).await?);
    db::migrate(&config.database.read_models, db::read_model_migrator()?).await?;
    tracing::info!("migrations completed");
    Ok(())
}

/// Drops both database files, the pictures and the personal-data keys, then
/// migrates fresh ones. Development only: the subcommand does not exist in
/// release builds.
#[cfg(debug_assertions)]
pub async fn reset(config: Config) -> Result<()> {
    use sqlx::migrate::MigrateDatabase;

    let path = &config.database.path;
    if path.exists() {
        tracing::warn!(path = %path.display(), "dropping the store");
    } else {
        tracing::info!(path = %path.display(), "the store does not exist, nothing to drop");
    }
    db::remove_store(path)?;
    let url = &config.database.read_models;
    if sqlx::Sqlite::database_exists(url).await? {
        tracing::warn!(url, "dropping existing database");
        sqlx::Sqlite::drop_database(url).await?;
    } else {
        tracing::info!(url, "database does not exist, nothing to drop");
    }
    let files = crate::cluster::Files {
        path: config.database.path.clone(),
        read_models_url: config.database.read_models.clone(),
    };
    if let Ok(marker) = files.marker()
        && marker.exists()
    {
        std::fs::remove_file(marker)?;
    }
    let media = &config.media.dir;
    if media.is_dir() {
        tracing::warn!(dir = %media.display(), "removing the uploaded pictures");
        std::fs::remove_dir_all(media)?;
    }
    let keys = &config.data_keys.dir;
    if keys.is_dir() {
        tracing::warn!(dir = %keys.display(), "removing the personal-data keys");
        std::fs::remove_dir_all(keys)?;
    }

    migrate(config).await?;

    tracing::info!("database reset completed");
    Ok(())
}
