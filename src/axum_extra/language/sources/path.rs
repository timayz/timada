use async_trait::async_trait;
use axum::{extract::Path, http::request::Parts, RequestPartsExt};
use std::collections::HashMap;

use crate::axum_extra::UserLanguageSource;

/// TBD
#[derive(Debug, Clone)]
pub struct PathSource {
    /// TBD
    name: String,
}

impl PathSource {
    /// TBD
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

#[async_trait]
impl UserLanguageSource for PathSource {
    async fn languages_from_parts(&self, parts: &mut Parts) -> Vec<String> {
        let Ok(path) = parts.extract::<Path<HashMap<String, String>>>().await else {
            return vec![];
        };

        let Some(lang) = path.get(self.name.as_str()) else {
            return vec![];
        };

        vec![lang.to_string()]
    }
}
