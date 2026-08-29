mod m0001_regions;

use sqlx_migrator::migration::Migration;
use sqlx_migrator::vec_box;

pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![m0001_regions::M0001Regions]
}
