fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    tracing::info!("demo-store scaffold — serve/migrate/seed CLI lands with the app wiring step");
    Ok(())
}
