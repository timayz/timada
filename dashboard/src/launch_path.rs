//! A shop's launch path: the things to get right between "a shop with a
//! name" and "a shop that is advertised", in their order, each done, to
//! do, or not checked.
//!
//! A step reads what timada already holds: the study linked to the shop,
//! its categories and products, its company and terms, its connections to
//! Google, its campaign. Where timada cannot know (a legal page's content,
//! a phone that answers, a page's speed on a phone), the step asks a
//! member to say so, and keeps who did (`timada-launch`'s
//! `StepConfirmed`). Nothing is fetched from outside to draw the page.
//!
//! The steps that must hold before ads start are marked
//! ([`Step::before_ads`]): the first campaign is not prepared while one of
//! them does not ([`blocking`], read by [`crate::campaigns::ready`]).

use std::collections::{BTreeMap, BTreeSet};

use timada_catalog::{CategoryRow, ProductFilter, ProductStatus};
use timada_google::{
    ACCESS_MERCHANT_CENTER, ACCESS_SEARCH_CONSOLE, CAMPAIGN_SHOPPING, merchant_standing,
};
use timada_launch::{
    LaunchState, STEP_CONSENT, STEP_CONTACT, STEP_LEGAL, STEP_SITEMAP, STEP_SITEMAP_SUBMITTED,
    STEP_SPEED, STEP_SUPPLIER_TEXT, STEP_TEST_ORDER,
};
use timada_proposals::{KIND_ADS_CAMPAIGN, waiting_about};
use timada_tenancy::TenantState;

use crate::campaigns::held;
use crate::cluster::Executor;
use crate::db::Db;

/// How many feed products one read takes when the margins are looked at.
const MARGIN_PAGE: i64 = 500;

/// How many names a step's detail lists before it says "and n more".
const NAMED: usize = 5;

/// Where a step stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Done,
    /// What timada reads says it is not done.
    ToDo,
    /// What timada reads holds, or says nothing: a member has to look and
    /// say so.
    NotChecked,
}

impl State {
    pub fn key(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::ToDo => "todo",
            Self::NotChecked => "unchecked",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Done => "Done",
            Self::ToDo => "To do",
            Self::NotChecked => "Not checked",
        }
    }
}

/// Where a step is put right.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fix {
    /// A section of the shop, by its key in the rail.
    Section(&'static str),
    /// The niche studies, or the one linked to the shop.
    Study(Option<String>),
    /// The page of the campaigns.
    Campaigns,
    /// A page outside timada.
    Outside(String),
}

/// One step of the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// What a test finds it by, and what a member's word is kept under.
    pub key: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    pub state: State,
    /// What was found, in a sentence.
    pub detail: String,
    /// Where it is put right, and what the link says.
    pub fix: Option<(&'static str, Fix)>,
    /// It must hold before the first campaign is prepared.
    pub before_ads: bool,
    /// A member says it holds: timada cannot know.
    pub confirms: bool,
    /// Who said so, and when (unix seconds).
    pub confirmed: Option<(String, u64)>,
}

/// A step timada checks by itself: done when `missing` is `None`.
fn checked(
    key: &'static str,
    group: &'static str,
    label: &'static str,
    missing: Option<String>,
    done: &str,
) -> Step {
    Step {
        key,
        group,
        label,
        state: if missing.is_none() {
            State::Done
        } else {
            State::ToDo
        },
        detail: missing.unwrap_or_else(|| done.to_owned()),
        fix: None,
        before_ads: false,
        confirms: false,
        confirmed: None,
    }
}

/// A step a member confirms. `missing` is what timada can tell is not
/// there yet, which a member's word does not make up for; `asks` says what
/// to look at.
fn confirmed(
    launch: Option<&LaunchState>,
    key: &'static str,
    group: &'static str,
    label: &'static str,
    missing: Option<String>,
    asks: &str,
) -> Step {
    let word = launch
        .and_then(|launch| launch.confirmed(key))
        .map(|(by, at)| (by.to_owned(), at));
    Step {
        key,
        group,
        label,
        state: match (&missing, &word) {
            (Some(_), _) => State::ToDo,
            (None, Some(_)) => State::Done,
            (None, None) => State::NotChecked,
        },
        detail: missing.unwrap_or_else(|| asks.to_owned()),
        fix: None,
        before_ads: false,
        confirms: true,
        confirmed: word,
    }
}

