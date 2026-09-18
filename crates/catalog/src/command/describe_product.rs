use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::ProductDescribed, error::CatalogError};

#[derive(Debug, Clone)]
pub struct DescribeProduct {
    pub long_description: String,
    pub key_features: Vec<String>,
}

impl<E: Executor> super::Command<E> {
    pub async fn describe_product(
        &self,
        id: impl Into<String>,
        cmd: DescribeProduct,
    ) -> Result<(), CatalogError> {
        let product = self.load_active(id).await?;

        product
            .write()?
            .event(&ProductDescribed {
                long_description: cmd.long_description,
                key_features: cmd.key_features,
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
