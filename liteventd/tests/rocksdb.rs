#[path = "liteventd/mod.rs"]
mod liteventd_test;

use liteventd::rocksdb::RocksDB;

#[tokio::test]
async fn rocksdb_save() -> anyhow::Result<()> {
    let executor = create_executor("save").await?;

    liteventd_test::save(&executor).await
}

#[tokio::test]
async fn rocksdb_invalid_original_version() -> anyhow::Result<()> {
    let executor = create_executor("invalid_original_version").await?;

    liteventd_test::invalid_original_version(&executor).await
}

async fn create_executor(key: impl Into<String>) -> anyhow::Result<RocksDB> {
    todo!()
}
