mod market_2025_08_16_04_17;

use market_2025_08_16_04_17::Market202508160417;
use sqlx_migrator::{Info, Migrator};

pub fn new_market_migrator() -> Result<Migrator<sqlx::Sqlite>, sqlx_migrator::Error> {
    let mut migrator = Migrator::default();
    migrator.add_migration(Box::new(Market202508160417))?;

    Ok(migrator)
}
