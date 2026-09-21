//! Who may do what. An operator has one role, and a role is a fixed set of
//! sections — and, inside the orders section, of kinds of action. It is all
//! in this file: a host that wants other roles changes this table, nothing
//! is configured at run time, and what the table does not name is the
//! owner's alone.

/// What an operator is there for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Everything, the team included.
    Owner,
    /// What is sold: products, families, categories, prices, stock, promotions.
    Catalogue,
    /// Who buys: orders, returns, reviews, questions, customers, e-mails.
    Support,
    /// The books: invoices, refunds, disputes, VAT — and whatever moves money.
    Accounting,
}

impl Role {
    pub const ALL: [Role; 4] = [
        Role::Owner,
        Role::Catalogue,
        Role::Support,
        Role::Accounting,
    ];

    /// As stored.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Catalogue => "catalogue",
            Role::Support => "support",
            Role::Accounting => "accounting",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|role| role.as_str() == value)
    }

    pub fn label(self) -> &'static str {
        match self {
            Role::Owner => "Propriétaire",
            Role::Catalogue => "Catalogue",
            Role::Support => "Service client",
            Role::Accounting => "Comptabilité",
        }
    }

    /// Whether the role's navigation shows the section.
    pub fn opens(self, section: Section) -> bool {
        use Section::*;
        match self {
            Role::Owner => true,
            Role::Catalogue => matches!(
                section,
                Products | Families | Categories | Inventory | Promotions
            ),
            Role::Support => matches!(
                section,
                Orders | Returns | Reviews | Questions | Customers | Emails
            ),
            // Orders too: that is where a payment is refunded from.
            Role::Accounting => matches!(section, Orders | Invoices | Refunds | Disputes | Vat),
        }
    }

    /// Refunding a payment, settling or retrying a refund by hand, recording a
    /// capture, a credit note for a lost dispute. What a *process* refunds —
    /// a cancelled order, a return taken back — is nobody's action.
    pub fn moves_money(self) -> bool {
        matches!(self, Role::Owner | Role::Accounting)
    }

    /// Shipping, cancelling, writing to the customer again.
    pub fn handles_orders(self) -> bool {
        matches!(self, Role::Owner | Role::Support)
    }

    /// Where the role lands after signing in: the first section it opens.
    pub fn home(self) -> Section {
        Section::ALL
            .into_iter()
            .find(|section| self.opens(*section))
            .unwrap_or(Section::Orders)
    }

    /// Whether the role may make this request. `path` is what follows the
    /// mount segment (`orders/abc/refund`); `writes` is any method but GET
    /// and HEAD. A path no section claims is the owner's alone.
    pub fn permits(self, path: &str, writes: bool) -> bool {
        let mut segments = path.trim_matches('/').split('/');
        let Some(section) = segments.next().and_then(Section::of_segment) else {
            return self == Role::Owner;
        };
        if !self.opens(section) {
            return false;
        }
        if section != Section::Orders || !writes {
            return true;
        }
        match OrderAction::of_path(path) {
            OrderAction::Money | OrderAction::Books => self.moves_money(),
            OrderAction::Handling => self.handles_orders(),
        }
    }
}

/// A section of the admin: the first URL segment under the mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Orders,
    Products,
    Categories,
    Families,
    Inventory,
    Customers,
    Promotions,
    Invoices,
    Returns,
    Refunds,
    Disputes,
    Vat,
    Reviews,
    Questions,
    Emails,
}

impl Section {
    /// In the order the navigation shows them.
    pub const ALL: [Section; 15] = [
        Section::Orders,
        Section::Products,
        Section::Categories,
        Section::Families,
        Section::Inventory,
        Section::Customers,
        Section::Promotions,
        Section::Invoices,
        Section::Returns,
        Section::Refunds,
        Section::Disputes,
        Section::Vat,
        Section::Reviews,
        Section::Questions,
        Section::Emails,
    ];

