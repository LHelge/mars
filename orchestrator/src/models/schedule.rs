//! The cron expression a scheduled agent fires on (`ARCHITECTURE.md`, "Task
//! tracker" → "Scheduled agents"; ADR 0043).
//!
//! One type, [`CronSchedule`], and the only place `croner` is named: the
//! profile model validates a saved expression through it, the route computes
//! `next_scheduled_at` through it and the scheduler job asks it the one
//! question it has — is an occurrence due in this window — so the crate, the
//! parser's configuration and the clock the expression is read in are decided
//! once.
//!
//! Two rules are worth stating here rather than in a table. The accepted form
//! is **exactly five fields** — minute, hour, day of month, month, day of week
//! — which is what a person knows from `crontab`: `croner` would also accept a
//! seconds field, a year field and the `@daily` family, and every one of them
//! is refused, because an expression whose meaning depends on how many fields
//! it happens to have is a trap and a seconds field promises a precision this
//! has no way to honour. And it is evaluated in **UTC**, always: no time zone
//! is stored on the instance or the profile, and the browser renders the next
//! run in the viewer's own zone (ADR 0043).
//!
//! The finest period the form can express is one minute (`* * * * *`), which
//! is exactly the scheduler's tick, so no further minimum is imposed: there is
//! no expression a user can write that the tick cannot honour.

use chrono::{DateTime, Utc};
use croner::Cron;
use croner::parser::{CronParser, Seconds, Year};

// The crate convention (`CLAUDE.md`, "Backend conventions").
#[allow(unused_imports)]
use crate::prelude::*;

/// How many whitespace-separated fields an accepted expression has.
pub const CRON_FIELDS: usize = 5;

/// Every way a schedule expression can be rejected.
///
/// Both variants are 400s of `SPEC.md`, "Agent profiles", reached through
/// [`crate::models::ProfileError::InvalidScheduleCron`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScheduleError {
    /// Not five fields: an empty string, a seconds or year field, an `@`-form,
    /// or anything else with a different shape.
    ///
    /// The message says what *is* accepted, because the person reading it is
    /// typing the expression and a bare "invalid" leaves them guessing which
    /// dialect this is.
    #[error(
        "schedule_cron must be a 5-field cron expression (minute hour day-of-month month \
         day-of-week) evaluated in UTC, with no seconds field, no year field and no @-form"
    )]
    Shape,
    /// Five fields that `croner` could not parse, with its own complaint.
    #[error("schedule_cron is not a valid cron expression: {0}")]
    Invalid(String),
}

/// A parsed 5-field UTC cron expression and the text it was parsed from.
///
/// The text is kept because it is what the column stores and what the API
/// echoes back: the expression a user typed is theirs, and re-rendering it
/// from the parsed form would hand back a different string for the same
/// schedule.
#[derive(Debug, Clone)]
pub struct CronSchedule {
    expression: String,
    cron: Cron,
}

impl CronSchedule {
    /// Parse `raw` as a 5-field UTC cron expression.
    ///
    /// Trimmed first, then checked for shape before `croner` sees it: the
    /// crate expands `@daily` into a 5-field pattern and accepts 6 and 7
    /// fields under its default configuration, so the field count and the
    /// `@`-forms are decided here and the parser is additionally built to
    /// refuse a seconds or year field on its own.
    pub fn parse(raw: &str) -> std::result::Result<Self, ScheduleError> {
        let expression = raw.trim();
        if expression.contains('@') || expression.split_whitespace().count() != CRON_FIELDS {
            return Err(ScheduleError::Shape);
        }

        let cron = parser()
            .parse(expression)
            .map_err(|error| ScheduleError::Invalid(error.to_string()))?;

        Ok(Self {
            expression: expression.to_string(),
            cron,
        })
    }

    /// The expression as it is stored, trimmed.
    pub fn as_str(&self) -> &str {
        &self.expression
    }

    /// The first occurrence strictly after `after`, in UTC.
    ///
    /// `None` for an expression that matches no instant within `croner`'s
    /// search limit — `0 0 30 2 *`, the 30th of February — which parses and
    /// can never fire. A profile carrying one is not an error to report at
    /// read time: the field is documented as null when there is no next run.
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.cron.find_next_occurrence(&after, false).ok()
    }
}

