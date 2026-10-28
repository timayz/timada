//! A scenario as the merchant draws it: a trigger, then a lane of steps, a
//! branching step (a condition) holding one more lane per branch. It is kept
//! whole as JSON in the scenario's events (see [`Definition::to_json`]).
//!
//! Everything here is pure: the editor applies an operation to its working
//! copy, shows it, and saves the copy whole. A place between two steps is a
//! [`Slot`]; the operations refuse what the limits forbid, and
//! [`Definition::check`] lists what keeps the copy from being published.

use std::collections::{BTreeMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::catalog::{self, CONDITION, Subject};
use crate::value_object::{
    DEFINITION_SCHEMA, MAX_DEFINITION_BYTES, MAX_DEPTH, MAX_RULES, MAX_STEPS,
};

/// A scenario, as drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Definition {
    /// The format it is written in ([`DEFINITION_SCHEMA`]).
    #[serde(default = "first_schema")]
    pub schema: u32,
    /// What starts a run; none until the merchant picks it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<Trigger>,
    /// The lane under the trigger.
    #[serde(default)]
    pub steps: Vec<Step>,
    /// The number of the last step id handed out (`s1`, `s2`, …), so an id
    /// is never handed out twice, even after its step was removed.
    #[serde(default)]
    pub next_id: u32,
}

fn first_schema() -> u32 {
    1
}

impl Default for Definition {
    fn default() -> Self {
        Self {
            schema: DEFINITION_SCHEMA,
            trigger: None,
            steps: Vec::new(),
            next_id: 0,
        }
    }
}

/// What starts a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trigger {
    /// A [`catalog::TRIGGERS`] key.
    pub kind: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub settings: BTreeMap<String, String>,
}

/// One step of a lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// `s<n>`, unique in the definition.
    pub id: String,
    /// A [`catalog::STEPS`] key, or one a newer build knows.
    pub kind: String,
    /// The settings exactly as typed, by key: read with the catalog's
    /// `parse_*`, so what does not read yet is kept and shown.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub settings: BTreeMap<String, String>,
    /// The lanes of a branching step, in its kind's order (a condition:
    /// Yes, then No).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub branches: Vec<Vec<Step>>,
}

impl Step {
    /// How many lanes deep the step reaches below the lane it sits in: none
    /// for a plain step, one for a condition, more when conditions nest.
    pub fn height(&self) -> usize {
        if self.branches.is_empty() {
            return 0;
        }
        1 + self
            .branches
            .iter()
            .flatten()
            .map(Step::height)
            .max()
            .unwrap_or(0)
    }

    /// Whether `id` names this step or one inside its lanes.
    pub fn holds(&self, id: &str) -> bool {
        self.id == id || self.branches.iter().flatten().any(|step| step.holds(id))
    }

    fn count(&self) -> usize {
        1 + self
            .branches
            .iter()
            .flatten()
            .map(Step::count)
            .sum::<usize>()
    }
}

/// A lane: the one under the trigger, or a branch of a branching step.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Lane {
    Root,
    Branch { step: String, branch: usize },
}

impl fmt::Display for Lane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Root => f.write_str("root"),
            Self::Branch { step, branch } => write!(f, "{step}.{branch}"),
        }
    }
}

impl Lane {
    /// Reads `root` or `<step>.<branch>`.
    pub fn parse(value: &str) -> Option<Self> {
        if value == "root" {
            return Some(Self::Root);
        }
        let (step, branch) = value.rsplit_once('.')?;
        valid_id(step).then_some(())?;
        Some(Self::Branch {
            step: step.to_owned(),
            branch: branch.parse().ok()?,
        })
    }
}

/// A place in a lane: before the step at `index`, or after the last when
/// `index` is the lane's length.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Slot {
    pub lane: Lane,
    pub index: usize,
}

impl fmt::Display for Slot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.lane, self.index)
    }
}

impl Slot {
    pub fn new(lane: Lane, index: usize) -> Self {
        Self { lane, index }
    }

