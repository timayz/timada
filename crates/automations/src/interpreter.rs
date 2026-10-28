//! What a run does next: a pure walk of the scenario's definition (the
//! version the run started on) against the run's log, at a given time.
//!
//! The walk goes down the lane under the trigger, step by step. A step the
//! log has done (it acted, it had nothing to do, its wait is over) is
//! passed; a condition sends the walk down the lane it chose, and the run
//! ends where that lane ends, as nothing comes after a condition; the first
//! step not done is what comes next.

use crate::catalog::{self, CONDITION, WAIT};
use crate::command::{Outcome, RunState};
use crate::definition::{Definition, Step};
use crate::value_object::CLAIM_LEASE_SECS;

/// Why a step cannot run, as a run's log says it.
pub const STUCK_UNKNOWN: &str = "This build does not know this block.";
pub const STUCK_SETTINGS: &str = "The block's settings do not read.";
pub const STUCK_LANE: &str = "The run went down a lane the block does not have.";
pub const STUCK_FAILED: &str = "The block failed.";

/// What the run does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next<'a> {
    /// Nothing before then: a wait not over, a step to try again later, a
    /// step another node is acting on.
    Sleep(u64),
    /// A wait to begin, until then.
    Wait(&'a Step, u64),
    /// A condition to check.
    Branch(&'a Step),
    /// A step that acts outside the run: to take (claim), then act.
    Act(&'a Step),
    /// A step taken to act whose outcome never came within the lease: the
    /// node acting stopped. Its attempt is given.
    Lapsed(&'a Step, u32),
    /// A step that cannot run, and why.
    Stuck(&'a Step, &'static str),
    /// Every step reached is done.
    Finish,
}

pub fn next<'a>(definition: &'a Definition, run: &RunState, now: u64) -> Next<'a> {
    walk(&definition.steps, run, now).unwrap_or(Next::Finish)
}

/// The first thing to do in a lane; `None` once the lane is done.
fn walk<'a>(steps: &'a [Step], run: &RunState, now: u64) -> Option<Next<'a>> {
    for step in steps {
        let Some(record) = run.latest(&step.id) else {
            return Some(begin(step, now));
        };
        match &record.outcome {
            Outcome::Waiting { until } => {
                if now < *until {
                    return Some(Next::Sleep(*until));
                }
            }
            Outcome::Branched(branch) => {
                let Some(lane) = step.branches.get(*branch as usize) else {
                    return Some(Next::Stuck(step, STUCK_LANE));
                };
                return walk(lane, run, now);
            }
            Outcome::Succeeded { .. } | Outcome::Skipped { .. } => {}
            Outcome::Claimed { attempt } => {
                let lapses = record.at + CLAIM_LEASE_SECS;
                return Some(if now >= lapses {
                    Next::Lapsed(step, *attempt)
                } else {
                    Next::Sleep(lapses)
                });
            }
            Outcome::Failed {
                retry_at: Some(at), ..
            } => {
                return Some(if now >= *at {
                    Next::Act(step)
                } else {
                    Next::Sleep(*at)
                });
            }
            Outcome::Failed { retry_at: None, .. } => return Some(Next::Stuck(step, STUCK_FAILED)),
        }
    }
    None
}

fn begin(step: &Step, now: u64) -> Next<'_> {
    match step.kind.as_str() {
        WAIT => match catalog::parse_wait(&step.settings) {
            Ok(wait) => Next::Wait(step, now + wait.secs()),
            Err(_) => Next::Stuck(step, STUCK_SETTINGS),
        },
        CONDITION => Next::Branch(step),
        kind if catalog::step_kind(kind).is_some() => Next::Act(step),
        _ => Next::Stuck(step, STUCK_UNKNOWN),
    }
}