    pub fn segment(self) -> &'static str {
        match self {
            Section::Orders => "orders",
            Section::Products => "products",
            Section::Categories => "categories",
            Section::Families => "families",
            Section::Inventory => "inventory",
            Section::Customers => "customers",
            Section::Promotions => "promotions",
            Section::Invoices => "invoices",
            Section::Returns => "returns",
            Section::Refunds => "refunds",
            Section::Disputes => "disputes",
            Section::Vat => "vat",
            Section::Reviews => "reviews",
            Section::Questions => "questions",
            Section::Emails => "emails",
        }
    }

    pub fn of_segment(segment: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|section| section.segment() == segment)
    }
}

/// What a write inside the orders section is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OrderAction {
    /// Money in or out by an operator's hand.
    Money,
    /// The rate the order enters the books at.
    Books,
    /// Everything else — and whatever is added later, until it is named here.
    Handling,
}

impl OrderAction {
    fn of_path(path: &str) -> Self {
        let path = path.trim_end_matches('/');
        let ends = |suffix: &str| path.ends_with(suffix);
        if ends("/refund")
            || ends("/refunds/retry")
            || ends("/refunds/settle")
            || ends("/capture-payment")
            || ends("/disputes/credit-note")
        {
            OrderAction::Money
        } else if ends("/exchange-rate") {
            OrderAction::Books
        } else {
            OrderAction::Handling
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_are_stored_and_read_back() {
        for role in Role::ALL {
            assert_eq!(Role::parse(role.as_str()), Some(role));
        }
        assert_eq!(Role::parse("root"), None);
    }

    #[test]
    fn every_section_is_somebodys_besides_the_owner() {
        for section in Section::ALL {
            assert!(Role::Owner.opens(section));
            assert!(
                Role::ALL
                    .into_iter()
                    .any(|role| role != Role::Owner && role.opens(section)),
                "{section:?}"
            );
            assert_eq!(Section::of_segment(section.segment()), Some(section));
        }
    }

    #[test]
    fn money_is_moved_by_the_owner_and_accounting_only() {
        for action in [
            "orders/o-1/refund",
            "orders/o-1/refunds/retry",
            "orders/o-1/refunds/settle",
            "orders/o-1/capture-payment",
            "orders/o-1/disputes/credit-note",
            "orders/o-1/exchange-rate",
        ] {
            assert!(Role::Owner.permits(action, true), "{action}");
            assert!(Role::Accounting.permits(action, true), "{action}");
            assert!(!Role::Support.permits(action, true), "{action}");
            assert!(!Role::Catalogue.permits(action, true), "{action}");
        }
        // Support reads the order and what was refunded; accounting does not ship.
        assert!(Role::Support.permits("orders/o-1", false));
        assert!(Role::Accounting.permits("orders/o-1", false));
        for action in ["orders/o-1/ship", "orders/o-1/cancel", "orders/o-1/resend"] {
            assert!(Role::Support.permits(action, true), "{action}");
            assert!(!Role::Accounting.permits(action, true), "{action}");
        }
    }

    #[test]
    fn a_section_is_closed_to_the_roles_that_do_not_open_it() {
        assert!(Role::Catalogue.permits("products/new", true));
        assert!(!Role::Catalogue.permits("orders", false));
        assert!(!Role::Support.permits("products", false));
        assert!(!Role::Support.permits("vat/oss.csv", false));
        assert!(Role::Accounting.permits("vat/oss.csv", false));
        assert!(!Role::Accounting.permits("customers", false));
        assert_eq!(Role::Catalogue.home(), Section::Products);
        assert_eq!(Role::Accounting.home(), Section::Orders);
    }

    #[test]
    fn what_no_section_claims_is_the_owners_alone() {
        for path in ["team", "journal", "", "orders-export"] {
            assert!(Role::Owner.permits(path, false), "{path}");
            for role in [Role::Catalogue, Role::Support, Role::Accounting] {
                assert!(!role.permits(path, false), "{role:?} {path}");
            }
        }
    }
}
