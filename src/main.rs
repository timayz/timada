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

#[derive(Deserialize, Clone)]
pub struct Market {
    pub dsn: String,
}

#[derive(Deserialize, Clone)]
pub struct Serve {
    pub addr: String,
    pub region: String,
    pub assets_base_url: String,
    pub data_dir: String,
    pub market: Market,
}

#[derive(Clone)]
pub struct State {
    pub config: Serve,
    pub market_executor: evento::Evento,
    pub market_db: SqlitePool,
}

pub async fn serve(config_path: impl Into<String>) -> anyhow::Result<()> {
    let config_path = config_path.into();
    let config: Serve = Config::builder()
        .add_source(config::File::with_name(&config_path).required(true))
        .add_source(config::Environment::with_prefix(env!("CARGO_PKG_NAME")))
        .build()?
        .try_deserialize()?;

    let mut executors: Vec<evento::Evento> = vec![];
    for dsn in [&config.market.dsn] {
        if dsn.starts_with("sqlite:") {
            let executor: evento::Sqlite = sqlx::SqlitePool::connect(dsn).await?.into();
            executors.push(executor.into())
        }
        if dsn.starts_with("mysql:") {
            let executor: evento::MySql = sqlx::MySqlPool::connect(dsn).await?.into();
            executors.push(executor.into())
        }
        if dsn.starts_with("postgres:") {
            let executor: evento::Postgres = sqlx::PgPool::connect(dsn).await?.into();
            executors.push(executor.into())
        }
    }

    let market_db =
        sqlx::SqlitePool::connect(&format!("{}/market_query.db", &config.data_dir)).await?;

    timada_market::product::subscribe_command(&config.region)
        .run(&executors[0])
        .await?;

    timada_market::product::subscribe_query_products(&config.region)?
        .data(market_db.clone())
        .run(&executors[0])
        .await?;

    let addr = config.addr.to_owned();

    let mut app = router::create_router()
        .layer(TemplateConfig::new(&config.assets_base_url))
        .with_state(State {
            config,
            market_executor: executors[0].clone(),
            market_db, // should be read sqlite ?
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
    pub data_dir: String,
    pub market: Market,
}

pub async fn migrate(config_path: impl Into<String>) -> anyhow::Result<()> {
    let config_path = config_path.into();
    let config: Migrate = Config::builder()
        .add_source(config::File::with_name(&config_path).required(true))
        .add_source(config::Environment::with_prefix(env!("CARGO_PKG_NAME")))
        .build()?
        .try_deserialize()?;

    install_default_drivers();

    for dsn in [&config.market.dsn] {
        if let Err(err) = sqlx::any::Any::create_database(dsn).await {
            tracing::warn!("{err}");
        };

        if dsn.starts_with("sqlite:") {
            let pool = sqlx::SqlitePool::connect(dsn).await?;
            let mut conn = pool.acquire().await?;
            let evento_migrator = evento::sql_migrator::new_migrator::<sqlx::Sqlite>()?;
            evento_migrator
                .run(&mut *conn, &sqlx_migrator::Plan::apply_all())
                .await?;
        }
        if dsn.starts_with("mysql:") {
            let pool = sqlx::MySqlPool::connect(dsn).await?;
            let mut conn = pool.acquire().await?;
            let evento_migrator = evento::sql_migrator::new_migrator::<sqlx::MySql>()?;
            evento_migrator
                .run(&mut *conn, &sqlx_migrator::Plan::apply_all())
                .await?;
        }
        if dsn.starts_with("postgres:") {
            let pool = sqlx::PgPool::connect(dsn).await?;
            let mut conn = pool.acquire().await?;
            let evento_migrator = evento::sql_migrator::new_migrator::<sqlx::Postgres>()?;
            evento_migrator
                .run(&mut *conn, &sqlx_migrator::Plan::apply_all())
                .await?;
        }
    }

    let dsn = format!("{}/market_query.db", config.data_dir);
    if let Err(err) = sqlx::Sqlite::create_database(&dsn).await {
        tracing::warn!("{err}");
    };

    let pool = sqlx::SqlitePool::connect(&dsn).await?;
    let market_migrator = timada_market::migrator::new_market_migrator()?;
    let mut conn = pool.acquire().await?;
    market_migrator
        .run(&mut *conn, &sqlx_migrator::Plan::apply_all())
        .await?;

    Ok(())
}
