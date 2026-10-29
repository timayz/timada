//! When a scheduled campaign goes out: the merchant's preset (once, every
//! day, every week on some days, every month on a day), a window of the day
//! and a pace. Pure calendar arithmetic, no store.
//!
//! A *cycle* is one period of the cadence (a day, a week from Monday, a
//! month), named by its first date; each cycle of a recurring campaign is
//! one edition. A cycle's *slots* are the moments its mails go out: every
//! `pace_minutes` from `window_start` until `window_end`, on each of its
//! days. A slot is a time of the local day, so the same slot is a different
//! instant in each time zone: 9:00 in Paris comes six hours before 9:00 in
//! New York, and each subscriber gets their mail at 9:00 where they are.
//!
//! The mails owed are spread over the slots: at each slot passed, the share
//! of the audience mailed so far grows to `passed / slots` ([`quota`]).
//! Recomputed at each look, so a subscriber confirmed mid-window joins the
//! slots left.

use jiff::civil::{Date, Time};
use jiff::tz::TimeZone;
use jiff::{Span, ToSpan};

use crate::error::NewsletterError;

/// How often a scheduled campaign goes out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Cadence {
    /// One edition, in the first cycle (a week) from the start date that has
    /// a slot.
    #[default]
    Once,
    Daily,
    Weekly,
    Monthly,
}

impl Cadence {
    pub const ALL: [Self; 4] = [Self::Once, Self::Daily, Self::Weekly, Self::Monthly];

    /// The value stored in events and forms.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Once => "once",
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|cadence| cadence.as_str() == value)
    }
}

/// The paces a merchant may pick, in minutes.
pub const PACES: [u16; 5] = [15, 30, 60, 120, 240];

/// The last day of the month a monthly campaign may name: every month has
/// it.
pub const MAX_MONTH_DAY: u8 = 28;

/// How many cycles ahead the calendar looks for one with a slot.
const LOOKAHEAD_CYCLES: usize = 60;

/// The furthest a zone is ahead of UTC (Kiritimati), and behind it, in
/// seconds: every zone's 9:00 falls between them.
const MAX_AHEAD_SECS: i64 = 14 * 3600;
const MAX_BEHIND_SECS: i64 = 12 * 3600;

/// A campaign's schedule, as the merchant set it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Schedule {
    pub cadence: Cadence,
    /// ISO weekdays, 1 = Monday … 7 = Sunday, sorted: the days of a weekly
    /// (or one-off) campaign. A one-off one without days goes out on its
    /// start date.
    pub weekdays: Vec<u8>,
    /// The day of the month of a monthly campaign, 1 to [`MAX_MONTH_DAY`].
    pub month_day: Option<u8>,
    /// No slot before this day.
    pub starts_on: Date,
    /// No slot after this day; recurring campaigns only.
    pub ends_on: Option<Date>,
    /// Minutes into the local day of the first slot (540 = 9:00).
    pub window_start: u16,
    /// Minutes into the local day the window closes (no slot at or after);
    /// none is a single slot at `window_start`.
    pub window_end: Option<u16>,
    /// Minutes between two slots of a day, one of [`PACES`].
    pub pace_minutes: u16,
}

/// A moment a scheduled campaign's mails go out: a day and a time of the
/// local day, the same wherever the subscriber is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Slot {
    pub date: Date,
    /// Minutes into the day.
    pub minute: u16,
}

impl Slot {
    /// The instant this slot is in `zone`, in unix seconds. A time that does
    /// not exist that day (a clock moved forward) is the one after the gap.
    pub fn instant_in(&self, zone: &TimeZone) -> i64 {
        let time = Time::new((self.minute / 60) as i8, (self.minute % 60) as i8, 0, 0)
            .unwrap_or(Time::midnight());
        self.date
            .to_datetime(time)
            .to_zoned(zone.clone())
            .map(|zoned| zoned.timestamp().as_second())
            .unwrap_or_else(|_| self.utc_instant())
    }

    /// The instant this slot is in UTC, in unix seconds.
    fn utc_instant(&self) -> i64 {
        self.date
            .to_zoned(TimeZone::UTC)
            .map(|zoned| zoned.timestamp().as_second())
            .unwrap_or_default()
            + i64::from(self.minute) * 60
    }

    /// The first instant this slot comes anywhere on Earth.
    pub fn earliest(&self) -> i64 {
        self.utc_instant() - MAX_AHEAD_SECS
    }