impl Step {
    fn fix(mut self, says: &'static str, fix: Fix) -> Self {
        self.fix = Some((says, fix));
        self
    }

    fn before_ads(mut self) -> Self {
        self.before_ads = true;
        self
    }
}

/// `names`, the first few of them, then how many more.
fn listed(names: &[String]) -> String {
    let shown = names
        .iter()
        .take(NAMED)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    match names.len().saturating_sub(NAMED) {
        0 => shown,
        more => format!("{shown} and {more} more"),
    }
}

/// The categories shoppers see that have none under them and hold fewer
/// than `least` products, by name.
pub fn thin_leaves(categories: &[CategoryRow], least: i64) -> Vec<String> {
    let live: Vec<&CategoryRow> = categories
        .iter()
        .filter(|category| !category.archived && !category.draft)
        .collect();
    let parents: BTreeSet<&str> = live
        .iter()
        .filter_map(|category| category.parent_id.as_deref())
        .collect();
    live.iter()
        .filter(|category| !parents.contains(category.id.as_str()))
        .filter(|category| category.product_count < least)
        .map(|category| format!("{} ({})", category.name, category.product_count))
        .collect()
}

/// The categories that hold exactly the same products as another, in
/// groups: each group is the ids of categories no shopper tells apart.
/// `filed` is every (category, product) of the shop.
pub fn twins(filed: &[(String, String)]) -> Vec<Vec<String>> {
    let mut held: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (category, product) in filed {
        held.entry(category).or_default().insert(product);
    }
    let mut by_products: BTreeMap<&BTreeSet<&str>, Vec<String>> = BTreeMap::new();
    for (category, products) in &held {
        by_products
            .entry(products)
            .or_default()
            .push((*category).to_owned());
    }
    by_products
        .into_values()
        .filter(|categories| categories.len() > 1)
        .collect()
}