    /// Reads `root.<index>` or `<step>.<branch>.<index>`.
    pub fn parse(value: &str) -> Option<Self> {
        let (lane, index) = value.rsplit_once('.')?;
        Some(Self {
            lane: Lane::parse(lane)?,
            index: index.parse().ok()?,
        })
    }
}

/// Why an edit was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    #[error("no such step")]
    NoSuchStep,
    #[error("no such place")]
    NoSuchSlot,
    #[error("unknown kind of step or trigger")]
    UnknownKind,
    #[error("a step cannot go inside itself")]
    IntoItself,
    #[error("a scenario holds at most {MAX_STEPS} steps")]
    TooManySteps,
    #[error("conditions nest at most {MAX_DEPTH} deep")]
    TooDeep,
    #[error("a condition checks at most {MAX_RULES} rules")]
    TooManyRules,
    #[error("the scenario was drawn by a newer version")]
    NewerSchema,
}

/// What a problem is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProblemAt {
    Scenario,
    Trigger,
    Step(String),
}

/// One thing that keeps a definition from being published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub at: ProblemAt,
    /// The setting it is about, when one is.
    pub key: Option<String>,
    /// A `PROBLEM_*` code, or one of the catalog's `ISSUE_*` codes.
    pub code: &'static str,
}

impl Problem {
    fn new(at: ProblemAt, code: &'static str) -> Self {
        Self {
            at,
            key: None,
            code,
        }
    }

    /// Whether the definition cannot even be kept as a draft (beyond the
    /// limits, or not ours to read).
    pub fn is_hard(&self) -> bool {
        HARD_PROBLEMS.contains(&self.code)
    }
}

pub const PROBLEM_NEWER_SCHEMA: &str = "newer_schema";
pub const PROBLEM_NO_TRIGGER: &str = "no_trigger";
pub const PROBLEM_UNKNOWN_TRIGGER: &str = "unknown_trigger";
pub const PROBLEM_NO_STEPS: &str = "no_steps";
pub const PROBLEM_UNKNOWN_STEP: &str = "unknown_step";
/// A step holds another number of lanes than its kind has branches.
pub const PROBLEM_BRANCHES: &str = "branches";
pub const PROBLEM_INVALID_ID: &str = "invalid_id";
pub const PROBLEM_TOO_MANY_STEPS: &str = "too_many_steps";
pub const PROBLEM_TOO_DEEP: &str = "too_deep";
pub const PROBLEM_TOO_LARGE: &str = "too_large";

const HARD_PROBLEMS: &[&str] = &[
    PROBLEM_NEWER_SCHEMA,
    PROBLEM_BRANCHES,
    PROBLEM_INVALID_ID,
    PROBLEM_TOO_MANY_STEPS,
    PROBLEM_TOO_DEEP,
    PROBLEM_TOO_LARGE,
];