    /// The last instant this slot comes anywhere on Earth.
    pub fn latest(&self) -> i64 {
        self.utc_instant() + MAX_BEHIND_SECS
    }
}

impl Schedule {
    /// The rules a schedule must follow; `today` is the shop's.
    pub fn validate(&self, today: Date) -> Result<(), NewsletterError> {
        let invalid = Err(NewsletterError::InvalidSchedule);
        if self.weekdays.iter().any(|day| !(1..=7).contains(day)) {
            return invalid;
        }
        match self.cadence {
            Cadence::Weekly if self.weekdays.is_empty() => return invalid,
            Cadence::Monthly
                if !self
                    .month_day
                    .is_some_and(|day| (1..=MAX_MONTH_DAY).contains(&day)) =>
            {
                return invalid;
            }
            _ => {}
        }
        if self.window_start >= 24 * 60
            || self
                .window_end
                .is_some_and(|end| end <= self.window_start || end > 24 * 60)
            || !PACES.contains(&self.pace_minutes)
        {
            return invalid;
        }
        if self.starts_on < today || self.ends_on.is_some_and(|ends_on| ends_on < self.starts_on) {
            return invalid;
        }
        if self.first_cycle().is_none() {
            return invalid;
        }
        Ok(())
    }

    /// Whether every cycle is an edition, rather than the first only.
    pub fn is_recurring(&self) -> bool {
        self.cadence != Cadence::Once
    }

    /// The cycle `date` falls in, named by its first day.
    pub fn cycle_of(&self, date: Date) -> Date {
        match self.cadence {
            Cadence::Daily => date,
            Cadence::Monthly => date.first_of_month(),
            Cadence::Once | Cadence::Weekly => date
                .checked_sub(i64::from(date.weekday().to_monday_zero_offset()).days())
                .unwrap_or(date),
        }
    }

    /// The cycle after `cycle`.
    pub fn next_cycle(&self, cycle: Date) -> Date {
        let step: Span = match self.cadence {
            Cadence::Daily => 1.day(),
            Cadence::Monthly => 1.month(),
            Cadence::Once | Cadence::Weekly => 1.week(),
        };
        cycle.checked_add(step).unwrap_or(cycle)
    }

    /// The first cycle on or after the start date with a slot.
    pub fn first_cycle(&self) -> Option<Date> {
        self.cycle_with_slots(self.cycle_of(self.starts_on))
    }

    /// `cycle` or the first one after it with a slot.
    pub fn cycle_with_slots(&self, cycle: Date) -> Option<Date> {
        let mut cycle = self.cycle_of(cycle);
        for _ in 0..LOOKAHEAD_CYCLES {
            if self.ends_on.is_some_and(|ends_on| cycle > ends_on) {
                return None;
            }
            if !self.days(cycle).is_empty() {
                return Some(cycle);
            }
            cycle = self.next_cycle(cycle);
        }
        None
    }

    /// The days of `cycle` the campaign goes out on.
    pub fn days(&self, cycle: Date) -> Vec<Date> {
        let cycle = self.cycle_of(cycle);
        let days: Vec<Date> = match self.cadence {
            Cadence::Daily => vec![cycle],
            Cadence::Monthly => self
                .month_day
                .and_then(|day| {
                    cycle
                        .checked_add(i64::from(day.saturating_sub(1)).days())
                        .ok()
                })
                .into_iter()
                .collect(),
            Cadence::Weekly | Cadence::Once => {
                let weekdays = if self.weekdays.is_empty() {
                    vec![self.starts_on.weekday().to_monday_one_offset() as u8]
                } else {
                    self.weekdays.clone()
                };
                (0..7)
                    .filter_map(|offset| cycle.checked_add(offset.days()).ok())
                    .filter(|day| weekdays.contains(&(day.weekday().to_monday_one_offset() as u8)))
                    .collect()
            }
        };
        days.into_iter()
            .filter(|day| *day >= self.starts_on)
            .filter(|day| self.ends_on.is_none_or(|ends_on| *day <= ends_on))
            .collect()
    }

    /// The minutes of the day the slots of a day fall on.
    pub fn times(&self) -> Vec<u16> {
        let Some(end) = self.window_end else {
            return vec![self.window_start];
        };
        let pace = self.pace_minutes.max(1);
        (self.window_start..end).step_by(pace as usize).collect()
    }