/// The steps of the shop's launch path, in their order, as they stand.
pub async fn path(store: &Executor, db: &Db, tenant: &TenantState) -> anyhow::Result<Vec<Step>> {
    let tenant_id = tenant.id.as_str();
    let launch = timada_launch::Command::new(store.clone(), tenant_id)
        .load_launch()
        .await?;
    let launch = launch.as_ref();
    let (per_category, products_wanted) = launch.map(LaunchState::path_targets).unwrap_or((
        timada_launch::DEFAULT_PRODUCTS_PER_CATEGORY,
        timada_launch::DEFAULT_PRODUCTS,
    ));
    let mut steps = Vec::new();

    // Niche.
    let study = timada_research::study_of_shop(db, tenant_id).await?;
    steps.push(
        checked(
            "study",
            "Niche",
            "A niche study says go",
            match &study {
                None => Some("No study is linked to the shop.".to_owned()),
                Some(study) if study.verdict == "go" => None,
                Some(study) if study.verdict == "no_go" => {
                    Some(format!("The study « {} » says no-go.", study.name))
                }
                Some(study) => Some(format!(
                    "The study « {} » is not complete: it has no verdict yet.",
                    study.name
                )),
            },
            "The study linked to the shop says go.",
        )
        .fix(
            "Open the study",
            Fix::Study(study.as_ref().map(|study| study.id.clone())),
        ),
    );

    // Categories.
    let categories = timada_catalog::categories_of_tenant(db, tenant_id).await?;
    let shown: Vec<&CategoryRow> = categories
        .iter()
        .filter(|category| !category.archived && !category.draft)
        .collect();
    steps.push(
        checked(
            "categories",
            "Categories",
            "The shop has its categories",
            shown
                .is_empty()
                .then(|| "No category is shown to shoppers yet.".to_owned()),
            &format!("{} categories are shown to shoppers.", shown.len()),
        )
        .fix("Categories", Fix::Section("categories")),
    );
    let thin = thin_leaves(&categories, i64::from(per_category));
    steps.push(
        checked(
            "category_products",
            "Categories",
            "Every category at the bottom of the tree holds products",
            if shown.is_empty() {
                Some("There is no category to fill yet.".to_owned())
            } else if thin.is_empty() {
                None
            } else {
                Some(format!(
                    "Fewer than {per_category} products in: {}.",
                    listed(&thin)
                ))
            },
            &format!("Each holds {per_category} products or more."),
        )
        .fix("Categories", Fix::Section("categories")),
    );
    let filed: Vec<(String, String)> = db
        .fetch_all(
            "SELECT category_id, product_id FROM catalog_product_categories
             WHERE tenant_id = ?1",
            (tenant_id,),
        )
        .await?;
    let name_of = |id: &str| {
        categories
            .iter()
            .find(|category| category.id == id)
            .map_or_else(|| id.to_owned(), |category| category.name.clone())
    };
    let same: Vec<String> = twins(&filed)
        .iter()
        .map(|group| {
            group
                .iter()
                .map(|id| name_of(id))
                .collect::<Vec<_>>()
                .join(" = ")
        })
        .collect();
    steps.push(
        checked(
            "category_overlap",
            "Categories",
            "No two categories hold the same products",
            (!same.is_empty()).then(|| format!("The same products in: {}.", listed(&same))),
            "Every category holds products of its own.",
        )
        .fix("Categories", Fix::Section("categories")),
    );

    // Catalog.
    let published = timada_catalog::count_products(
        db,
        tenant_id,
        &ProductFilter::default().status(Some(ProductStatus::Published)),
    )
    .await?;
    steps.push(
        checked(
            "product_count",
            "Catalog",
            "The catalog is as large as planned",
            (published < i64::from(products_wanted)).then(|| {
                format!("{published} products are published, of the {products_wanted} planned.")
            }),
            &format!("{published} products are published."),
        )
        .fix("Products", Fix::Section("products")),
    );
    let mut clashes = Vec::new();
    for category in &shown {
        let taken: Option<(String,)> = db
            .fetch_optional(
                "SELECT name FROM catalog_products WHERE tenant_id = ?1 AND slug = ?2",
                (tenant_id, category.slug.as_str()),
            )
            .await?;
        if taken.is_some() {
            clashes.push(category.slug.clone());
        }
    }
    steps.push(
        checked(
            "slug_clash",
            "Catalog",
            "No product has the address of a category",
            (!clashes.is_empty()).then(|| {
                format!(
                    "A product and a category share the address: {}.",
                    listed(&clashes)
                )
            }),
            "No product and category share an address.",
        )
        .fix("Products", Fix::Section("products"))
        .before_ads(),
    );
    // Every thing on sale: what it leaves, when its cost is known.
    let costs = timada_catalog::load_sale_costs(store, tenant_id).await?;
    let vat = crate::margins::home_vat(crate::margins::tax_profile(store, tenant).await?);
    let (mut on_sale, mut unknown, mut losing) = (0usize, Vec::new(), Vec::new());
    let mut after: Option<String> = None;
    loop {
        let page =
            timada_catalog::feed_products(db, tenant_id, after.as_deref(), MARGIN_PAGE).await?;
        for product in &page {
            let margins = timada_catalog::item_margins(product, &vat, costs.costs.as_ref());
            on_sale += margins.len();
            if margins.iter().any(Option::is_none) {
                unknown.push(product.name.clone());
            } else if margins
                .iter()
                .flatten()
                .any(|margin| margin.margin_cents() <= 0)
            {
                losing.push(product.name.clone());
            }
        }
        match page.last() {
            Some(last) if page.len() as i64 == MARGIN_PAGE => after = Some(last.id.clone()),
            _ => break,
        }
    }
    steps.push(
        checked(
            "margins",
            "Catalog",
            "Every product leaves something, and its break-even is known",
            if on_sale == 0 {
                Some("No product is on sale yet.".to_owned())
            } else if !unknown.is_empty() {
                Some(format!(
                    "The cost is not known of: {}. Without it there is no break-even.",
                    listed(&unknown)
                ))
            } else if !losing.is_empty() {
                Some(format!("Sold at a loss: {}.", listed(&losing)))
            } else {
                None
            },
            "Every product on sale leaves something once its costs are paid.",
        )
        .fix("Costs", Fix::Section("costs"))
        .before_ads(),
    );
    steps.push(
        confirmed(
            launch,
            STEP_SUPPLIER_TEXT,
            "Catalog",
            "No product is left with its supplier's text",
            None,
            "Read the names and descriptions of what was imported: a text copied from a supplier is on a hundred other shops.",
        )
        .fix("Products", Fix::Section("products")),
    );

    // Shop.
    let company = timada_tenancy::Command(store.clone())
        .load_company_of(tenant)
        .await?;
    steps.push(
        checked(
            "company",
            "Shop",
            "The company's details are filled",
            company
                .is_none()
                .then(|| "The shop has no company with its legal identity.".to_owned()),
            "The shop's company has its legal identity.",
        )
        .fix("Invoicing", Fix::Section("invoicing"))
        .before_ads(),
    );
    let pages = timada_pages::pages_of_tenant(db, tenant_id).await?;
    let written = pages.iter().filter(|page| page.published).count();
    steps.push(
        confirmed(
            launch,
            STEP_LEGAL,
            "Shop",
            "The legal pages are written for this company",
            (written == 0).then(|| "No page of the shop is published.".to_owned()),
            &format!(
                "{written} pages are published. Read the legal notice, the terms of sale and the privacy policy: they have to say what is true of this company."
            ),
        )
        .fix("Pages", Fix::Section("pages"))
        .before_ads(),
    );
    steps.push(
        confirmed(
            launch,
            STEP_CONSENT,
            "Shop",
            "The storefront asks before it measures",
            None,
            "Open the storefront in a private window: the consent banner shows, and nothing is measured before an answer.",
        )
        .before_ads(),
    );
    steps.push(confirmed(
        launch,
        STEP_CONTACT,
        "Shop",
        "Shoppers have a way to reach the shop that answers",
        None,
        "Send the contact form, write to the address and call the number the shop gives: each has to reach someone.",
    ));
    let orders = timada_orders::Command::new(store.clone(), tenant_id);
    let ships = !orders.shipping_methods().await?.is_empty();
    let returns = orders.return_policy().await?;
    steps.push(
        checked(
            "shipping_returns",
            "Shop",
            "Delivery and returns are stated",
            match (ships, &returns) {
                (false, _) => Some("The shop has no shipping method.".to_owned()),
                (true, None) => Some("The shop does not say how returns go.".to_owned()),
                (true, Some(_)) => None,
            },
            "The shipping methods and the return terms are set: the storefront's pages and what it tells search engines are written from them.",
        )
        .fix("Shipping", Fix::Section("shipping"))
        .before_ads(),
    );

    // Found.
    let address = orders.storefront_address().await?;
    steps.push(
        confirmed(
            launch,
            STEP_SITEMAP,
            "Found",
            "The sitemap and the robots file answer",
            address
                .is_none()
                .then(|| "The shop does not say where shoppers find it.".to_owned()),
            &match &address {
                Some(address) => format!(
                    "Open {address}/sitemap.xml and {address}/robots.txt: both have to answer."
                ),
                None => String::new(),
            },
        )
        .fix("API keys", Fix::Section("api_keys")),
    );
    let connection = timada_google::Command(store.clone())
        .load_connection(tenant_id)
        .await?;
    let site = connection
        .as_ref()
        .filter(|connection| connection.reads(ACCESS_SEARCH_CONSOLE))
        .and_then(|connection| connection.site.clone());
    steps.push(
        confirmed(
            launch,
            STEP_SITEMAP_SUBMITTED,
            "Found",
            "Search Console is connected and has the sitemap",
            site.is_none().then(|| {
                "Search Console is not connected, or the shop's site is not chosen.".to_owned()
            }),
            "Search Console is connected. Give it the sitemap's address, under Sitemaps.",
        )
        .fix("Google account", Fix::Section("google")),
    );

    // Measured.
    let tag = timada_pages::Command::new(store.clone(), tenant_id)
        .load_google_tag()
        .await?
        .map(|tag| tag.settings);
    let counted = tag
        .as_ref()
        .is_some_and(|tag| tag.ads_id.is_some() && tag.purchase_label.is_some());
    steps.push(
        checked(
            "tag",
            "Measured",
            "The Google tag is set and counts the sales",
            (!counted).then(|| {
                "The Google tag has no Google Ads id, or no label for its purchase conversion."
                    .to_owned()
            }),
            "The Google tag has its Google Ads id and its purchase conversion.",
        )
        .fix("Google tag", Fix::Section("google_tag"))
        .before_ads(),
    );
    let ordered = timada_orders::order_count(db, tenant_id).await? > 0;
    steps.push(
        confirmed(
            launch,
            STEP_TEST_ORDER,
            "Measured",
            "A real test order went through, its purchase counted",
            None,
            if ordered {
                "Place an order as a shopper would, pay it, and see the purchase arrive in Google Ads' conversions."
            } else {
                "No order was placed on the shop yet. Place one as a shopper would, pay it, and see the purchase arrive in Google Ads' conversions."
            },
        )
        .fix("Orders", Fix::Section("orders"))
        .before_ads(),
    );

    // Speed.
    steps.push({
        let step = confirmed(
            launch,
            STEP_SPEED,
            "Speed",
            "The home, a category and a product pass Core Web Vitals on a phone",
            None,
            "A new shop has no field data: run the three pages through PageSpeed Insights, on mobile.",
        );
        match &address {
            Some(address) => step.fix(
                "PageSpeed Insights",
                Fix::Outside(format!(
                    "https://pagespeed.web.dev/analysis?url={address}&form_factor=mobile"
                )),
            ),
            None => step,
        }
    });

    // Feed.
    let merchant = connection
        .as_ref()
        .filter(|connection| connection.reads(ACCESS_MERCHANT_CENTER))
        .and_then(|connection| connection.merchant.clone());
    let standing = match &merchant {
        Some(merchant) => merchant_standing(db, tenant_id, &merchant.account_id).await?,
        None => None,
    };
    steps.push(
        checked(
            "feed",
            "Feed",
            "Merchant Center accepts every product",
            match (&merchant, &standing) {
                (None, _) => Some("Merchant Center is not connected.".to_owned()),
                (_, None) => Some("Merchant Center was not read yet.".to_owned()),
                (_, Some(standing)) if !standing.all_accepted_for_ads() => Some(format!(
                    "{} refused, {} still looked at, {} not sent there.",
                    standing.ads.refused, standing.ads.pending, standing.ads.absent
                )),
                _ => None,
            },
            "Every product is accepted for Shopping ads.",
        )
        .fix("Merchant Center", Fix::Section("merchant_center"))
        .before_ads(),
    );

    // Ads.
    let campaign = held(store, tenant_id, CAMPAIGN_SHOPPING).await?;
    let waits = match campaign {
        Some(_) => false,
        None => waiting_about(db, tenant_id, KIND_ADS_CAMPAIGN, CAMPAIGN_SHOPPING)
            .await?
            .is_some(),
    };
    steps.push(
        checked(
            "campaign_prepared",
            "Ads",
            "The first campaign is prepared",
            (campaign.is_none() && !waits)
                .then(|| "The first campaign is not prepared yet.".to_owned()),
            if campaign.is_some() {
                "The first campaign was created at Google Ads."
            } else {
                "The first campaign is prepared and waits for an answer."
            },
        )
        .fix("Campaigns", Fix::Campaigns),
    );
    let live = campaign.as_ref().and_then(|campaign| campaign.live_at);
    steps.push(
        checked(
            "campaign_live",
            "Ads",
            "The first campaign runs",
            live.is_none()
                .then(|| "The first campaign does not run yet.".to_owned()),
            &live.map_or_else(String::new, |at| {
                format!("Live since {}.", timada_core::civil::format_date(at))
            }),
        )
        .fix("Campaigns", Fix::Campaigns),
    );
    Ok(steps)
}

/// The steps that must hold before ads start and do not, by label.
pub fn blocking(steps: &[Step]) -> Vec<&'static str> {
    steps
        .iter()
        .filter(|step| step.before_ads && step.state != State::Done)
        .map(|step| step.label)
        .collect()
}

#[cfg(test)]
mod tests;
