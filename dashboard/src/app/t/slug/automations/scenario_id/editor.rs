//! The editor's working copy and what each of its buttons does to it.
//!
//! The page holds the copy: the definition as JSON in a hidden field, with
//! the scenario version it was made from, the block whose settings are open
//! and the block being moved, if any. Every button posts the whole form,
//! named `op`; the settings shown are applied first, so typing is never lost
//! to a drag, then the button's action runs on the copy and the page comes
//! back with it. Nothing is written until "Save draft".

use std::collections::{BTreeMap, HashMap};

use timada_automations::{Definition, EditError, Lane, ScenarioState, Slot, catalog};

use super::super::labels::{edit_error_label, step_label, trigger_label};

/// What `selected` holds while the trigger's settings are open.
pub const TRIGGER: &str = "trigger";
/// The prefix of the fields holding the open block's settings.
const SETTINGS: &str = "settings.";

/// What is being put somewhere without dragging: a block of the scenario
/// (its id), or a new block of the palette (its kind).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Moving {
    Step(String),
    Kind(String),
}

impl Moving {
    pub fn encode(&self) -> String {
        match self {
            Self::Step(id) => format!("step:{id}"),
            Self::Kind(kind) => format!("kind:{kind}"),
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.split_once(':')? {
            ("step", id) if !id.is_empty() => Some(Self::Step(id.to_owned())),
            ("kind", kind) if !kind.is_empty() => Some(Self::Kind(kind.to_owned())),
            _ => None,
        }
    }
}

/// What the editor shows.
#[derive(Debug, Clone)]
pub struct Editor {
    pub definition: Definition,
    /// The name as typed.
    pub name: String,
    /// The scenario's version the copy was made from.
    pub base: u16,
    /// `"trigger"` or a block's id: whose settings are open.
    pub selected: Option<String>,
    pub moving: Option<Moving>,
    /// What the last action did, for screen readers.
    pub announce: Option<String>,
    /// Why the last action did not happen.
    pub refusal: Option<&'static str>,
    /// The element the page puts the focus on.
    pub focus: Option<String>,
    /// The draft and the name as saved, to tell whether the copy holds
    /// unsaved changes.
    saved: (String, String),
}

/// A button of the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Applies the settings shown, nothing else.
    Apply,
    Select(String),
    Deselect,
    Trigger(String),
    Add(Slot, String),
    /// A block dragged to a place.
    Move(String, Slot),
    /// Picks a block up to move it without dragging.
    Pick(String),
    /// Picks a palette block up to put it somewhere without dragging.
    Place(String),
    /// Puts down what was picked up.
    Drop(Slot),
    Cancel,
    Up(String),
    Down(String),
    Remove(String),
    RuleAdd,
    RuleRemove(usize),
    Save,
}

impl Op {
    /// Reads a button's `op` value; anything else applies the settings.
    pub fn parse(value: &str) -> Self {
        let mut parts = value.splitn(3, ':');
        let (verb, first, second) = (parts.next(), parts.next(), parts.next());
        let slot = |value: Option<&str>| value.and_then(Slot::parse);
        let id = |value: Option<&str>| value.filter(|id| !id.is_empty()).map(str::to_owned);
        let parsed = match verb.unwrap_or("") {
            "select" => id(first).map(Self::Select),
            "deselect" => Some(Self::Deselect),
            "trigger" => id(first).map(Self::Trigger),
            "add" => slot(first)
                .zip(id(second))
                .map(|(slot, kind)| Self::Add(slot, kind)),
            "move" => id(first)
                .zip(slot(second))
                .map(|(id, slot)| Self::Move(id, slot)),
            "pick" => id(first).map(Self::Pick),
            "place" => id(first).map(Self::Place),
            "drop" => slot(first).map(Self::Drop),
            "cancel" => Some(Self::Cancel),
            "up" => id(first).map(Self::Up),
            "down" => id(first).map(Self::Down),
            "remove" => id(first).map(Self::Remove),
            "rule-add" => Some(Self::RuleAdd),
            "rule-remove" => first
                .and_then(|index| index.parse().ok())
                .map(Self::RuleRemove),
            "save" => Some(Self::Save),
            _ => None,
        };
        parsed.unwrap_or(Self::Apply)
    }
}