    /// Every slot of `cycle`, in order.
    pub fn slots(&self, cycle: Date) -> Vec<Slot> {
        let times = self.times();
        self.days(cycle)
            .into_iter()
            .flat_map(|date| times.iter().map(move |&minute| Slot { date, minute }))
            .collect()
    }

    /// When `cycle` opens (its first slot, somewhere on Earth) and closes
    /// (its last slot, everywhere, plus one pace for the last mails to go):
    /// unix seconds. `None` for a cycle without a slot.
    pub fn window_of(&self, cycle: Date) -> Option<(i64, i64)> {
        let slots = self.slots(cycle);
        let first = slots.first()?;
        let last = slots.last()?;
        Some((
            first.earliest(),
            last.latest() + i64::from(self.pace_minutes) * 60,
        ))
    }

    /// The next `count` days with a slot, from the cycle `from` falls in.
    pub fn upcoming_days(&self, from: Date, count: usize) -> Vec<Date> {
        let mut days = Vec::new();
        let mut cycle = self.cycle_with_slots(from.max(self.starts_on));
        while let Some(current) = cycle {
            days.extend(self.days(current).into_iter().filter(|day| *day >= from));
            if days.len() >= count || !self.is_recurring() {
                break;
            }
            cycle = self.cycle_with_slots(self.next_cycle(current));
        }
        days.truncate(count);
        days
    }
}

/// How many of `audience` should have been mailed once `passed` of `slots`
/// went by: an even share per slot, rounded up, everyone at the last.
pub fn quota(audience: u64, passed: usize, slots: usize) -> u64 {
    if slots == 0 || passed >= slots {
        return audience;
    }
    (audience * passed as u64).div_ceil(slots as u64)
}

/// A `YYYY-MM-DD` day.
pub fn parse_day(value: &str) -> Option<Date> {
    value.trim().parse().ok()
}

/// `HH:MM` as minutes into the day.
pub fn parse_time_of_day(value: &str) -> Option<u16> {
    let (hours, minutes) = value.trim().split_once(':')?;
    let (hours, minutes): (u16, u16) = (hours.parse().ok()?, minutes.get(..2)?.parse().ok()?);
    (hours < 24 && minutes < 60).then_some(hours * 60 + minutes)
}

/// Minutes into the day as `HH:MM`.
pub fn format_time_of_day(minute: u16) -> String {
    format!("{:02}:{:02}", minute / 60, minute % 60)
}

