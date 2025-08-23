mod assets;
mod axum_extra;
mod error;
mod router;

use axum_extra::TemplateConfig;
use clap::{arg, command, Command};
use config::Config;
use serde::Deserialize;
use sqlx::{any::install_default_drivers, migrate::MigrateDatabase, SqlitePool};
use sqlx_migrator::Migrate as _;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

rust_i18n::i18n!("locales");

pub(crate) mod filters {
    pub fn t(value: &str, values: &dyn askama::Values) -> askama::Result<String> {
        let preferred_language = askama::get_value::<String>(values, "preferred_language")
            .expect("Unable to get preferred_language from askama::get_value");

        Ok(rust_i18n::t!(value, locale = preferred_language).to_string())
    }

    pub fn assets(value: &str, values: &dyn askama::Values) -> askama::Result<String> {
        let config = askama::get_value::<crate::axum_extra::TemplateConfig>(values, "config")
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
        .subcommand(
            Command::new("reset")
                .about("Reset timada database")
                .arg(arg!(-c --config <FILE> "path to configuration file").required(true)),
        )
        .get_matches();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_env("TIMADA_LOG").unwrap_or_else(|_| {
                matches
                    .get_one::<String>("log")
                    .cloned()
                    .unwrap_or_else(|| format!("{}=error,evento=error", env!("CARGO_CRATE_NAME")))
                    .into()
            }),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    match matches.subcommand() {
        Some(("serve", sub_matches)) => {
            let config = sub_matches.get_one::<String>("config").expect("required");
            let config = expect_config(config);
            if let Err(err) = serve(config).await {
                tracing::error!("{err}");

                std::process::exit(1);
            }
        }
        Some(("migrate", sub_matches)) => {
            let config = sub_matches.get_one::<String>("config").expect("required");
            let config = expect_config(config);
            if let Err(err) = migrate(config).await {
                tracing::error!("{err}");

                std::process::exit(1);
            }
        }
        #[cfg(debug_assertions)]
        Some(("reset", sub_matches)) => {
            let config = sub_matches.get_one::<String>("config").expect("required");
            let config = expect_config(config);
            if let Err(err) = reset(config).await {
                tracing::error!("{err}");

                std::process::exit(1);
            }
        }
        #[cfg(not(debug_assertions))]
        Some(("reset", _sub_matches)) => {
            tracing::error!("reset command not allow in prodoction");

            std::process::exit(1);
        }
        _ => unreachable!("Exhausted list of subcommands and subcommand_required prevents `None`"),
    };

    Ok(())
}

fn expect_config<E: serde::de::DeserializeOwned>(path: &str) -> E {
    match get_config::<E>(path) {
        Ok(c) => c,
        Err(err) => {
            tracing::error!("{err}");

            std::process::exit(1);
        }
    }
}

fn get_config<E: serde::de::DeserializeOwned>(path: &str) -> Result<E, config::ConfigError> {
    Config::builder()
        .add_source(config::File::with_name(path).required(true))
        .add_source(config::Environment::with_prefix(env!("CARGO_PKG_NAME")))
        .build()?
        .try_deserialize()
}

#[derive(Deserialize, Clone)]
pub struct Serve {
    pub addr: String,
    pub region: String,
    #[serde(rename = "assets-base-url")]
    pub assets_base_url: String,
    #[serde(rename = "data-dir")]
    pub data_dir: String,
    pub dsn: String,
}

#[derive(Clone)]
pub struct State {
    pub config: Serve,
    pub evento: evento::Evento,
    pub query_pool: SqlitePool,
}

pub async fn serve(config: Serve) -> anyhow::Result<()> {
    let evento_executor: evento::Evento = if config.dsn.starts_with("sqlite:") {
        let executor: evento::Sqlite = sqlx::SqlitePool::connect(&config.dsn).await?.into();
        executor.into()
    } else if config.dsn.starts_with("mysql:") {
        let executor: evento::MySql = sqlx::MySqlPool::connect(&config.dsn).await?.into();
        executor.into()
    } else if config.dsn.starts_with("postgres:") {
        let executor: evento::Postgres = sqlx::PgPool::connect(&config.dsn).await?.into();
        executor.into()
    } else {
        anyhow::bail!(
            "{} not supported, consider using Sqlite, MySql or Postgres",
            config.dsn
        )
    };

    let query_db =
        sqlx::SqlitePool::connect(&format!("{}/query.sqlite3", &config.data_dir)).await?;

    timada_market::product::subscribe_command(&config.region)
        .run(&evento_executor)
        .await?;

    timada_market::product::subscribe_query_products(&config.region)?
        .data(query_db.clone())
        .run(&evento_executor)
        .await?;

    let addr = config.addr.to_owned();

    let mut app = router::create_router()
        .layer(TemplateConfig::new(&config.assets_base_url))
        .with_state(State {
            config,
            evento: evento_executor.clone(),
            query_pool: query_db, // should be read sqlite ?
        });

    #[cfg(debug_assertions)]
    {
        app = app.layer(tower_livereload::LiveReloadLayer::new());
    }

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("listening on {}", listener.local_addr()?);
    axum::serve(listener, app).await?;

    Ok(())
}

#[derive(Deserialize)]
struct Migrate {
    #[serde(rename = "data-dir")]
    pub data_dir: String,
    pub dsn: String,
}

async fn migrate(config: Migrate) -> anyhow::Result<()> {
    install_default_drivers();

    if let Err(err) = sqlx::any::Any::create_database(&config.dsn).await {
        tracing::warn!("{err}");
    };

    if config.dsn.starts_with("sqlite:") {
        let pool = sqlx::SqlitePool::connect(&config.dsn).await?;
        let mut conn = pool.acquire().await?;
        let evento_migrator = evento::sql_migrator::new_migrator::<sqlx::Sqlite>()?;
        evento_migrator
            .run(&mut *conn, &sqlx_migrator::Plan::apply_all())
            .await?;
    } else if config.dsn.starts_with("mysql:") {
        let pool = sqlx::MySqlPool::connect(&config.dsn).await?;
        let mut conn = pool.acquire().await?;
        let evento_migrator = evento::sql_migrator::new_migrator::<sqlx::MySql>()?;
        evento_migrator
            .run(&mut *conn, &sqlx_migrator::Plan::apply_all())
            .await?;
    } else if config.dsn.starts_with("postgres:") {
        let pool = sqlx::PgPool::connect(&config.dsn).await?;
        let mut conn = pool.acquire().await?;
        let evento_migrator = evento::sql_migrator::new_migrator::<sqlx::Postgres>()?;
        evento_migrator
            .run(&mut *conn, &sqlx_migrator::Plan::apply_all())
            .await?;
    } else {
        anyhow::bail!(
            "{} not supported, consider using Sqlite, MySql or Postgres",
            config.dsn
        )
    }

    let dsn = format!("{}/query.sqlite3", config.data_dir);
    if let Err(err) = sqlx::Sqlite::create_database(&dsn).await {
        tracing::warn!("{err}");
    };

    let pool = sqlx::SqlitePool::connect(&dsn).await?;
    let mut conn = pool.acquire().await?;
    let mut migrator = sqlx_migrator::Migrator::default();
    timada_market::migrator::add_migrations(&mut migrator)?;
    migrator
        .run(&mut *conn, &sqlx_migrator::Plan::apply_all())
        .await?;

    Ok(())
}

#[cfg(debug_assertions)]
async fn reset(config: Migrate) -> anyhow::Result<()> {
    install_default_drivers();

    if let Err(err) = sqlx::any::Any::drop_database(&config.dsn).await {
        tracing::warn!("{err}");
    };

    let dsn = format!("{}/query.sqlite3", config.data_dir);
    if let Err(err) = sqlx::Sqlite::drop_database(&dsn).await {
        tracing::warn!("{err}");
    };

    migrate(config).await
}
