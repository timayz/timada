use crate::connector::ConnectorError;

#[derive(Debug, thiserror::Error)]
pub enum SourcingError {
    #[error("supplier not found")]
    SupplierNotFound,
    #[error("a supplier is already registered as `{0}`")]
    AlreadyRegistered(String),
    #[error("supplier is suspended")]
    SupplierSuspended,
    #[error("product is not sourced from any supplier")]
    NotSourced,
    #[error("the selling price is locked: {0}")]
    PriceLocked(String),
    #[error("no connector named `{0}`")]
    ConnectorUnknown(String),
    #[error("connector `{key}` does not {task}")]
    ConnectorDoes { key: String, task: &'static str },
    #[error("`{0}` is required")]
    Required(&'static str),
    #[error("`{0}` is not a currency code")]
    InvalidCurrency(String),
    #[error(transparent)]
    Connector(#[from] ConnectorError),
    #[error(transparent)]
    Rate(#[from] timada_tax::RateError),
    #[error(transparent)]
    Money(#[from] timada_core::MoneyError),
    #[error(transparent)]
    Pricing(#[from] timada_pricing::PricingError),
    #[error(transparent)]
    Sql(#[from] sqlx::Error),
    #[error(transparent)]
    Write(#[from] evento::WriteError),
    #[error(transparent)]
    Server(#[from] anyhow::Error),
}
