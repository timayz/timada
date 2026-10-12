//! Permission checks inside a tenant, evaluated by casbin (RBAC with domains:
//! the tenant is the domain, its custom roles the roles).
//!
//! The policies of a tenant are not loaded at startup or fed by a
//! subscription: every tenant-scoped request already loads the strict
//! `TenantState`, whose `aggregate_version` says exactly which events it
//! reflects. The enforcer keeps, per tenant, the version it was synced from
//! and re-syncs when a newer state comes along, so a role edit applies on the
//! very next request. Owners bypass casbin: they hold every permission.
//! `manage` on a resource is stored as both `manage` and `read` rules, the
//! domain's implication (`timada_tenancy::implies`) made explicit.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use casbin::prelude::*;
use timada_tenancy::{TenantState, split_permission};
use tokio::sync::RwLock;

/// `p = role, tenant, resource, action`; `g = user, role, tenant`.
const MODEL: &str = r#"
[request_definition]
r = sub, dom, obj, act

[policy_definition]
p = sub, dom, obj, act

[role_definition]
g = _, _, _

[policy_effect]
e = some(where (p.eft == allow))

[matchers]
m = g(r.sub, p.sub, r.dom) && r.dom == p.dom && r.obj == p.obj && r.act == p.act
"#;

pub struct Authorizer {
    inner: RwLock<Inner>,
}

struct Inner {
    enforcer: Enforcer,
    /// `tenant id → aggregate_version` the enforcer's rules reflect.
    synced: HashMap<String, u16>,
}

impl Authorizer {
    pub async fn new() -> Result<Arc<Self>> {
        let model = DefaultModel::from_str(MODEL)
            .await
            .context("cannot parse the permission model")?;
        let enforcer = Enforcer::new(model, MemoryAdapter::default())
            .await
            .context("cannot build the permission enforcer")?;
        Ok(Arc::new(Self {
            inner: RwLock::new(Inner {
                enforcer,
                synced: HashMap::new(),
            }),
        }))
    }

    /// Whether `user_id` holds `permission` (`resource:action`) in `tenant`.
    /// Unknown permissions are never held.
    pub async fn allows(
        &self,
        tenant: &TenantState,
        user_id: &str,
        permission: &str,
    ) -> Result<bool> {
        if tenant.is_owner(user_id) {
            return Ok(true);
        }
        let Some((resource, action)) = split_permission(permission) else {
            return Ok(false);
        };
        {
            let inner = self.inner.read().await;
            if inner.synced.get(&tenant.id) == Some(&tenant.aggregate_version) {
                return Ok(inner.enforcer.enforce((
                    user_id,
                    tenant.id.as_str(),
                    resource,
                    action,
                ))?);
            }
        }
        let mut inner = self.inner.write().await;
        // Another request may have synced while this one waited.
        if inner.synced.get(&tenant.id) != Some(&tenant.aggregate_version) {
            inner.sync(tenant).await?;
        }
        Ok(inner
            .enforcer
            .enforce((user_id, tenant.id.as_str(), resource, action))?)
    }
}

