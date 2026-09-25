//! Time-of-day and weekday primitives used by schedule rules.

use chrono::{Datelike, NaiveDateTime, NaiveTime, Weekday};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{Error, Result};

/// `HH:MM` / `HH:MM:SS` strings are handled by chrono's own serde impl; this
/// module only adds forgiving weekday parsing for hand-written TOML.
fn day_name(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => "mon",
        Weekday::Tue => "tue",
        Weekday::Wed => "wed",
        Weekday::Thu => "thu",
        Weekday::Fri => "fri",
        Weekday::Sat => "sat",
        Weekday::Sun => "sun",
    }
}

fn parse_weekday(raw: &str) -> Option<Weekday> {
    let value = raw.trim().to_ascii_lowercase();
    Some(match value.as_str() {
        "mon" | "monday" => Weekday::Mon,
        "tue" | "tues" | "tuesday" => Weekday::Tue,
        "wed" | "wednesday" => Weekday::Wed,
        "thu" | "thur" | "thurs" | "thursday" => Weekday::Thu,
        "fri" | "friday" => Weekday::Fri,
        "sat" | "saturday" => Weekday::Sat,
        "sun" | "sunday" => Weekday::Sun,
        _ => return None,
    })
}

pub fn serialize_weekdays<S: Serializer>(
    days: &[Weekday],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(days.iter().map(|d| day_name(*d)))
}

pub fn deserialize_weekdays<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<Weekday>, D::Error> {
    let raw = Vec::<String>::deserialize(deserializer)?;
    let mut days = Vec::with_capacity(raw.len());
    for entry in raw {
        match parse_weekday(&entry) {
            Some(day) => {
                if !days.contains(&day) {
                    days.push(day);
                }
            }
            None => {
                return Err(serde::de::Error::custom(format!(
                    "unknown weekday `{entry}` (expected e.g. mon, tue, monday, friday)"
                )))
            }
        }
    }
    Ok(days)
}

/// True when `day` is selected. An empty list means "every day".
pub fn weekday_selected(days: &[Weekday], day: Weekday) -> bool {
    days.is_empty() || days.contains(&day)
}

/// Time-of-day containment supporting ranges that wrap past midnight
/// (e.g. `22:00` → `06:00`).
///
/// `start == end` is treated as an always-open range; validation rejects it
/// in configuration instead.
pub fn time_in_range(start: NaiveTime, end: NaiveTime, value: NaiveTime) -> bool {
    if start == end {
        return true;
    }
    if start < end {
        value >= start && value < end
    } else {
        value >= start || value < end
    }
}

/// A recurring time window, optionally activating a focus profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Schedule {
    /// Unique id.
    pub id: String,

    /// Human-readable label.
    pub name: String,

    /// Days the schedule applies to. Empty means every day.
    #[serde(
        default,
        serialize_with = "serialize_weekdays",
        deserialize_with = "deserialize_weekdays"
    )]
    pub weekdays: Vec<Weekday>,

    /// Window start (inclusive).
    pub start_time: NaiveTime,

    /// Window end (exclusive; may wrap past midnight).
    pub end_time: NaiveTime,

    /// Focus profile to activate while the schedule is live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,

    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Schedule {
    pub fn new(id: impl Into<String>, start_time: NaiveTime, end_time: NaiveTime) -> Self {
        Self {
            id: id.into(),
            name: String::new(),
            weekdays: Vec::new(),
            start_time,
            end_time,
            profile: None,
            enabled: true,
        }
    }

    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn with_weekdays(mut self, weekdays: Vec<Weekday>) -> Self {
        self.weekdays = weekdays;
        self
    }

    pub fn activating_profile(mut self, profile: impl Into<String>) -> Self {
        self.profile = Some(profile.into());
        self
    }

    /// Is this schedule live at `local` (wall-clock)?
    pub fn matches(&self, local: NaiveDateTime) -> bool {
        self.enabled
            && weekday_selected(&self.weekdays, local.weekday())
            && time_in_range(self.start_time, self.end_time, local.time())
    }

    /// Structural validation for persistence.
    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() {
            return Err(Error::validation("schedule id must not be empty"));
        }
        if self.start_time == self.end_time {
            return Err(Error::validation(format!(
                "schedule `{}`: start and end must differ",
                self.id
            )));
        }
        Ok(())
    }
}

