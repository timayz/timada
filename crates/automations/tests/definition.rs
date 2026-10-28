mod common;

use common::*;
use timada_automations::{
    Definition, EditError, Lane, MAX_DEPTH, MAX_STEPS, PROBLEM_AFTER_BRANCH, PROBLEM_NO_STEPS,
    PROBLEM_NO_TRIGGER, PROBLEM_UNKNOWN_STEP, ProblemAt, Slot, Step,
    catalog::{CONDITION, ISSUE_MISSING, NOTIFY, ORDER_PLACED, REVIEW_SUBMITTED, Subject, WAIT},
};

fn ids(steps: &[Step]) -> Vec<&str> {
    steps.iter().map(|step| step.id.as_str()).collect()
}

/// The steps [`Definition::check`] finds after a condition.
fn after_branch(definition: &Definition) -> Vec<ProblemAt> {
    definition
        .check()
        .into_iter()
        .filter(|problem| problem.code == PROBLEM_AFTER_BRANCH)
        .map(|problem| problem.at)
        .collect()
}

#[test]
fn slots_and_lanes_read_back_as_written() {
    for written in ["root.0", "root.12", "s4.1.2", "s10.0.0"] {
        assert_eq!(Slot::parse(written).unwrap().to_string(), written);
    }
    assert_eq!(Lane::parse("s4.1").unwrap().to_string(), "s4.1");
    assert!(Slot::parse("nowhere").is_none());
    assert!(Slot::parse("x4.1.2").is_none());
    assert!(Lane::parse("s4").is_none());
}

#[test]
fn steps_get_ids_never_handed_out_before() {
    let mut definition = Definition::default();
    let first = definition.insert(&root(0), WAIT).unwrap();
    let second = definition.insert(&root(1), NOTIFY).unwrap();
    assert_eq!((first.as_str(), second.as_str()), ("s1", "s2"));
    definition.remove(&second).unwrap();
    // Removed, its id is not handed out again.
    assert_eq!(definition.insert(&root(1), WAIT).unwrap(), "s3");
    assert_eq!(
        definition.insert(&root(9), WAIT),
        Err(EditError::NoSuchSlot)
    );
    assert_eq!(
        definition.insert(&root(0), "teleport"),
        Err(EditError::UnknownKind)
    );
}

#[test]
fn a_condition_holds_a_yes_and_a_no_lane() {
    let mut definition = Definition::default();
    let condition = definition.insert(&root(0), CONDITION).unwrap();
    let yes = definition
        .insert(&branch(&condition, 0, 0), NOTIFY)
        .unwrap();
    let no = definition.insert(&branch(&condition, 1, 0), WAIT).unwrap();
    let step = definition.find(&condition).unwrap();
    assert_eq!(step.branches.len(), 2);
    assert_eq!(ids(&step.branches[0]), vec![yes.as_str()]);
    assert_eq!(ids(&step.branches[1]), vec![no.as_str()]);
    assert_eq!(definition.count(), 3);
    assert_eq!(definition.depth(), 1);
    assert_eq!(definition.locate(&no), Some(branch(&condition, 1, 0)));
    // A plain step has no lane to put anything in.
    assert_eq!(
        definition.insert(&branch(&yes, 0, 0), WAIT),
        Err(EditError::NoSuchSlot)
    );
}

#[test]
fn moving_within_a_lane_counts_the_slot_before_the_move() {
    let mut definition = Definition::default();
    for index in 0..4 {
        definition.insert(&root(index), WAIT).unwrap();
    }
    // s1 to the slot after s3 (index 3 of the lane as shown).
    definition.move_to("s1", &root(3)).unwrap();
    assert_eq!(ids(&definition.steps), vec!["s2", "s3", "s1", "s4"]);
    // To the slot just before or after itself: nothing moves.
    definition.move_to("s1", &root(2)).unwrap();
    definition.move_to("s1", &root(3)).unwrap();
    assert_eq!(ids(&definition.steps), vec!["s2", "s3", "s1", "s4"]);
    definition.move_to("s4", &root(0)).unwrap();
    assert_eq!(ids(&definition.steps), vec!["s4", "s2", "s3", "s1"]);
}

