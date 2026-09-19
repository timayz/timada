//! What timada persists is positional bitcode: an event keeps the layout it
//! was written with forever, and so does every value type nested in it. This
//! crate reads the sources of `crates/*`, writes those shapes down in
//! `events.lock`, and refuses any change to a line that is already there —
//! the only way forward is a new variant, which is a new line.
//!
//! Three kinds of lines:
//!
//! - `event <aggregate>::<Variant>` — a variant of an `#[evento::aggregate]`
//!   enum. Frozen: never changed, renamed or removed.
//! - `type <crate>::<Name>` — an `Encode` type reachable from an event. Frozen
//!   too, enums included: bitcode packs the discriminant, so even a variant
//!   appended at the end changes how the old ones decode.
//! - `view <crate>::<Name> rev=<n>` — a snapshotted projection, with the
//!   `Encode` types only it uses. May change, provided `.revision(n)` grows so
//!   the snapshots taken with the old shape are dropped.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::{Path, PathBuf},
};

use anyhow::Context;
use quote::ToTokens;
use syn::visit::Visit;

const HEADER: &str = "\
# Shapes timada persists. Generated: `cargo run -p timada-event-lock -- update`.
# `event` and `type` lines are frozen — add a new variant instead of editing
# one. A `view` line may change when its `.revision(n)` grows.
# See docs/event-evolution.md.
";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Event,
    Type,
    View,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Event => "event",
            Kind::Type => "type",
            Kind::View => "view",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub kind: Kind,
    /// `.revision(n)` of a view's projection; `None` for events and types.
    pub revision: Option<u32>,
    pub shape: String,
}

/// The persisted shapes, keyed by `<kind> <name>`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Lock {
    entries: BTreeMap<String, Entry>,
}

impl Lock {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, key: &str) -> Option<&Entry> {
        self.entries.get(key)
    }

    fn insert(&mut self, kind: Kind, name: &str, revision: Option<u32>, shape: String) {
        self.entries.insert(
            format!("{} {name}", kind.as_str()),
            Entry {
                kind,
                revision,
                shape,
            },
        );
    }

    pub fn render(&self) -> String {
        let mut out = String::from(HEADER);
        let mut previous = None;
        // Events first, then the types they freeze, then the views.
        let mut lines: Vec<(&String, &Entry)> = self.entries.iter().collect();
        lines.sort_by_key(|(key, entry)| (entry.kind, (*key).clone()));
        for (key, entry) in lines {
            if previous != Some(entry.kind) {
                out.push('\n');
                previous = Some(entry.kind);
            }
            match entry.revision {
                Some(revision) => out.push_str(&format!("{key} rev={revision} {}\n", entry.shape)),
                None => out.push_str(&format!("{key} {}\n", entry.shape)),
            }
        }
        out
    }

    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let mut lock = Lock::default();
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let malformed = || anyhow::anyhow!("malformed events.lock line: `{line}`");
            let (kind, rest) = line.split_once(' ').ok_or_else(malformed)?;
            let kind = match kind {
                "event" => Kind::Event,
                "type" => Kind::Type,
                "view" => Kind::View,
                _ => return Err(malformed()),
            };
            let (name, rest) = rest.split_once(' ').ok_or_else(malformed)?;
            let (revision, shape) = match kind {
                Kind::View => {
                    let (revision, shape) = rest.split_once(' ').ok_or_else(malformed)?;
                    let revision = revision
                        .strip_prefix("rev=")
                        .and_then(|r| r.parse().ok())
                        .ok_or_else(malformed)?;
                    (Some(revision), shape)
                }
                _ => (None, rest),
            };
            lock.insert(kind, name, revision, shape.to_owned());
        }
        Ok(lock)
    }
}

/// What is wrong between the committed lock and the sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// A frozen shape was edited.
    Changed {
        key: String,
        locked: String,
        current: String,
    },
    /// A frozen shape disappeared (removed or renamed).
    Removed { key: String },
    /// A view changed shape and kept its revision.
    ViewNeedsRevision { key: String, revision: u32 },
    /// A view's revision went backwards.
    ViewRevisionDecreased {
        key: String,
        locked: u32,
        current: u32,
    },
    /// The lock is merely behind: new shapes, a view with a bumped revision,
    /// or a view that is gone.
    OutOfDate { key: String },
}

