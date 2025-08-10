mod assets;
mod routes;

use axum::Extension;
use clap::{arg, command, Command};
use config::Config;
use serde::Deserialize;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

rust_i18n::i18n!("locales");

pub(crate) mod filters {
    pub fn t(value: &str, values: &dyn askama::Values) -> askama::Result<String> {
        let preferred_language = askama::get_value::<String>(values, "preferred_language")
            .expect("Unable to get preferred_language from askama::get_value");

        Ok(rust_i18n::t!(value, locale = preferred_language).to_string())
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
        _ => unreachable!("Exhausted list of subcommands and subcommand_required prevents `None`"),
    };

    Ok(())
}

#[derive(Deserialize)]
struct Serve {
    pub addr: String,
}

pub async fn serve(config_path: impl Into<String>) -> anyhow::Result<()> {
    let config_path = config_path.into();
    let config: Serve = Config::builder()
        .add_source(config::File::with_name(&config_path).required(true))
        .add_source(config::Environment::with_prefix(env!("CARGO_PKG_NAME")))
        .build()?
        .try_deserialize()?;

    let mut app = routes::create_router().layer(Extension(
        shared::UserLanguage::config()
            .add_source(shared::QuerySource::new("lng"))
            .add_source(shared::AcceptLanguageSource),
    ));

    #[cfg(debug_assertions)]
    {
        app = app.layer(tower_livereload::LiveReloadLayer::new());
    }

    let listener = tokio::net::TcpListener::bind(&config.addr).await?;
    tracing::info!("listening on {}", listener.local_addr()?);
    axum::serve(listener, app).await?;

    Ok(())
}
