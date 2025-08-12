mod assets;
mod routes;

use clap::{arg, command, Command};
use config::Config;
use serde::Deserialize;
use shared::TemplateConfig;
use sqlx::{any::install_default_drivers, migrate::MigrateDatabase};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

rust_i18n::i18n!("locales");

pub(crate) mod filters {
    pub fn t(value: &str, values: &dyn askama::Values) -> askama::Result<String> {
        let preferred_language = askama::get_value::<String>(values, "preferred_language")
            .expect("Unable to get preferred_language from askama::get_value");

        Ok(rust_i18n::t!(value, locale = preferred_language).to_string())
    }

    pub fn assets(value: &str, values: &dyn askama::Values) -> askama::Result<String> {
        let config = askama::get_value::<shared::TemplateConfig>(values, "config")
            .expect("Unable to get config from askama::get_value");

        Ok(format!("{}/{value}", config.assets_base_url))
    }
}

#[tokio::main()]
async fn main() -> anyhow::Result<()> {
    let matches = command!()
        .propagate_version(true)
        .subcommand_required(true)
        .arg_required_else_help(true)
        .arg(arg!(--log [LEVEL] "define log level, default is error"))
        .subcommand(
            Command::new("serve")
                .about("Serve timada admin web server")
                .arg(arg!(-c --config <FILE> "path to configuration file").required(true)),
        )
        .subcommand(
            Command::new("migrate")
                .about("Create timada database")
                .arg(arg!(-c --config <FILE> "path to configuration file").required(true)),
        )
        .get_matches();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_env("TIMADA_LOG").unwrap_or_else(|_| {
                matches
                    .get_one::<String>("log")
                    .cloned()
                    .unwrap_or_else(|| {
                        format!("{}=error,liteventd=error", env!("CARGO_CRATE_NAME"))
                    })
                    .into()
            }),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    match matches.subcommand() {
        Some(("serve", sub_matches)) => {
            let config_path = sub_matches.get_one::<String>("config").expect("required");
            if let Err(err) = serve(config_path).await {
                tracing::error!("{err}");

                std::process::exit(1);
            }
        }
        Some(("migrate", sub_matches)) => {
            let config_path = sub_matches.get_one::<String>("config").expect("required");
            if let Err(err) = migrate(config_path).await {
                tracing::error!("{err}");

                std::process::exit(1);
            }
        }
        _ => unreachable!("Exhausted list of subcommands and subcommand_required prevents `None`"),
    };

    Ok(())
}

#[derive(Deserialize)]
struct Product {
    pub dsn: String,
}

#[derive(Deserialize)]
struct Serve {
    pub addr: String,
    pub assets_base_url: String,
    pub product: Product,
}

pub async fn serve(config_path: impl Into<String>) -> anyhow::Result<()> {
    let config_path = config_path.into();
    let config: Serve = Config::builder()
        .add_source(config::File::with_name(&config_path).required(true))
        .add_source(config::Environment::with_prefix(env!("CARGO_PKG_NAME")))
        .build()?
        .try_deserialize()?;

    let product_executor: liteventd::sql::Sql<sqlx::Sqlite> =
        sqlx::SqlitePool::connect(&config.product.dsn).await?.into();

    let mut app = routes::create_router()
        .layer(TemplateConfig::new(&config.assets_base_url))
        .layer(product::State {
            executor: product_executor.clone(),
        });

    #[cfg(debug_assertions)]
    {
        app = app.layer(tower_livereload::LiveReloadLayer::new());
    }

    let listener = tokio::net::TcpListener::bind(&config.addr).await?;
    tracing::info!("listening on {}", listener.local_addr()?);
    axum::serve(listener, app).await?;

    Ok(())
}

#[derive(Deserialize)]
struct Migrate {
    pub product: Product,
}

pub async fn migrate(config_path: impl Into<String>) -> anyhow::Result<()> {
    let config_path = config_path.into();
    let config: Migrate = Config::builder()
        .add_source(config::File::with_name(&config_path).required(true))
        .add_source(config::Environment::with_prefix(env!("CARGO_PKG_NAME")))
        .build()?
        .try_deserialize()?;

    install_default_drivers();

    sqlx::Any::create_database(&config.product.dsn).await?;

    let (pool, schema) = match config
        .product
        .dsn
        .split_once(":")
        .expect("Invalid product dsn")
        .0
    {
        "sqlite" => {
            let pool = sqlx::Pool::<sqlx::Sqlite>::connect(&config.product.dsn).await?;
            let schema = liteventd::sql::Sql::<sqlx::Sqlite>::get_schema();

            (pool, schema)
        }
        name => panic!("'{name}' not supported, consider using SQLite"),
    };

    sqlx::query(&schema).execute(&pool).await?;

    Ok(())
}
