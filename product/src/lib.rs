mod routes;

pub use routes::create_router;

rust_i18n::i18n!("locales");

pub(crate) mod filters {
    #[allow(dead_code)]
    pub fn t(value: &str, values: &dyn askama::Values) -> askama::Result<String> {
        let preferred_language = askama::get_value::<String>(values, "preferred_language")
            .expect("Unable to get preferred_language from askama::get_value");

        Ok(rust_i18n::t!(value, locale = preferred_language).to_string())
    }
}

#[derive(Clone)]
pub struct State {
    pub executor: liteventd::sql::Sql<sqlx::Sqlite>,
}