/// What the editor posts. The settings of the open block come as
/// `settings.<key>` fields, gathered in `fields`.
#[derive(serde::Deserialize)]
pub struct EditorForm {
    #[serde(default)]
    pub definition: String,
    #[serde(default)]
    pub base: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub selected: String,
    #[serde(default)]
    pub moving: String,
    /// The block whose settings the form carries.
    #[serde(default)]
    pub settings_for: String,
    #[serde(default)]
    pub op: String,
    #[serde(flatten)]
    pub fields: HashMap<String, String>,
}

impl EditorForm {
    /// The settings the form carries, as typed (browsers send a textarea's
    /// line breaks as CRLF).
    fn settings(&self) -> BTreeMap<String, String> {
        self.fields
            .iter()
            .filter_map(|(key, value)| {
                key.strip_prefix(SETTINGS)
                    .map(|key| (key.to_owned(), value.replace("\r\n", "\n")))
            })
            .collect()
    }

    /// The working copy the page posted, with the open block's settings
    /// applied. A copy that does not read (an old tab, a mangled form) is
    /// set aside for the saved draft, and said so.
    pub fn into_editor(self, state: &ScenarioState) -> anyhow::Result<Editor> {
        let mut editor = Editor::of(state)?;
        match Definition::from_json(&self.definition) {
            Ok(definition) => editor.definition = definition,
            Err(_) => {
                editor.refusal = Some("The page was out of date: it shows the saved draft.");
                return Ok(editor);
            }
        }
        if let Ok(base) = self.base.parse() {
            editor.base = base;
        }
        editor.name = self.name.clone();
        editor.selected = Some(self.selected.clone()).filter(|id| !id.is_empty());
        editor.moving = Moving::parse(&self.moving);
        if !self.settings_for.is_empty()
            && self.settings_for != TRIGGER
            && editor.definition.find(&self.settings_for).is_some()
            && editor.definition.is_editable()
        {
            editor
                .definition
                .configure(&self.settings_for, self.settings())
                .expect("the step was found above");
        }
        Ok(editor)
    }

    pub fn op(&self) -> Op {
        Op::parse(&self.op)
    }
}

impl Editor {
    /// The editor on the saved draft, nothing open.
    pub fn of(state: &ScenarioState) -> anyhow::Result<Self> {
        use evento::ProjectionAggregate;
        Ok(Self {
            definition: state.definition()?,
            name: state.name.clone(),
            base: state.aggregate_version()?,
            selected: None,
            moving: None,
            announce: None,
            refusal: None,
            focus: None,
            saved: (state.draft.clone(), state.name.clone()),
        })
    }

    /// Whether the copy holds what the saved draft does not.
    pub fn is_dirty(&self) -> bool {
        let saved = Definition::from_json(&self.saved.0).unwrap_or_default();
        self.definition != saved || self.name.trim() != self.saved.1
    }

