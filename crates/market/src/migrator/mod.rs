mod market_2025_08_16_04_17;

use market_2025_08_16_04_17::Market202508160417;
use sqlx_migrator::{Info, Migrator};

pub fn add_migrations(migrator: &mut Migrator<sqlx::Sqlite>) -> Result<(), sqlx_migrator::Error> {
    migrator.add_migration(Box::new(Market202508160417))?;

    Ok(())
}