impl Problem {
    /// Whether `update` must refuse to record it.
    pub fn is_breaking(&self) -> bool {
        !matches!(self, Problem::OutOfDate { .. })
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::Changed {
                key,
                locked,
                current,
            } => write!(
                f,
                "`{key}` is persisted and changed shape — stored data would no longer decode.\n    \
                 locked:  {locked}\n    current: {current}\n    \
                 Restore it and add a new variant (or a companion event) for the new shape."
            ),
            Problem::Removed { key } => write!(
                f,
                "`{key}` is persisted and was removed or renamed — stored data would be orphaned. \
                 Restore it; an event that is no longer written still has to be read."
            ),
            Problem::ViewNeedsRevision { key, revision } => write!(
                f,
                "`{key}` changed shape but its projection is still at `.revision({revision})` — \
                 old snapshots would be mis-decoded. Bump the revision."
            ),
            Problem::ViewRevisionDecreased {
                key,
                locked,
                current,
            } => write!(
                f,
                "`{key}` went from revision {locked} back to {current}; revisions only grow."
            ),
            Problem::OutOfDate { key } => write!(
                f,
                "`{key}` is not up to date in events.lock — run \
                 `cargo run -p timada-event-lock -- update` and commit the result."
            ),
        }
    }
}

/// Compares the committed lock with what the sources say now.
pub fn check(locked: &Lock, current: &Lock) -> Vec<Problem> {
    let mut problems = Vec::new();
    for (key, was) in &locked.entries {
        let Some(now) = current.entries.get(key) else {
            problems.push(match was.kind {
                Kind::View => Problem::OutOfDate { key: key.clone() },
                _ => Problem::Removed { key: key.clone() },
            });
            continue;
        };
        match was.kind {
            Kind::Event | Kind::Type if was.shape != now.shape => problems.push(Problem::Changed {
                key: key.clone(),
                locked: was.shape.clone(),
                current: now.shape.clone(),
            }),
            Kind::View => {
                let (locked_rev, current_rev) =
                    (was.revision.unwrap_or(0), now.revision.unwrap_or(0));
                if current_rev < locked_rev {
                    problems.push(Problem::ViewRevisionDecreased {
                        key: key.clone(),
                        locked: locked_rev,
                        current: current_rev,
                    });
                } else if was.shape != now.shape && current_rev == locked_rev {
                    problems.push(Problem::ViewNeedsRevision {
                        key: key.clone(),
                        revision: current_rev,
                    });
                } else if was != now {
                    problems.push(Problem::OutOfDate { key: key.clone() });
                }
            }
            _ => {}
        }
    }
    for key in current.entries.keys() {
        if !locked.entries.contains_key(key) {
            problems.push(Problem::OutOfDate { key: key.clone() });
        }
    }
    problems
}

/// The workspace this tool was built in.
pub fn workspace_root() -> anyhow::Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .context("tools/event-lock is not two levels below the workspace root")
}

/// An `Encode` struct or enum found in the sources.
struct TypeDef {
    krate: String,
    name: String,
    shape: String,
    /// Identifiers its fields mention: candidates for nested types.
    mentions: BTreeSet<String>,
}

struct EventDef {
    name: String,
    krate: String,
    shape: String,
    mentions: BTreeSet<String>,
}

struct ViewDef {
    krate: String,
    name: String,
    shape: String,
    mentions: BTreeSet<String>,
    revision: u32,
}

#[derive(Default)]
struct Found {
    events: Vec<EventDef>,
    types: Vec<TypeDef>,
    views: Vec<ViewDef>,
}

/// Reads every `crates/*/src/**/*.rs` and returns the shapes they persist.
pub fn scan(root: &Path) -> anyhow::Result<Lock> {
    let mut found = Found::default();
    let mut crates: Vec<PathBuf> = std::fs::read_dir(root.join("crates"))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.join("Cargo.toml").is_file())
        .collect();
    crates.sort();
    for dir in crates {
        let krate = package_name(&dir.join("Cargo.toml"))?;
        let mut files = Vec::new();
        rust_files(&dir.join("src"), &mut files)?;
        files.sort();
        for file in files {
            let source = std::fs::read_to_string(&file)?;
            let parsed = syn::parse_file(&source)
                .with_context(|| format!("cannot parse {}", file.display()))?;
            let before = found.views.len();
            collect(&parsed.items, &krate, &mut found)
                .with_context(|| format!("in {}", file.display()))?;
            let revisions = revisions_in(&source);
            let views = &mut found.views[before..];
            match (views.len(), revisions.as_slice()) {
                (_, []) => {}
                (1, [revision]) => views[0].revision = *revision,
                _ => anyhow::bail!(
                    "{}: cannot tell which view `.revision(..)` belongs to — keep one \
                     snapshotted view per file",
                    file.display()
                ),
            }
        }
    }
    assemble(found)
}