/// Whether `id` is a step id as handed out (`s<n>`).
fn valid_id(id: &str) -> bool {
    id.strip_prefix('s')
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn id_number(id: &str) -> u32 {
    id.strip_prefix('s')
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

impl Definition {
    /// Reads a definition as stored; an empty string is a scenario never
    /// saved (no trigger, no step).
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        if json.trim().is_empty() {
            return Ok(Self::default());
        }
        serde_json::from_str(json)
    }

    /// The definition as stored: the same definition always gives the same
    /// text (settings are sorted by key), so two copies compare as strings.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a definition always serializes")
    }

    /// Whether this build may edit it: one of a later format is shown only.
    pub fn is_editable(&self) -> bool {
        self.schema <= DEFINITION_SCHEMA
    }

    /// What a run is about, once the trigger is known.
    pub fn subject(&self) -> Option<Subject> {
        let trigger = self.trigger.as_ref()?;
        catalog::trigger_kind(&trigger.kind).map(|kind| kind.subject)
    }

    /// Every step, every lane counted.
    pub fn count(&self) -> usize {
        self.steps.iter().map(Step::count).sum()
    }

    /// How many lanes deep the deepest one is (the lane under the trigger is
    /// 0).
    pub fn depth(&self) -> usize {
        self.steps.iter().map(Step::height).max().unwrap_or(0)
    }

    /// Every step, depth first, each before the steps of its lanes.
    pub fn walk(&self) -> Vec<&Step> {
        let mut steps = Vec::new();
        let mut stack: Vec<&Step> = self.steps.iter().rev().collect();
        while let Some(step) = stack.pop() {
            steps.push(step);
            stack.extend(
                step.branches
                    .iter()
                    .rev()
                    .flat_map(|lane| lane.iter().rev()),
            );
        }
        steps
    }

    pub fn find(&self, id: &str) -> Option<&Step> {
        self.walk().into_iter().find(|step| step.id == id)
    }

    fn find_mut(&mut self, id: &str) -> Option<&mut Step> {
        fn search<'a>(steps: &'a mut [Step], id: &str) -> Option<&'a mut Step> {
            for step in steps {
                if step.id == id {
                    return Some(step);
                }
                if step.holds(id) {
                    return step.branches.iter_mut().find_map(|lane| search(lane, id));
                }
            }
            None
        }
        search(&mut self.steps, id)
    }

    /// The steps of a lane.
    pub fn lane(&self, lane: &Lane) -> Option<&Vec<Step>> {
        match lane {
            Lane::Root => Some(&self.steps),
            Lane::Branch { step, branch } => self.find(step)?.branches.get(*branch),
        }
    }

    fn lane_mut(&mut self, lane: &Lane) -> Option<&mut Vec<Step>> {
        match lane {
            Lane::Root => Some(&mut self.steps),
            Lane::Branch { step, branch } => self.find_mut(step)?.branches.get_mut(*branch),
        }
    }

    /// How deep a lane is: 0 under the trigger, one more inside each branch.
    pub fn lane_depth(&self, lane: &Lane) -> Option<usize> {
        match lane {
            Lane::Root => Some(0),
            Lane::Branch { step, branch } => {
                let owner = self.find(step)?;
                owner.branches.get(*branch)?;
                Some(self.lane_depth(&self.locate(step)?.lane)? + 1)
            }
        }
    }

    /// Where a step sits.
    pub fn locate(&self, id: &str) -> Option<Slot> {
        fn search(steps: &[Step], lane: Lane, id: &str) -> Option<Slot> {
            for (index, step) in steps.iter().enumerate() {
                if step.id == id {
                    return Some(Slot::new(lane, index));
                }
                if step.holds(id) {
                    return step
                        .branches
                        .iter()
                        .enumerate()
                        .find_map(|(branch, steps)| {
                            search(
                                steps,
                                Lane::Branch {
                                    step: step.id.clone(),
                                    branch,
                                },
                                id,
                            )
                        });
                }
            }
            None
        }
        search(&self.steps, Lane::Root, id)
    }

    fn editable(&self) -> Result<(), EditError> {
        if self.is_editable() {
            Ok(())
        } else {
            Err(EditError::NewerSchema)
        }
    }

    /// Picks the trigger; the same kind again keeps its settings.
    pub fn set_trigger(&mut self, kind: &str) -> Result<(), EditError> {
        self.editable()?;
        catalog::trigger_kind(kind).ok_or(EditError::UnknownKind)?;
        if self
            .trigger
            .as_ref()
            .is_some_and(|trigger| trigger.kind == kind)
        {
            return Ok(());
        }
        self.trigger = Some(Trigger {
            kind: kind.to_owned(),
            settings: BTreeMap::new(),
        });
        Ok(())
    }

    /// Puts a new step of `kind` at `slot`, with the kind's starting
    /// settings; its id.
    pub fn insert(&mut self, slot: &Slot, kind: &str) -> Result<String, EditError> {
        self.editable()?;
        let step_kind = catalog::step_kind(kind).ok_or(EditError::UnknownKind)?;
        if self.count() >= MAX_STEPS {
            return Err(EditError::TooManySteps);
        }
        let depth = self.lane_depth(&slot.lane).ok_or(EditError::NoSuchSlot)?;
        if step_kind.branches > 0 && depth + 1 > MAX_DEPTH {
            return Err(EditError::TooDeep);
        }
        if slot.index > self.lane(&slot.lane).ok_or(EditError::NoSuchSlot)?.len() {
            return Err(EditError::NoSuchSlot);
        }
        let number = self
            .walk()
            .iter()
            .map(|step| id_number(&step.id))
            .max()
            .unwrap_or(0)
            .max(self.next_id)
            + 1;
        let id = format!("s{number}");
        let step = Step {
            id: id.clone(),
            kind: kind.to_owned(),
            settings: catalog::default_settings(kind, self.subject()),
            branches: vec![Vec::new(); step_kind.branches],
        };
        self.next_id = number;
        self.lane_mut(&slot.lane)
            .expect("the lane was found above")
            .insert(slot.index, step);
        Ok(id)
    }

    /// Moves a step, with its lanes, to `slot`.
    pub fn move_to(&mut self, id: &str, slot: &Slot) -> Result<(), EditError> {
        self.editable()?;
        let from = self.locate(id).ok_or(EditError::NoSuchStep)?;
        let step = self.find(id).ok_or(EditError::NoSuchStep)?;
        if let Lane::Branch { step: owner, .. } = &slot.lane
            && step.holds(owner)
        {
            return Err(EditError::IntoItself);
        }
        let height = step.height();
        let depth = self.lane_depth(&slot.lane).ok_or(EditError::NoSuchSlot)?;
        if slot.index > self.lane(&slot.lane).ok_or(EditError::NoSuchSlot)?.len() {
            return Err(EditError::NoSuchSlot);
        }
        if depth + height > MAX_DEPTH {
            return Err(EditError::TooDeep);
        }
        let mut index = slot.index;
        if from.lane == slot.lane && from.index < index {
            index -= 1;
        }
        let step = self
            .lane_mut(&from.lane)
            .expect("the step was found above")
            .remove(from.index);
        // The target lane is not inside the step, so it is still there.
        self.lane_mut(&slot.lane)
            .expect("the lane was found above")
            .insert(index, step);
        Ok(())
    }

    /// Moves a step one place up its lane; the first of a branch goes out,
    /// above the step holding the branch.
    pub fn move_up(&mut self, id: &str) -> Result<(), EditError> {
        self.editable()?;
        let from = self.locate(id).ok_or(EditError::NoSuchStep)?;
        if from.index > 0 {
            return self.move_to(id, &Slot::new(from.lane, from.index - 1));
        }
        match from.lane {
            Lane::Root => Ok(()),
            Lane::Branch { step: owner, .. } => {
                let owner = self.locate(&owner).ok_or(EditError::NoSuchStep)?;
                self.move_to(id, &owner)
            }
        }
    }

    /// Moves a step one place down its lane; the last of a branch goes out,
    /// below the step holding the branch.
    pub fn move_down(&mut self, id: &str) -> Result<(), EditError> {
        self.editable()?;
        let from = self.locate(id).ok_or(EditError::NoSuchStep)?;
        let len = self.lane(&from.lane).map_or(0, Vec::len);
        if from.index + 1 < len {
            return self.move_to(id, &Slot::new(from.lane, from.index + 2));
        }
        match from.lane {
            Lane::Root => Ok(()),
            Lane::Branch { step: owner, .. } => {
                let owner = self.locate(&owner).ok_or(EditError::NoSuchStep)?;
                self.move_to(id, &Slot::new(owner.lane, owner.index + 1))
            }
        }
    }

    /// Takes a step out, with everything in its lanes.
    pub fn remove(&mut self, id: &str) -> Result<Step, EditError> {
        self.editable()?;
        let from = self.locate(id).ok_or(EditError::NoSuchStep)?;
        Ok(self
            .lane_mut(&from.lane)
            .expect("the step was found above")
            .remove(from.index))
    }

    /// Replaces a step's settings, whole.
    pub fn configure(
        &mut self,
        id: &str,
        settings: BTreeMap<String, String>,
    ) -> Result<(), EditError> {
        self.editable()?;
        self.find_mut(id).ok_or(EditError::NoSuchStep)?.settings = settings;
        Ok(())
    }

    /// Adds a rule to a condition, on the subject's first field.
    pub fn add_rule(&mut self, id: &str) -> Result<(), EditError> {
        self.editable()?;
        let subject = self.subject();
        let step = self.find_mut(id).ok_or(EditError::NoSuchStep)?;
        if step.kind != CONDITION {
            return Err(EditError::UnknownKind);
        }
        let indices = catalog::rule_indices(&step.settings);
        if indices.len() >= MAX_RULES {
            return Err(EditError::TooManyRules);
        }
        let index = indices.last().map_or(0, |last| last + 1);
        let field = subject.and_then(|subject| subject.fields().first());
        let pairs = [
            ("field", field.map_or("", |field| field.key)),
            ("op", field.map_or("", |field| field.ty.ops()[0].as_str())),
            ("value", ""),
        ];
        for (part, value) in pairs {
            step.settings
                .insert(format!("rules.{index}.{part}"), value.to_owned());
        }
        Ok(())
    }

    /// Takes a rule off a condition.
    pub fn remove_rule(&mut self, id: &str, index: usize) -> Result<(), EditError> {
        self.editable()?;
        let step = self.find_mut(id).ok_or(EditError::NoSuchStep)?;
        let prefix = format!("rules.{index}.");
        step.settings.retain(|key, _| !key.starts_with(&prefix));
        Ok(())
    }

    /// What keeps the definition from being published; empty when it can be.
    pub fn check(&self) -> Vec<Problem> {
        let mut problems = Vec::new();
        if !self.is_editable() {
            problems.push(Problem::new(ProblemAt::Scenario, PROBLEM_NEWER_SCHEMA));
            return problems;
        }
        match &self.trigger {
            None => problems.push(Problem::new(ProblemAt::Trigger, PROBLEM_NO_TRIGGER)),
            Some(trigger) if catalog::trigger_kind(&trigger.kind).is_none() => {
                problems.push(Problem::new(ProblemAt::Trigger, PROBLEM_UNKNOWN_TRIGGER));
            }
            Some(_) => {}
        }
        if self.steps.is_empty() {
            problems.push(Problem::new(ProblemAt::Scenario, PROBLEM_NO_STEPS));
        }
        if self.count() > MAX_STEPS {
            problems.push(Problem::new(ProblemAt::Scenario, PROBLEM_TOO_MANY_STEPS));
        }
        if self.depth() > MAX_DEPTH {
            problems.push(Problem::new(ProblemAt::Scenario, PROBLEM_TOO_DEEP));
        }
        if self.to_json().len() > MAX_DEFINITION_BYTES {
            problems.push(Problem::new(ProblemAt::Scenario, PROBLEM_TOO_LARGE));
        }
        let subject = self.subject();
        let mut seen = HashSet::new();
        for step in self.walk() {
            let at = || ProblemAt::Step(step.id.clone());
            if !valid_id(&step.id) || !seen.insert(step.id.as_str()) {
                problems.push(Problem::new(at(), PROBLEM_INVALID_ID));
            }
            let Some(kind) = catalog::step_kind(&step.kind) else {
                problems.push(Problem::new(at(), PROBLEM_UNKNOWN_STEP));
                continue;
            };
            if step.branches.len() != kind.branches {
                problems.push(Problem::new(at(), PROBLEM_BRANCHES));
            }
            for issue in catalog::step_issues(&step.kind, subject, &step.settings) {
                problems.push(Problem {
                    at: at(),
                    key: Some(issue.key),
                    code: issue.code,
                });
            }
        }
        problems
    }
}
