//! How much disk a tenant's pictures may take (#468).
//!
//! One tenant at the caps (twenty pictures per product, every product) could
//! fill every node's disk; a quota bounds it. The quota is the platform's
//! default (`media.tenant_quota_bytes`) unless the platform set one apart
//! for the tenant (`TenantState::media_quota_bytes`, from its admin page).
//! What a tenant uses is the sum of its files in the `media_files` read
//! model ([`super::read_model`]): what the store knows, the same on every
//! node, rather than what one node's disk holds.
//!
//! The check runs where an upload is taken, once rendered (so the bytes
//! counted are the ones kept), against the read model as it stands: two
//! uploads landing together may overshoot by a picture, and a shop at its
//! quota removes a picture and sees the room a moment later. Imports leave
//! a picture out when there is no room for it.

use timada_tenancy::TenantState;

use crate::db::Db;

/// Judges whether a tenant has room for more pictures.
pub struct Quota {
    db: Db,
    /// The platform's default; `None` for no quota.
    default_bytes: Option<u64>,
}

/// What a tenant may still store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Room {
    Enough,
    /// Not for `wanted` more bytes: what is used and what is allowed.
    Full {
        used: u64,
        limit: u64,
    },
}

/// A tenant's pictures against its quota, for a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub used: u64,
    /// `None` for no quota.
    pub limit: Option<u64>,
    /// Whether the limit is the tenant's own rather than the platform's.
    pub own_limit: bool,
}

impl Quota {
    pub fn new(db: Db, default_bytes: Option<u64>) -> Self {
        Self { db, default_bytes }
    }

    /// The quota `tenant` has: its own when the platform set one, else the
    /// platform's default; `None` for none.
    pub fn limit_of(&self, tenant: &TenantState) -> Option<u64> {
        tenant.media_quota_bytes.or(self.default_bytes)
    }

    /// What `tenant`'s pictures take, as the store knows them.
    pub async fn used_by(&self, tenant_id: &str) -> anyhow::Result<u64> {
        let (used,): (i64,) = self
            .db
            .fetch_one(
                "SELECT coalesce(sum(size), 0) FROM media_files WHERE tenant_id = ?1",
                (tenant_id,),
            )
            .await?;
        Ok(u64::try_from(used).unwrap_or(0))
    }

    /// For a page: what is used, against what is allowed.
    pub async fn usage_of(&self, tenant: &TenantState) -> anyhow::Result<Usage> {
        Ok(Usage {
            used: self.used_by(&tenant.id).await?,
            limit: self.limit_of(tenant),
            own_limit: tenant.media_quota_bytes.is_some(),
        })
    }

    /// Whether `tenant` has room for `wanted` more bytes.
    pub async fn room_for(&self, tenant: &TenantState, wanted: u64) -> anyhow::Result<Room> {
        self.room(&tenant.id, self.limit_of(tenant), wanted).await
    }

    /// Whether the tenant `tenant_id`, allowed `limit` bytes (`None`: no
    /// quota), has room for `wanted` more.
    pub async fn room(
        &self,
        tenant_id: &str,
        limit: Option<u64>,
        wanted: u64,
    ) -> anyhow::Result<Room> {
        let Some(limit) = limit else {
            return Ok(Room::Enough);
        };
        let used = self.used_by(tenant_id).await?;
        Ok(if used.saturating_add(wanted) > limit {
            Room::Full { used, limit }
        } else {
            Room::Enough
        })
    }
}

/// The error code a page words when a tenant's pictures take all the room
/// they may (`crate::ui::say`).
pub const STORAGE_FULL: &str = "storage_full";

/// Bytes for a person: `1.2 GB`, `350 MB`, `12 kB`.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["kB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = "B";
    for next in UNITS {
        if value < 1000.0 {
            break;
        }
        value /= 1000.0;
        unit = next;
    }
    if value >= 100.0 {
        format!("{value:.0} {unit}")
    } else {
        format!("{value:.1} {unit}")
    }
}

#[cfg(test)]
mod tests {
    use super::super::fetcher::tests::fetcher_alone;
    use super::super::read_model::Source;
    use super::super::test_support::{png, rendered};
    use super::*;

    #[test]
    fn bytes_read_like_a_person_writes_them() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(12_300), "12.3 kB");
        assert_eq!(format_bytes(350_000_000), "350 MB");
        assert_eq!(format_bytes(1_200_000_000), "1.2 GB");
        assert_eq!(format_bytes(5 * 1024 * 1024 * 1024), "5.4 GB");
    }

    /// The tenant's own limit stands over the platform's; the room counts
    /// what the store knows of the tenant's files.
    #[tokio::test]
    async fn room_is_what_the_store_knows_against_the_limit() {
        let (_store, fetcher) = fetcher_alone().await;
        let picture = rendered(&png(900, 600, [1, 2, 3, 255])).picture;
        fetcher
            .record("acme", "i1", &picture, Source::Product, "mat", 1)
            .await
            .unwrap();
        let quota = Quota::new(fetcher.db.clone(), Some(picture.bytes() + 10));
        let mut acme = TenantState {
            id: "acme".to_owned(),
            ..TenantState::default()
        };
        assert_eq!(quota.used_by("acme").await.unwrap(), picture.bytes());
        assert_eq!(quota.used_by("zeta").await.unwrap(), 0);
        assert_eq!(quota.room_for(&acme, 10).await.unwrap(), Room::Enough);
        assert_eq!(
            quota.room_for(&acme, 11).await.unwrap(),
            Room::Full {
                used: picture.bytes(),
                limit: picture.bytes() + 10
            }
        );
        let usage = quota.usage_of(&acme).await.unwrap();
        assert_eq!(usage.limit, Some(picture.bytes() + 10));
        assert!(!usage.own_limit);

        // Set apart by the platform: its own limit, over the default.
        acme.media_quota_bytes = Some(picture.bytes() * 10);
        assert_eq!(quota.room_for(&acme, 11).await.unwrap(), Room::Enough);
        assert!(quota.usage_of(&acme).await.unwrap().own_limit);

        // No default and nothing set apart: no quota.
        let none = Quota::new(fetcher.db.clone(), None);
        acme.media_quota_bytes = None;
        assert_eq!(
            none.room_for(&acme, u64::MAX / 2).await.unwrap(),
            Room::Enough
        );
        assert_eq!(none.usage_of(&acme).await.unwrap().limit, None);
    }
}