#[test]
fn a_step_moves_between_lanes_with_its_own_lanes() {
    let mut definition = Definition::default();
    let before = definition.insert(&root(0), WAIT).unwrap();
    let outer = definition.insert(&root(1), CONDITION).unwrap();
    let inner = definition.insert(&branch(&outer, 0, 0), CONDITION).unwrap();
    let deep = definition.insert(&branch(&inner, 1, 0), NOTIFY).unwrap();

    // Not into itself, nor into a lane of its own.
    assert_eq!(
        definition.move_to(&outer, &branch(&inner, 0, 0)),
        Err(EditError::IntoItself)
    );
    assert_eq!(
        definition.move_to(&inner, &branch(&inner, 0, 0)),
        Err(EditError::IntoItself)
    );
    // The inner condition goes to the No lane, what it holds with it.
    definition.move_to(&inner, &branch(&outer, 1, 0)).unwrap();
    assert_eq!(definition.locate(&inner), Some(branch(&outer, 1, 0)));
    assert_eq!(definition.locate(&deep), Some(branch(&inner, 1, 0)));
    assert!(definition.find(&outer).unwrap().branches[0].is_empty());
    // A plain step goes into a branch.
    definition.move_to(&before, &branch(&outer, 0, 0)).unwrap();
    assert_eq!(definition.locate(&before), Some(branch(&outer, 0, 0)));
    assert_eq!(ids(&definition.steps), vec![outer.as_str()]);
}

#[test]
fn a_condition_ends_its_lane() {
    let mut definition = Definition::default();
    let wait = definition.insert(&root(0), WAIT).unwrap();
    let notify = definition.insert(&root(1), NOTIFY).unwrap();
    // Put between two steps, a condition takes the steps after it into its
    // Yes lane.
    let condition = definition.insert(&root(1), CONDITION).unwrap();
    assert_eq!(
        ids(&definition.steps),
        vec![wait.as_str(), condition.as_str()]
    );
    assert_eq!(definition.locate(&notify), Some(branch(&condition, 0, 0)));
    // Nothing goes after it.
    assert_eq!(
        definition.insert(&root(2), WAIT),
        Err(EditError::AfterBranch)
    );
    assert_eq!(
        definition.move_to(&notify, &root(2)),
        Err(EditError::AfterBranch)
    );
    // Moved above a step, it takes that step along too, under its own.
    definition.move_to(&condition, &root(0)).unwrap();
    assert_eq!(ids(&definition.steps), vec![condition.as_str()]);
    let yes = &definition.find(&condition).unwrap().branches[0];
    assert_eq!(ids(yes), vec![notify.as_str(), wait.as_str()]);
}

#[test]
fn what_follows_a_condition_ends_its_yes_path() {
    let mut definition = Definition::default();
    let outer = definition.insert(&root(0), CONDITION).unwrap();
    let inner = definition.insert(&branch(&outer, 0, 0), CONDITION).unwrap();
    let wait = definition.insert(&root(0), WAIT).unwrap();
    assert_eq!(ids(&definition.steps), vec![wait.as_str(), outer.as_str()]);
    // The outer Yes lane ends on a condition: the wait goes under that
    // one's Yes lane, not after it.
    definition.move_to(&outer, &root(0)).unwrap();
    assert_eq!(ids(&definition.steps), vec![outer.as_str()]);
    assert_eq!(definition.locate(&wait), Some(branch(&inner, 0, 0)));
    assert_eq!(after_branch(&definition), vec![]);
}