impl PartialEq for CronSchedule {
    /// Two schedules are the same when their text is, which is what the column
    /// compares and what a test asserts on.
    fn eq(&self, other: &Self) -> bool {
        self.expression == other.expression
    }
}

impl Eq for CronSchedule {}

/// The one parser configuration: five fields, nothing else.
///
/// `Seconds::Disallowed` refuses a 6-field expression outright rather than
/// reading its first field as seconds, and `Year::Disallowed` does the same
/// for a 7-field one, so the crate agrees with the shape check above instead
/// of quietly accepting a richer dialect.
fn parser() -> CronParser {
    CronParser::builder()
        .seconds(Seconds::Disallowed)
        .year(Year::Disallowed)
        .build()
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn a_five_field_expression_parses_and_keeps_its_text() {
        let schedule = CronSchedule::parse("  30 3 * * 1-5  ").expect("five fields parse");
        assert_eq!(schedule.as_str(), "30 3 * * 1-5");
    }

    #[test]
    fn the_next_occurrence_is_in_utc_and_after_the_instant_given() {
        let schedule = CronSchedule::parse("30 3 * * *").expect("five fields parse");
        let after = Utc.with_ymd_and_hms(2026, 9, 22, 4, 0, 0).unwrap();

        let next = schedule.next_after(after).expect("a daily schedule fires");

        assert_eq!(next, Utc.with_ymd_and_hms(2026, 9, 23, 3, 30, 0).unwrap());
    }

    #[test]
    fn an_occurrence_at_the_instant_itself_is_not_the_next_one() {
        let schedule = CronSchedule::parse("* * * * *").expect("five fields parse");
        let after = Utc.with_ymd_and_hms(2026, 9, 22, 4, 0, 0).unwrap();

        assert_eq!(
            schedule.next_after(after),
            Some(Utc.with_ymd_and_hms(2026, 9, 22, 4, 1, 0).unwrap())
        );
    }

    #[test]
    fn a_schedule_that_can_never_fire_has_no_next_occurrence() {
        // Parses, and the 30th of February never arrives.
        let schedule = CronSchedule::parse("0 0 30 2 *").expect("five fields parse");

        assert_eq!(schedule.next_after(Utc::now()), None);
    }

    #[test]
    fn six_and_seven_field_expressions_are_refused() {
        for expression in ["0 30 3 * * *", "0 30 3 * * * 2026", "30 3 * * * 2026"] {
            assert_eq!(
                CronSchedule::parse(expression),
                Err(ScheduleError::Shape),
                "{expression} was accepted"
            );
        }
    }

    #[test]
    fn nicknames_are_refused_however_they_are_spelled() {
        for expression in ["@daily", "@REBOOT", "@every 5m", "@hourly"] {
            assert_eq!(
                CronSchedule::parse(expression),
                Err(ScheduleError::Shape),
                "{expression} was accepted"
            );
        }
    }

    #[test]
    fn an_empty_or_short_expression_is_refused_for_its_shape() {
        for expression in ["", "   ", "*", "* * * *"] {
            assert_eq!(CronSchedule::parse(expression), Err(ScheduleError::Shape));
        }
    }

    #[test]
    fn five_fields_of_garbage_are_refused_with_the_parser_s_own_complaint() {
        let error = CronSchedule::parse("nope not a cron here").expect_err("garbage is refused");

        assert!(
            matches!(&error, ScheduleError::Invalid(detail) if !detail.is_empty()),
            "{error:?}"
        );
        assert!(
            error
                .to_string()
                .starts_with("schedule_cron is not a valid")
        );
    }

    #[test]
    fn an_out_of_range_field_is_refused() {
        for expression in ["70 3 * * *", "0 99 * * *", "0 0 * * 9"] {
            assert!(
                matches!(
                    CronSchedule::parse(expression),
                    Err(ScheduleError::Invalid(_))
                ),
                "{expression} was accepted"
            );
        }
    }

    #[test]
    fn the_shape_message_says_what_is_accepted() {
        let message = ScheduleError::Shape.to_string();

        assert!(message.contains("5-field"));
        assert!(message.contains("UTC"));
        assert!(message.contains("seconds"));
        assert!(message.contains("@-form"));
    }
}