/// The English name of an ISO weekday (1 = Monday).
pub fn weekday_name(day: u8) -> &'static str {
    match day {
        1 => "Monday",
        2 => "Tuesday",
        3 => "Wednesday",
        4 => "Thursday",
        5 => "Friday",
        6 => "Saturday",
        _ => "Sunday",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(value: &str) -> Date {
        parse_day(value).unwrap()
    }

    /// Tuesdays and Thursdays, 9:00 to 17:00, every 30 minutes.
    fn tue_thu() -> Schedule {
        Schedule {
            cadence: Cadence::Weekly,
            weekdays: vec![2, 4],
            month_day: None,
            starts_on: day("2026-11-02"),
            ends_on: None,
            window_start: 9 * 60,
            window_end: Some(17 * 60),
            pace_minutes: 30,
        }
    }

    #[test]
    fn a_week_of_tuesdays_and_thursdays_has_thirty_two_slots() {
        let schedule = tue_thu();
        let cycle = schedule.first_cycle().unwrap();
        assert_eq!(cycle, day("2026-11-02"));
        let slots = schedule.slots(cycle);
        assert_eq!(slots.len(), 32);
        assert_eq!(
            slots[0],
            Slot {
                date: day("2026-11-03"),
                minute: 540
            }
        );
        assert_eq!(
            slots[31],
            Slot {
                date: day("2026-11-05"),
                minute: 16 * 60 + 30
            }
        );
        assert_eq!(schedule.next_cycle(cycle), day("2026-11-09"));
    }

    #[test]
    fn a_thousand_subscribers_go_out_evenly_over_the_slots() {
        assert_eq!(quota(1000, 0, 32), 0);
        assert_eq!(quota(1000, 1, 32), 32);
        assert_eq!(quota(1000, 16, 32), 500);
        assert_eq!(quota(1000, 31, 32), 969);
        assert_eq!(quota(1000, 32, 32), 1000);
        // Late joiners raise the slots left.
        assert_eq!(quota(1100, 17, 32), 585);
        assert_eq!(quota(3, 1, 0), 3);
    }

    #[test]
    fn nine_oclock_is_a_different_instant_in_each_zone() {
        let slot = Slot {
            date: day("2026-11-03"),
            minute: 540,
        };
        let paris = TimeZone::get("Europe/Paris").unwrap();
        let new_york = TimeZone::get("America/New_York").unwrap();
        let kiritimati = TimeZone::get("Pacific/Kiritimati").unwrap();
        // 9:00 in Paris (UTC+1 in November) is 8:00 UTC.
        assert_eq!(slot.instant_in(&paris), slot.utc_instant() - 3600);
        assert_eq!(
            slot.instant_in(&new_york) - slot.instant_in(&paris),
            6 * 3600
        );
        assert_eq!(slot.instant_in(&kiritimati), slot.earliest());
        assert!(slot.instant_in(&new_york) < slot.latest());
    }

    #[test]
    fn a_slot_in_a_skipped_hour_goes_out_after_the_gap() {
        // Paris moved from 2:00 to 3:00 on 2026-03-29.
        let paris = TimeZone::get("Europe/Paris").unwrap();
        let skipped = Slot {
            date: day("2026-03-29"),
            minute: 2 * 60 + 30,
        };
        let after = Slot {
            date: day("2026-03-29"),
            minute: 3 * 60 + 30,
        };
        assert_eq!(skipped.instant_in(&paris), after.instant_in(&paris));
    }

    #[test]
    fn each_cadence_has_its_cycles() {
        let monthly = Schedule {
            cadence: Cadence::Monthly,
            weekdays: vec![],
            month_day: Some(1),
            window_end: None,
            starts_on: day("2026-11-15"),
            ..tue_thu()
        };
        // The first of November is before the start: December's first.
        assert_eq!(monthly.first_cycle(), Some(day("2026-12-01")));
        assert_eq!(monthly.slots(day("2026-12-01")).len(), 1);
        assert_eq!(monthly.next_cycle(day("2026-12-01")), day("2027-01-01"));

        let daily = Schedule {
            cadence: Cadence::Daily,
            window_start: 600,
            window_end: Some(720),
            pace_minutes: 60,
            ..tue_thu()
        };
        assert_eq!(daily.slots(day("2026-11-04")).len(), 2);

        // Once, without days: the start date, at its time.
        let once = Schedule {
            cadence: Cadence::Once,
            weekdays: vec![],
            starts_on: day("2026-11-04"),
            window_end: None,
            ..tue_thu()
        };
        assert_eq!(
            once.slots(once.first_cycle().unwrap()),
            [Slot {
                date: day("2026-11-04"),
                minute: 540
            }]
        );
        assert!(!once.is_recurring());

        // Starting on a Wednesday: this week's Thursday only.
        let late = Schedule {
            starts_on: day("2026-11-04"),
            ..tue_thu()
        };
        assert_eq!(late.slots(late.first_cycle().unwrap()).len(), 16);
        assert_eq!(
            late.upcoming_days(day("2026-11-04"), 3),
            [day("2026-11-05"), day("2026-11-10"), day("2026-11-12")]
        );
    }

    #[test]
    fn bad_schedules_are_refused() {
        let today = day("2026-11-01");
        assert!(tue_thu().validate(today).is_ok());
        for bad in [
            Schedule {
                weekdays: vec![],
                ..tue_thu()
            },
            Schedule {
                weekdays: vec![8],
                ..tue_thu()
            },
            Schedule {
                window_end: Some(9 * 60),
                ..tue_thu()
            },
            Schedule {
                pace_minutes: 7,
                ..tue_thu()
            },
            Schedule {
                starts_on: day("2026-10-01"),
                ..tue_thu()
            },
            Schedule {
                ends_on: Some(day("2026-11-01")),
                ..tue_thu()
            },
            Schedule {
                cadence: Cadence::Monthly,
                month_day: Some(31),
                ..tue_thu()
            },
            // Ends before any of its days.
            Schedule {
                ends_on: Some(day("2026-11-02")),
                ..tue_thu()
            },
        ] {
            assert!(bad.validate(today).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn times_of_day_read_and_print() {
        assert_eq!(parse_time_of_day("09:00"), Some(540));
        assert_eq!(parse_time_of_day("17:30"), Some(1050));
        assert_eq!(parse_time_of_day("24:00"), None);
        assert_eq!(parse_time_of_day("nine"), None);
        assert_eq!(format_time_of_day(545), "09:05");
    }
}