    /// The label of what is being moved or put down.
    pub fn moving_label(&self) -> Option<&'static str> {
        match self.moving.as_ref()? {
            Moving::Step(id) => self.definition.find(id).map(|step| step_label(&step.kind)),
            Moving::Kind(kind) => Some(step_label(kind)),
        }
    }

    fn label_of(&self, id: &str) -> &'static str {
        self.definition
            .find(id)
            .map_or("Block", |step| step_label(&step.kind))
    }

    fn refuse(&mut self, error: EditError) {
        self.refusal = Some(edit_error_label(error));
    }

    /// Runs a button on the copy; whether it asks for the copy to be saved
    /// (which the caller does, as it writes).
    pub fn run(&mut self, op: Op) -> bool {
        match op {
            Op::Apply => self.announce = Some("Settings applied.".into()),
            Op::Select(id) => {
                if id == TRIGGER || self.definition.find(&id).is_some() {
                    self.focus = Some("inspector".into());
                    self.selected = Some(id);
                } else {
                    self.refuse(EditError::NoSuchStep);
                }
            }
            Op::Deselect => self.selected = None,
            Op::Trigger(kind) => match self.definition.set_trigger(&kind) {
                Ok(()) => {
                    self.selected = Some(TRIGGER.into());
                    self.focus = Some(TRIGGER.into());
                    self.announce = Some(format!("Trigger set: {}.", trigger_label(&kind)));
                }
                Err(error) => self.refuse(error),
            },
            Op::Add(slot, kind) => self.put(&slot, &kind),
            Op::Move(id, slot) => self.shift(&id, &slot),
            Op::Pick(id) => {
                if self.definition.find(&id).is_some() {
                    self.announce = Some(format!(
                        "Choose where to move {}.",
                        self.label_of(&id).to_lowercase()
                    ));
                    self.moving = Some(Moving::Step(id));
                    self.focus = Some("moving".into());
                } else {
                    self.refuse(EditError::NoSuchStep);
                }
            }
            Op::Place(kind) => {
                if catalog::step_kind(&kind).is_some() {
                    self.announce = Some(format!(
                        "Choose where to put {}.",
                        step_label(&kind).to_lowercase()
                    ));
                    self.moving = Some(Moving::Kind(kind));
                    self.focus = Some("moving".into());
                } else {
                    self.refuse(EditError::UnknownKind);
                }
            }
            Op::Drop(slot) => match self.moving.take() {
                Some(Moving::Step(id)) => self.shift(&id, &slot),
                Some(Moving::Kind(kind)) => self.put(&slot, &kind),
                None => self.refuse(EditError::NoSuchStep),
            },
            Op::Cancel => {
                self.moving = None;
                self.announce = Some("Cancelled.".into());
            }
            Op::Up(id) => match self.definition.move_up(&id) {
                Ok(()) => {
                    self.announce = Some(format!("{} moved up.", self.label_of(&id)));
                    self.focus = Some(id);
                }
                Err(error) => self.refuse(error),
            },
            Op::Down(id) => match self.definition.move_down(&id) {
                Ok(()) => {
                    self.announce = Some(format!("{} moved down.", self.label_of(&id)));
                    self.focus = Some(id);
                }
                Err(error) => self.refuse(error),
            },
            Op::Remove(id) => {
                let label = self.label_of(&id);
                match self.definition.remove(&id) {
                    Ok(removed) => {
                        if self
                            .selected
                            .as_deref()
                            .is_some_and(|open| removed.holds(open))
                        {
                            self.selected = None;
                        }
                        if let Some(Moving::Step(moving)) = &self.moving
                            && removed.holds(moving)
                        {
                            self.moving = None;
                        }
                        self.announce = Some(format!("{label} removed."));
                        self.focus = Some("canvas".into());
                    }
                    Err(error) => self.refuse(error),
                }
            }
            Op::RuleAdd => {
                if let Some(id) = self.selected.clone()
                    && let Err(error) = self.definition.add_rule(&id)
                {
                    self.refuse(error);
                }
                self.focus = Some("inspector".into());
            }
            Op::RuleRemove(index) => {
                if let Some(id) = self.selected.clone()
                    && let Err(error) = self.definition.remove_rule(&id, index)
                {
                    self.refuse(error);
                }
                self.focus = Some("inspector".into());
            }
            Op::Save => return true,
        }
        false
    }

    fn put(&mut self, slot: &Slot, kind: &str) {
        self.moving = None;
        match self.definition.insert(slot, kind) {
            Ok(id) => {
                self.announce = Some(format!("{} added.", step_label(kind)));
                self.focus = Some(id.clone());
                self.selected = Some(id);
            }
            Err(error) => self.refuse(error),
        }
    }

    fn shift(&mut self, id: &str, slot: &Slot) {
        self.moving = None;
        match self.definition.move_to(id, slot) {
            Ok(()) => {
                self.announce = Some(format!("{} moved.", self.label_of(id)));
                self.focus = Some(id.to_owned());
            }
            Err(error) => self.refuse(error),
        }
    }

    /// Whether a block being moved or put down may go to `slot`: not inside
    /// itself, and not right next to where it already is.
    pub fn takes(&self, slot: &Slot) -> bool {
        match &self.moving {
            None => false,
            Some(Moving::Kind(_)) => true,
            Some(Moving::Step(id)) => {
                let Some(step) = self.definition.find(id) else {
                    return false;
                };
                if let Lane::Branch { step: owner, .. } = &slot.lane
                    && step.holds(owner)
                {
                    return false;
                }
                match self.definition.locate(id) {
                    Some(at) if at.lane == slot.lane => {
                        slot.index != at.index && slot.index != at.index + 1
                    }
                    _ => true,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use timada_automations::catalog::{CONDITION, NOTIFY, ORDER_PLACED, WAIT};

    use super::*;

    fn editor() -> Editor {
        Editor {
            definition: Definition::default(),
            name: "Big orders".into(),
            base: 1,
            selected: None,
            moving: None,
            announce: None,
            refusal: None,
            focus: None,
            saved: (String::new(), "Big orders".into()),
        }
    }

    fn root(index: usize) -> Slot {
        Slot::new(Lane::Root, index)
    }

    #[test]
    fn buttons_read_their_op() {
        assert_eq!(
            Op::parse("add:root.0:wait"),
            Op::Add(root(0), "wait".into())
        );
        assert_eq!(
            Op::parse("move:s3:s1.1.0"),
            Op::Move(
                "s3".into(),
                Slot::new(
                    Lane::Branch {
                        step: "s1".into(),
                        branch: 1
                    },
                    0
                )
            )
        );
        assert_eq!(Op::parse("rule-remove:2"), Op::RuleRemove(2));
        assert_eq!(Op::parse("save"), Op::Save);
        // Anything else only applies the settings.
        assert_eq!(Op::parse(""), Op::Apply);
        assert_eq!(Op::parse("add:nowhere:wait"), Op::Apply);
    }

    #[test]
    fn a_block_is_picked_up_then_put_down_without_dragging() {
        let mut editor = editor();
        editor.definition.set_trigger(ORDER_PLACED).unwrap();
        assert!(!editor.run(Op::Place(WAIT.into())));
        assert_eq!(editor.moving, Some(Moving::Kind(WAIT.into())));
        assert!(editor.takes(&root(0)));
        editor.run(Op::Drop(root(0)));
        assert_eq!(editor.moving, None);
        assert_eq!(editor.definition.count(), 1);
        // The new block's settings open.
        assert_eq!(editor.selected.as_deref(), Some("s1"));
        assert!(editor.is_dirty());

        editor.run(Op::Add(root(1), CONDITION.into()));
        editor.run(Op::Pick("s1".into()));
        // Not next to where it is, nor inside itself.
        assert!(!editor.takes(&root(0)));
        assert!(!editor.takes(&root(1)));
        assert!(editor.takes(&root(2)));
        editor.run(Op::Drop(Slot::new(
            Lane::Branch {
                step: "s2".into(),
                branch: 0,
            },
            0,
        )));
        assert_eq!(
            editor.definition.locate("s1").unwrap().lane,
            Lane::Branch {
                step: "s2".into(),
                branch: 0
            }
        );
        editor.run(Op::Pick("s2".into()));
        assert!(!editor.takes(&Slot::new(
            Lane::Branch {
                step: "s2".into(),
                branch: 1,
            },
            0,
        )));
    }

    #[test]
    fn removing_the_open_block_closes_its_settings() {
        let mut editor = editor();
        editor.run(Op::Add(root(0), CONDITION.into()));
        editor.run(Op::Add(
            Slot::new(
                Lane::Branch {
                    step: "s1".into(),
                    branch: 0,
                },
                0,
            ),
            NOTIFY.into(),
        ));
        assert_eq!(editor.selected.as_deref(), Some("s2"));
        editor.run(Op::Remove("s1".into()));
        assert_eq!(editor.selected, None);
        assert_eq!(editor.announce.as_deref(), Some("If / else removed."));
        editor.run(Op::Remove("s1".into()));
        assert!(editor.refusal.is_some());
    }
}
