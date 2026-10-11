//! The topcoat router. This module maps to `/`; child modules add paths.

use topcoat::{
    Result,
    asset::{AssetBundle, RouterBuilderAssetExt},
    router::{Router, RouterBuilderDiscoverExt, module_router, page},
    runtime::RouterBuilderRuntimeExt,
    tailwind,
    view::{View, view},
};

use crate::cluster::Executor;
use crate::config::Config;
use crate::db::{ReadPool, WritePool};

pub fn router(
    executor: Executor,
    read_pool: ReadPool,
    write_pool: WritePool,
    config: Config,
) -> Router {
    // The bundle sits next to the binary; `topcoat dev` writes it after every
    // build. A bare `cargo run` has none, and rendering an asset without it
    // panics with a less helpful message than this one.
    let assets = AssetBundle::load().expect(
        "asset bundle missing: run `topcoat dev` or `topcoat asset bundle -p timada-dashboard`",
    );

    module_router!()
        .discover()
        .app_context(executor)
        .app_context(read_pool)
        .app_context(write_pool)
        .app_context(config)
        .assets(assets)
        .runtime()
        .build()
}

#[page]
async fn home() -> Result<impl View> {
    Ok(view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>"Timada dashboard"</title>
                <link rel="stylesheet" href=(tailwind::stylesheet!())>
                topcoat::dev::script()
            </head>
            <body class="min-h-screen bg-neutral-50 text-neutral-900">
                <main class="mx-auto max-w-3xl p-8">
                    <h1 class="text-2xl font-semibold">"Timada dashboard"</h1>
                </main>
            </body>
        </html>
    })
}