/// Schedules live at `local`, in declaration order.
pub fn active_schedules(schedules: &[Schedule], local: NaiveDateTime) -> Vec<&Schedule> {
    schedules.iter().filter(|s| s.matches(local)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    #[test]
    fn normal_range_is_inclusive_start_exclusive_end() {
        assert!(time_in_range(t(9, 0), t(17, 0), t(9, 0)));
        assert!(time_in_range(t(9, 0), t(17, 0), t(16, 59)));
        assert!(!time_in_range(t(9, 0), t(17, 0), t(17, 0)));
        assert!(!time_in_range(t(9, 0), t(17, 0), t(8, 59)));
    }

    #[test]
    fn wrapping_range_crosses_midnight() {
        assert!(time_in_range(t(22, 0), t(6, 0), t(23, 30)));
        assert!(time_in_range(t(22, 0), t(6, 0), t(2, 0)));
        assert!(!time_in_range(t(22, 0), t(6, 0), t(12, 0)));
    }

    #[test]
    fn empty_weekday_list_matches_every_day() {
        assert!(weekday_selected(&[], Weekday::Sat));
    }

    #[test]
    fn weekdays_parse_forgivingly() {
        assert_eq!(parse_weekday("Monday"), Some(Weekday::Mon));
        assert_eq!(parse_weekday(" fri "), Some(Weekday::Fri));
        assert_eq!(parse_weekday("THURSDAY"), Some(Weekday::Thu));
        assert_eq!(parse_weekday("noday"), None);
    }

    fn local(h: u32, m: u32, day: u32) -> NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(2026, 9, day)
            .unwrap()
            .and_hms_opt(h, m, 0)
            .unwrap()
    }

    #[test]
    fn schedule_matches_only_inside_its_window() {
        let schedule = Schedule::new(
            "quiet",
            NaiveTime::from_hms_opt(22, 0, 0).unwrap(),
            NaiveTime::from_hms_opt(6, 0, 0).unwrap(),
        )
        .named("Quiet hours");

        assert!(schedule.matches(local(23, 0, 25)));
        assert!(schedule.matches(local(5, 0, 26)));
        assert!(!schedule.matches(local(12, 0, 25)));

        let disabled = Schedule {
            enabled: false,
            ..schedule
        };
        assert!(!disabled.matches(local(23, 0, 25)));
    }

    #[test]
    fn active_schedules_are_filtered_by_weekday() {
        let schedule = Schedule::new(
            "weekdays",
            NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            NaiveTime::from_hms_opt(17, 0, 0).unwrap(),
        )
        .with_weekdays(vec![Weekday::Mon]);

        // 2026-09-21 is a Monday, 2026-09-26 a Saturday.
        assert_eq!(
            active_schedules(std::slice::from_ref(&schedule), local(10, 0, 21)).len(),
            1
        );
        assert_eq!(
            active_schedules(std::slice::from_ref(&schedule), local(10, 0, 26)).len(),
            0
        );
    }

    #[test]
    fn schedule_validation_rejects_empty_window() {
        let t = NaiveTime::from_hms_opt(9, 0, 0).unwrap();
        let schedule = Schedule::new("bad", t, t);
        assert!(schedule.validate().is_err());

        let ok = Schedule::new(
            "ok",
            NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            NaiveTime::from_hms_opt(17, 0, 0).unwrap(),
        );
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn schedule_round_trips_through_toml() {
        let schedule = Schedule::new(
            "focus",
            NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            NaiveTime::from_hms_opt(12, 0, 0).unwrap(),
        )
        .named("Morning focus")
        .with_weekdays(vec![Weekday::Mon, Weekday::Fri])
        .activating_profile("Deep Work");

        let raw = toml::to_string(&schedule).unwrap();
        let back: Schedule = toml::from_str(&raw).unwrap();
        assert_eq!(schedule, back);
    }
}