fn assemble(found: Found) -> anyhow::Result<Lock> {
    // Everything an event mentions, transitively, is frozen with it.
    let mut frozen: BTreeSet<usize> = BTreeSet::new();
    for event in &found.events {
        reach(&event.krate, &event.mentions, &found.types, &mut frozen);
    }

    let mut lock = Lock::default();
    for event in &found.events {
        let key = format!("event {}", event.name);
        anyhow::ensure!(lock.get(&key).is_none(), "`{key}` is declared twice");
        lock.insert(Kind::Event, &event.name, None, event.shape.clone());
    }
    for index in &frozen {
        let def = &found.types[*index];
        let name = format!("{}::{}", def.krate, def.name);
        anyhow::ensure!(
            lock.get(&format!("type {name}")).is_none(),
            "`{name}` is declared twice in its crate: the lock cannot tell them apart"
        );
        lock.insert(Kind::Type, &name, None, def.shape.clone());
    }
    for view in &found.views {
        // A view's shape includes the types only it uses; the frozen ones
        // already have their own line.
        let mut own: BTreeSet<usize> = BTreeSet::new();
        reach(&view.krate, &view.mentions, &found.types, &mut own);
        let nested: Vec<String> = own
            .difference(&frozen)
            .map(|index| format!("{} {}", found.types[*index].name, found.types[*index].shape))
            .collect();
        let shape = if nested.is_empty() {
            view.shape.clone()
        } else {
            format!("{} with {}", view.shape, nested.join("; "))
        };
        let name = format!("{}::{}", view.krate, view.name);
        lock.insert(Kind::View, &name, Some(view.revision), shape);
    }
    Ok(lock)
}

/// Adds to `into` the types `mentions` leads to, transitively. A name is
/// looked up in the mentioning crate first, then anywhere: a wrong guess only
/// freezes one type too many.
fn reach(krate: &str, mentions: &BTreeSet<String>, types: &[TypeDef], into: &mut BTreeSet<usize>) {
    for mention in mentions {
        let local: Vec<usize> = (0..types.len())
            .filter(|i| types[*i].name == *mention && types[*i].krate == krate)
            .collect();
        let candidates = if local.is_empty() {
            (0..types.len())
                .filter(|i| types[*i].name == *mention)
                .collect()
        } else {
            local
        };
        for index in candidates {
            if into.insert(index) {
                reach(&types[index].krate, &types[index].mentions, types, into);
            }
        }
    }
}

fn package_name(manifest: &Path) -> anyhow::Result<String> {
    std::fs::read_to_string(manifest)?
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("name = \"")
                .and_then(|rest| rest.strip_suffix('"'))
                .map(str::to_owned)
        })
        .with_context(|| format!("no package name in {}", manifest.display()))
}

fn rust_files(dir: &Path, into: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            rust_files(&path, into)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            into.push(path);
        }
    }
    Ok(())
}

/// The integer literals of every `.revision(<n>)` call in a file.
fn revisions_in(source: &str) -> Vec<u32> {
    source
        .split(".revision(")
        .skip(1)
        .filter_map(|rest| {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().ok()
        })
        .collect()
}