#[test]
fn up_and_down_step_into_and_out_of_a_branch() {
    let mut definition = Definition::default();
    let first = definition.insert(&root(0), WAIT).unwrap();
    let condition = definition.insert(&root(1), CONDITION).unwrap();
    let yes = definition
        .insert(&branch(&condition, 0, 0), NOTIFY)
        .unwrap();

    definition.move_up(&yes).unwrap();
    // Out of the branch, above the condition.
    assert_eq!(
        ids(&definition.steps),
        vec![first.as_str(), yes.as_str(), condition.as_str()]
    );
    definition.move_up(&yes).unwrap();
    definition.move_up(&yes).unwrap();
    // First of the lane under the trigger: nowhere higher to go.
    assert_eq!(
        ids(&definition.steps),
        vec![yes.as_str(), first.as_str(), condition.as_str()]
    );
    // Just above the condition: into its Yes lane, at the top.
    definition.move_down(&first).unwrap();
    assert_eq!(
        ids(&definition.steps),
        vec![yes.as_str(), condition.as_str()]
    );
    assert_eq!(definition.locate(&first), Some(branch(&condition, 0, 0)));
    // Last of its lane, or a condition: the run ends there, nowhere lower
    // to go.
    definition.move_down(&first).unwrap();
    definition.move_down(&condition).unwrap();
    assert_eq!(definition.locate(&first), Some(branch(&condition, 0, 0)));
    assert_eq!(
        ids(&definition.steps),
        vec![yes.as_str(), condition.as_str()]
    );
    // A condition moved up past a step takes it along, under its own.
    definition.move_up(&condition).unwrap();
    assert_eq!(ids(&definition.steps), vec![condition.as_str()]);
    let lane = &definition.find(&condition).unwrap().branches[0];
    assert_eq!(ids(lane), vec![first.as_str(), yes.as_str()]);
}

#[test]
fn conditions_nest_only_so_deep() {
    let mut definition = Definition::default();
    let mut lane = root(0);
    let mut chain = Vec::new();
    for _ in 0..MAX_DEPTH {
        let condition = definition.insert(&lane, CONDITION).unwrap();
        lane = branch(&condition, 0, 0);
        chain.push(condition);
    }
    assert_eq!(definition.depth(), MAX_DEPTH);
    assert_eq!(definition.insert(&lane, CONDITION), Err(EditError::TooDeep));
    // A plain step still fits in the deepest lane.
    definition.insert(&lane, WAIT).unwrap();
    // Moving a condition next to a lane as deep would nest it once more
    // than allowed.
    let other = definition
        .insert(&branch(&chain[0], 1, 0), CONDITION)
        .unwrap();
    assert_eq!(definition.move_to(&other, &lane), Err(EditError::TooDeep));
    // So would one put above the chain, taking it along.
    assert_eq!(definition.condition_depth(&root(0)), Some(MAX_DEPTH + 1));
    assert_eq!(
        definition.insert(&root(0), CONDITION),
        Err(EditError::TooDeep)
    );
    assert_eq!(
        definition.move_to(&other, &root(0)),
        Err(EditError::TooDeep)
    );
    // Up out of the lane holding it, a condition takes the one that held
    // it along: counted without itself inside, it fits.
    definition.remove(&other).unwrap();
    definition.move_up(&chain[1]).unwrap();
    assert_eq!(ids(&definition.steps), vec![chain[1].as_str()]);
    assert!(definition.depth() <= MAX_DEPTH);
    assert_eq!(after_branch(&definition), vec![]);
}

#[test]
fn a_scenario_holds_only_so_many_steps() {
    let mut definition = Definition::default();
    for index in 0..MAX_STEPS {
        definition.insert(&root(index), WAIT).unwrap();
    }
    assert_eq!(
        definition.insert(&root(0), WAIT),
        Err(EditError::TooManySteps)
    );
}

#[test]
fn removing_a_condition_takes_its_lanes_with_it() {
    let mut definition = Definition::default();
    let condition = definition.insert(&root(0), CONDITION).unwrap();
    definition.insert(&branch(&condition, 0, 0), WAIT).unwrap();
    definition.insert(&branch(&condition, 1, 0), WAIT).unwrap();
    assert_eq!(definition.count(), 3);
    definition.remove(&condition).unwrap();
    assert_eq!(definition.count(), 0);
    assert_eq!(definition.remove(&condition), Err(EditError::NoSuchStep));
}