impl Inner {
    /// Replaces every rule of the tenant's domain with the state's roles and
    /// members.
    async fn sync(&mut self, tenant: &TenantState) -> Result<()> {
        let dom = tenant.id.as_str();
        self.enforcer
            .remove_filtered_policy(1, vec![dom.to_owned()])
            .await?;
        self.enforcer
            .remove_filtered_grouping_policy(2, vec![dom.to_owned()])
            .await?;
        let mut policies: Vec<Vec<String>> = Vec::new();
        for (role, def) in &tenant.roles {
            for (resource, action) in def.permissions.iter().filter_map(|p| split_permission(p)) {
                let rule = |action: &str| {
                    vec![
                        role.clone(),
                        dom.to_owned(),
                        resource.to_owned(),
                        action.to_owned(),
                    ]
                };
                policies.push(rule(action));
                if action == "manage" && !def.permissions.contains(&format!("{resource}:read")) {
                    policies.push(rule("read"));
                }
            }
        }
        if !policies.is_empty() {
            self.enforcer.add_policies(policies).await?;
        }
        let groups: Vec<Vec<String>> = tenant
            .members
            .iter()
            .map(|(user, role)| vec![user.clone(), role.clone(), dom.to_owned()])
            .collect();
        if !groups.is_empty() {
            self.enforcer.add_grouping_policies(groups).await?;
        }
        self.synced
            .insert(tenant.id.clone(), tenant.aggregate_version);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use timada_tenancy::{
        PERMISSION_MEMBERS_MANAGE, PERMISSION_MEMBERS_READ, PERMISSION_ROLES_MANAGE, ROLE_OWNER,
        RoleDef,
    };

    use super::*;

    fn tenant(version: u16) -> TenantState {
        let mut tenant = TenantState {
            id: "t1".to_owned(),
            aggregate_version: version,
            ..TenantState::default()
        };
        tenant
            .members
            .insert("alice".to_owned(), ROLE_OWNER.to_owned());
        tenant
            .members
            .insert("bob".to_owned(), "support".to_owned());
        tenant.roles.insert(
            "support".to_owned(),
            RoleDef {
                label: "Support".to_owned(),
                permissions: [PERMISSION_MEMBERS_READ.to_owned()].into_iter().collect(),
            },
        );
        tenant
    }

    #[tokio::test]
    async fn roles_grant_permissions_and_owners_hold_everything() {
        let authz = Authorizer::new().await.unwrap();
        let t = tenant(3);
        assert!(
            authz
                .allows(&t, "alice", PERMISSION_ROLES_MANAGE)
                .await
                .unwrap()
        );
        assert!(
            authz
                .allows(&t, "bob", PERMISSION_MEMBERS_READ)
                .await
                .unwrap()
        );
        assert!(
            !authz
                .allows(&t, "bob", PERMISSION_MEMBERS_MANAGE)
                .await
                .unwrap()
        );
        assert!(
            !authz
                .allows(&t, "carol", PERMISSION_MEMBERS_READ)
                .await
                .unwrap()
        );
        assert!(!authz.allows(&t, "bob", "members:delete").await.unwrap());
        assert!(!authz.allows(&t, "bob", "nonsense").await.unwrap());
    }

    #[tokio::test]
    async fn manage_implies_read() {
        let authz = Authorizer::new().await.unwrap();
        let mut t = tenant(1);
        t.roles.get_mut("support").unwrap().permissions =
            [PERMISSION_MEMBERS_MANAGE.to_owned()].into_iter().collect();
        assert!(
            authz
                .allows(&t, "bob", PERMISSION_MEMBERS_READ)
                .await
                .unwrap()
        );
        assert!(
            authz
                .allows(&t, "bob", PERMISSION_MEMBERS_MANAGE)
                .await
                .unwrap()
        );
        assert!(
            !authz
                .allows(&t, "bob", PERMISSION_ROLES_MANAGE)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn a_newer_state_replaces_the_rules_and_an_older_one_is_ignored() {
        let authz = Authorizer::new().await.unwrap();
        let t = tenant(3);
        assert!(
            authz
                .allows(&t, "bob", PERMISSION_MEMBERS_READ)
                .await
                .unwrap()
        );

        // Version 4: the role gained `members:manage` and bob was revoked,
        // carol joined as support.
        let mut newer = tenant(4);
        newer
            .roles
            .get_mut("support")
            .unwrap()
            .permissions
            .insert(PERMISSION_MEMBERS_MANAGE.to_owned());
        newer.members.remove("bob");
        newer
            .members
            .insert("carol".to_owned(), "support".to_owned());
        assert!(
            authz
                .allows(&newer, "carol", PERMISSION_MEMBERS_MANAGE)
                .await
                .unwrap()
        );
        assert!(
            !authz
                .allows(&newer, "bob", PERMISSION_MEMBERS_READ)
                .await
                .unwrap()
        );

        // The same version again is served from the synced rules.
        assert!(
            authz
                .allows(&newer, "carol", PERMISSION_MEMBERS_READ)
                .await
                .unwrap()
        );
        assert_eq!(authz.inner.read().await.synced["t1"], 4);

        // Two tenants do not share rules.
        let mut other = tenant(1);
        other.id = "t2".to_owned();
        other.members.remove("bob");
        assert!(
            !authz
                .allows(&other, "bob", PERMISSION_MEMBERS_READ)
                .await
                .unwrap()
        );
        assert!(
            !authz
                .allows(&other, "carol", PERMISSION_MEMBERS_READ)
                .await
                .unwrap()
        );
        assert!(
            authz
                .allows(&newer, "carol", PERMISSION_MEMBERS_READ)
                .await
                .unwrap()
        );
    }
}