fn collect(items: &[syn::Item], krate: &str, found: &mut Found) -> anyhow::Result<()> {
    for item in items {
        match item {
            syn::Item::Mod(module) => {
                if let Some((_, items)) = &module.content {
                    collect(items, krate, found)?;
                }
            }
            syn::Item::Enum(item) => {
                if let Some(attr) = attribute(&item.attrs, &["evento", "aggregate"]) {
                    let aggregate = pinned_name(attr)?.with_context(|| {
                        format!(
                            "`{}` has no `name = \"..\"`: pin it, the crate name is persisted",
                            item.ident
                        )
                    })?;
                    for variant in &item.variants {
                        found.events.push(EventDef {
                            name: format!("{aggregate}::{}", variant.ident),
                            krate: krate.to_owned(),
                            shape: fields_shape(&variant.fields),
                            mentions: mentions_of(&variant.fields),
                        });
                    }
                } else if derives_encode(&item.attrs) {
                    let variants: Vec<String> = item
                        .variants
                        .iter()
                        .map(|v| match &v.fields {
                            syn::Fields::Unit => v.ident.to_string(),
                            fields => format!("{} {}", v.ident, fields_shape(fields)),
                        })
                        .collect();
                    let mut mentions = BTreeSet::new();
                    for variant in &item.variants {
                        mentions.extend(mentions_of(&variant.fields));
                    }
                    found.types.push(TypeDef {
                        krate: krate.to_owned(),
                        name: item.ident.to_string(),
                        shape: format!("enum {{ {} }}", variants.join(", ")),
                        mentions,
                    });
                }
            }
            syn::Item::Struct(item) => {
                let snapshotted = attribute(&item.attrs, &["evento", "projection"])
                    .is_some_and(|attr| attr.meta.to_token_stream().to_string().contains("Encode"));
                if snapshotted {
                    found.views.push(ViewDef {
                        krate: krate.to_owned(),
                        name: item.ident.to_string(),
                        shape: fields_shape(&item.fields),
                        mentions: mentions_of(&item.fields),
                        revision: 0,
                    });
                } else if derives_encode(&item.attrs) {
                    found.types.push(TypeDef {
                        krate: krate.to_owned(),
                        name: item.ident.to_string(),
                        shape: fields_shape(&item.fields),
                        mentions: mentions_of(&item.fields),
                    });
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn attribute<'a>(attrs: &'a [syn::Attribute], path: &[&str]) -> Option<&'a syn::Attribute> {
    attrs.iter().find(|attr| {
        let segments: Vec<String> = attr
            .path()
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        segments == path
    })
}

/// The `name = ".."` of an `#[evento::aggregate(..)]` attribute.
fn pinned_name(attr: &syn::Attribute) -> anyhow::Result<Option<String>> {
    let mut name = None;
    if matches!(attr.meta, syn::Meta::List(_)) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                let value: syn::LitStr = meta.value()?.parse()?;
                name = Some(value.value());
            }
            Ok(())
        })?;
    }
    Ok(name)
}

fn derives_encode(attrs: &[syn::Attribute]) -> bool {
    attrs
        .iter()
        .filter(|attr| attr.path().is_ident("derive"))
        .any(|attr| {
            let mut encodes = false;
            // A derive list that does not parse is simply not ours.
            let _ = attr.parse_nested_meta(|meta| {
                encodes |= meta
                    .path
                    .segments
                    .last()
                    .is_some_and(|segment| segment.ident == "Encode");
                Ok(())
            });
            encodes
        })
}

fn fields_shape(fields: &syn::Fields) -> String {
    match fields {
        syn::Fields::Unit => "unit".to_owned(),
        syn::Fields::Named(named) => {
            let fields: Vec<String> = named
                .named
                .iter()
                .map(|f| {
                    let name = f
                        .ident
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default();
                    format!("{name}: {}", type_text(&f.ty))
                })
                .collect();
            format!("{{ {} }}", fields.join(", "))
        }
        syn::Fields::Unnamed(unnamed) => {
            let fields: Vec<String> = unnamed.unnamed.iter().map(|f| type_text(&f.ty)).collect();
            format!("({})", fields.join(", "))
        }
    }
}

/// `Vec < (String , u32) >` as `Vec<(String, u32)>`.
fn type_text(ty: &syn::Type) -> String {
    let compact: String = ty
        .to_token_stream()
        .to_string()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    compact.replace(',', ", ")
}