#[test]
fn what_keeps_a_scenario_from_being_published() {
    let empty = Definition::default().check();
    let codes: Vec<&str> = empty.iter().map(|problem| problem.code).collect();
    assert_eq!(codes, vec![PROBLEM_NO_TRIGGER, PROBLEM_NO_STEPS]);

    let ready = big_orders();
    assert_eq!(ready.check(), vec![]);
    assert_eq!(ready.subject(), Some(Subject::Order));

    // A notice without its subject, on its own step.
    let mut definition = ready.clone();
    let condition = definition.steps[0].id.clone();
    let notify = definition
        .insert(&branch(&condition, 1, 0), NOTIFY)
        .unwrap();
    let problems = definition.check();
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].at, ProblemAt::Step(notify));
    assert_eq!(problems[0].key.as_deref(), Some("subject"));
    assert_eq!(problems[0].code, ISSUE_MISSING);

    // Another trigger whose subject has no total: the condition no longer
    // reads.
    let mut definition = ready.clone();
    definition.set_trigger(REVIEW_SUBMITTED).unwrap();
    assert!(!definition.check().is_empty());

    // Drawn otherwise, a step after a condition would never run: it is
    // pointed at, and the draft still saves.
    let mut definition = ready.clone();
    let wait = definition.insert(&root(0), WAIT).unwrap();
    let step = definition.steps.remove(0);
    definition.steps.push(step);
    let problems = definition.check();
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].at, ProblemAt::Step(wait));
    assert_eq!(problems[0].code, PROBLEM_AFTER_BRANCH);
    assert!(!problems[0].is_hard());
}

#[test]
fn a_kind_this_build_does_not_know_is_kept_but_not_published() {
    let json = r#"{"schema":1,"trigger":{"kind":"order.placed"},"steps":[
        {"id":"s1","kind":"teleport","settings":{"where":"mars"}}],"next_id":1}"#;
    let definition = Definition::from_json(json).unwrap();
    let problems = definition.check();
    assert_eq!(problems[0].code, PROBLEM_UNKNOWN_STEP);
    // Written again, the step and its settings are still there.
    let again = Definition::from_json(&definition.to_json()).unwrap();
    assert_eq!(again.steps[0].settings["where"], "mars");
}

#[test]
fn a_newer_format_is_shown_but_not_edited() {
    let mut definition = Definition::from_json(r#"{"schema":99,"steps":[]}"#).unwrap();
    assert!(!definition.is_editable());
    assert_eq!(
        definition.insert(&root(0), WAIT),
        Err(EditError::NewerSchema)
    );
    assert!(definition.check()[0].is_hard());
}

#[test]
fn the_json_is_the_same_for_the_same_definition() {
    let definition = big_orders();
    let json = definition.to_json();
    assert_eq!(Definition::from_json(&json).unwrap(), definition);
    assert_eq!(Definition::from_json(&json).unwrap().to_json(), json);
    // Never saved: nothing in it.
    assert_eq!(Definition::from_json("").unwrap(), Definition::default());
}

#[test]
fn rules_come_and_go_on_a_condition() {
    let mut definition = big_orders();
    let condition = definition.steps[0].id.clone();
    definition.add_rule(&condition).unwrap();
    let settings = &definition.find(&condition).unwrap().settings;
    assert_eq!(settings["rules.1.field"], "order.total");
    definition.remove_rule(&condition, 0).unwrap();
    let settings = &definition.find(&condition).unwrap().settings;
    assert!(!settings.contains_key("rules.0.field"));
    assert!(settings.contains_key("rules.1.field"));
    let wait = definition
        .walk()
        .into_iter()
        .find(|step| step.kind == WAIT)
        .unwrap()
        .id
        .clone();
    assert_eq!(definition.add_rule(&wait), Err(EditError::UnknownKind));
    let _ = ORDER_PLACED;
}
