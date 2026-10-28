/// Who writes what the host does on its own behalf (a run it advances, a
/// code it creates), in the `*_by` fields.
pub const AUTOMATION_ACTOR: &str = "timada-automations";

/// The longest name of a scenario, in characters.
pub const MAX_NAME_LEN: usize = 80;
/// The most steps a scenario holds, every lane counted.
pub const MAX_STEPS: usize = 50;
/// How deep branching steps nest: a lane inside this many branches holds no
/// branching step.
pub const MAX_DEPTH: usize = 4;
/// The largest definition, as JSON, in bytes.
pub const MAX_DEFINITION_BYTES: usize = 256 * 1024;
/// The most rules one condition checks.
pub const MAX_RULES: usize = 10;
/// The longest wait, in seconds.
pub const MAX_WAIT_SECS: u64 = 90 * 24 * 60 * 60;
/// The longest subject of a notice, in characters.
pub const MAX_SUBJECT_LEN: usize = 200;
/// The longest note of a notice, in characters.
pub const MAX_NOTE_LEN: usize = 2000;
/// The longest text a condition compares with, in characters.
pub const MAX_VALUE_LEN: usize = 200;

/// The definition format this build reads and writes. A definition of a
/// later format (written by a newer node) is shown, never edited.
pub const DEFINITION_SCHEMA: u32 = 1;

/// Where a scenario stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ScenarioStatus {
    /// Never published: nothing runs.
    #[default]
    Draft,
    /// Published: what triggers it starts a run.
    Active,
    /// Published, then held.
    Paused,
    /// Deleted from the merchant's view.
    Archived,
}

impl ScenarioStatus {
    pub const ALL: [Self; 4] = [Self::Draft, Self::Active, Self::Paused, Self::Archived];

    /// The value stored in the read model.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Archived => "archived",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|status| status.as_str() == value)
    }
}

/// A fresh id for a scenario: a ULID, sortable by when it was made.
pub fn new_scenario_id() -> String {
    ulid::Ulid::generate().to_string()
}

/// A name trimmed, or `None` when it is empty, too long or holds control
/// characters.
pub fn checked_name(name: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty()
        && name.chars().count() <= MAX_NAME_LEN
        && !name.chars().any(char::is_control))
    .then(|| name.to_owned())
}