fn mentions_of(fields: &syn::Fields) -> BTreeSet<String> {
    struct Idents(BTreeSet<String>);
    impl<'ast> Visit<'ast> for Idents {
        fn visit_path_segment(&mut self, segment: &'ast syn::PathSegment) {
            self.0.insert(segment.ident.to_string());
            syn::visit::visit_path_segment(self, segment);
        }
    }
    let mut idents = Idents(BTreeSet::new());
    for field in fields {
        idents.visit_type(&field.ty);
    }
    idents.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock_of(source: &str) -> anyhow::Result<Lock> {
        let parsed = syn::parse_file(source)?;
        let mut found = Found::default();
        collect(&parsed.items, "timada-demo", &mut found)?;
        if let [revision] = revisions_in(source).as_slice()
            && let [view] = found.views.as_mut_slice()
        {
            view.revision = *revision;
        }
        assemble(found)
    }

    const V1: &str = r#"
        #[derive(Encode, Decode)] pub struct Money { pub minor: i64, pub currency: String }
        #[derive(Encode, Decode)] pub enum Status { Open, Closed }
        #[evento::aggregate(name = "timada-demo/Order")]
        pub enum Order { Placed { total: Money, lines: Vec<(String, u32)> }, Cancelled }
        #[evento::projection(bitcode::Encode, bitcode::Decode)]
        pub struct OrderView { pub id: String, pub status: Status }
    "#;

    #[test]
    fn events_freeze_what_they_mention_and_views_carry_the_rest() -> anyhow::Result<()> {
        let lock = lock_of(V1)?;
        let shape = |key: &str| lock.get(key).map(|e| e.shape.as_str());
        assert_eq!(
            shape("event timada-demo/Order::Placed"),
            Some("{ total: Money, lines: Vec<(String, u32)> }")
        );
        assert_eq!(shape("event timada-demo/Order::Cancelled"), Some("unit"));
        assert_eq!(
            shape("type timada-demo::Money"),
            Some("{ minor: i64, currency: String }")
        );
        // `Status` is only used by the view: part of its shape, not frozen.
        assert_eq!(shape("type timada-demo::Status"), None);
        assert_eq!(
            shape("view timada-demo::OrderView"),
            Some("{ id: String, status: Status } with Status enum { Open, Closed }")
        );
        assert_eq!(Lock::parse(&lock.render())?, lock);
        assert!(check(&lock, &lock).is_empty());
        Ok(())
    }

    #[test]
    fn frozen_shapes_cannot_change_but_new_ones_can_appear() -> anyhow::Result<()> {
        let locked = lock_of(V1)?;

        let field_added = lock_of(&V1.replace("total: Money,", "total: Money, note: String,"))?;
        assert!(matches!(
            check(&locked, &field_added).as_slice(),
            [Problem::Changed { key, .. }] if key == "event timada-demo/Order::Placed"
        ));
        let nested_changed = lock_of(&V1.replace("pub minor: i64", "pub minor: i128"))?;
        assert!(matches!(
            check(&locked, &nested_changed).as_slice(),
            [Problem::Changed { key, .. }] if key == "type timada-demo::Money"
        ));
        let renamed = lock_of(&V1.replace("Cancelled }", "Canceled }"))?;
        let problems = check(&locked, &renamed);
        assert!(problems.contains(&Problem::Removed {
            key: "event timada-demo/Order::Cancelled".into()
        }));

        // The way forward: a new variant is only a lock to refresh.
        let grown = lock_of(&V1.replace("Cancelled }", "Cancelled, Noted { note: String } }"))?;
        let problems = check(&locked, &grown);
        assert_eq!(
            problems,
            [Problem::OutOfDate {
                key: "event timada-demo/Order::Noted".into()
            }]
        );
        assert!(!problems[0].is_breaking());
        Ok(())
    }

    #[test]
    fn a_view_may_change_only_with_a_new_revision() -> anyhow::Result<()> {
        let locked = lock_of(V1)?;
        let reshaped = V1.replace("Open, Closed", "Open, Closed, Archived");
        assert_eq!(
            check(&locked, &lock_of(&reshaped)?),
            [Problem::ViewNeedsRevision {
                key: "view timada-demo::OrderView".into(),
                revision: 0
            }]
        );
        let bumped = format!("{reshaped} fn p() {{ Projection::new().revision(1) }}");
        let bumped = lock_of(&bumped)?;
        assert_eq!(
            check(&locked, &bumped),
            [Problem::OutOfDate {
                key: "view timada-demo::OrderView".into()
            }]
        );
        assert!(matches!(
            check(&bumped, &locked).as_slice(),
            [Problem::ViewRevisionDecreased { .. }]
        ));
        Ok(())
    }

    #[test]
    fn an_aggregate_must_pin_its_name() {
        let unpinned = "#[evento::aggregate] pub enum Order { Placed }";
        assert!(lock_of(unpinned).is_err());
    }
}
