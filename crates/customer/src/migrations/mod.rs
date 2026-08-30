mod m0001_credentials;

use sqlx_migrator::migration::Migration;
use sqlx_migrator::vec_box;

pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![m0001_credentials::M0001Credentials]
}
