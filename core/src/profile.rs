//! Focus profiles — named, manually toggled sets of preferences — plus the
//! schedule controller that activates them on a time-of-day window.

use std::sync::Mutex;

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::rules::Decision;
use crate::schedule::{active_schedules, Schedule};

/// A named focus profile the user can switch into manually
/// (e.g. "Deep Work", "Meetings", "Away").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusProfile {
    /// Unique profile name, matched case-insensitively by rules.
    pub name: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Decision applied to notifications while this profile is active when no
    /// rule matches first. Falls back to the global default when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_decision: Option<Decision>,
}

impl FocusProfile {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            default_decision: None,
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_default_decision(mut self, decision: Decision) -> Self {
        self.default_decision = Some(decision);
        self
    }
}

/// Runtime holder for the currently active profile (the "manual toggle").
///
/// Shared between the tray/CLI entry points and the context provider so the
/// active profile is visible to rule evaluation immediately.
#[derive(Debug, Default)]
pub struct ActiveProfile {
    current: Mutex<Option<String>>,
}

impl ActiveProfile {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_profile(name: impl Into<String>) -> Self {
        Self {
            current: Mutex::new(Some(name.into())),
        }
    }

    /// Activate a profile. Returns the previously active name.
    pub fn activate(&self, name: impl Into<String>) -> Option<String> {
        let mut guard = self.current.lock().expect("active profile poisoned");
        guard.replace(name.into())
    }

    /// Clear the active profile (back to normal). Returns the previous name.
    pub fn clear(&self) -> Option<String> {
        self.current.lock().expect("active profile poisoned").take()
    }

    /// Toggle: activate when inactive or a different profile, clear when the
    /// named profile is already active.
    pub fn toggle(&self, name: impl Into<String>) -> Option<String> {
        let name = name.into();
        let mut guard = self.current.lock().expect("active profile poisoned");
        if guard.as_deref() == Some(name.as_str()) {
            guard.take()
        } else {
            guard.replace(name)
        }
    }

    pub fn active(&self) -> Option<String> {
        self.current
            .lock()
            .expect("active profile poisoned")
            .clone()
    }

    pub fn is_active(&self, name: &str) -> bool {
        self.current
            .lock()
            .expect("active profile poisoned")
            .as_deref()
            .is_some_and(|active| active.eq_ignore_ascii_case(name))
    }
}

/// Validate that profile names are present and unique.
pub fn validate_profiles(profiles: &[FocusProfile]) -> Result<()> {
    let mut seen: Vec<&str> = Vec::new();
    for profile in profiles {
        let trimmed = profile.name.trim();
        if trimmed.is_empty() {
            return Err(Error::validation("focus profile name must not be empty"));
        }
        if seen.iter().any(|n| n.eq_ignore_ascii_case(trimmed)) {
            return Err(Error::validation(format!(
                "duplicate focus profile `{}`",
                profile.name
            )));
        }
        seen.push(trimmed);
    }
    Ok(())
}

/// Case-insensitive lookup of a profile by name.
pub fn find_profile<'a>(profiles: &'a [FocusProfile], name: &str) -> Option<&'a FocusProfile> {
    profiles.iter().find(|p| p.name.eq_ignore_ascii_case(name))
}

/// Outcome of one schedule-controller tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleTick {
    /// True when the effective profile changed during this tick.
    pub changed: bool,
    /// Profile the controller wants active; `None` means normal mode.
    pub profile: Option<String>,
}

/// Applies schedule-driven profile activation, once per transition.
///
/// The first live schedule (declaration order) that names a profile wins;
/// when nothing is live the profile is cleared. While a schedule is live it
/// owns the slot — a manual toggle made during the window is overwritten at
/// the next tick. Ticks are idempotent: re-evaluating the same state makes
/// no change and reports `changed: false`.
#[derive(Debug, Default)]
pub struct ScheduleController {
    effective: Option<Option<String>>,
}

impl ScheduleController {
    pub fn new() -> Self {
        Self::default()
    }

    /// Evaluate `schedules` at wall-clock `now` and enforce the winner on
    /// `active`.
    pub fn tick(
        &mut self,
        schedules: &[Schedule],
        active: &ActiveProfile,
        now: NaiveDateTime,
    ) -> ScheduleTick {
        let live = active_schedules(schedules, now);
        let want = live.iter().find_map(|schedule| schedule.profile.clone());

        // First tick with nothing live: adopt the current manual state as-is.
        if self.effective.is_none() && want.is_none() && live.is_empty() {
            self.effective = Some(None);
            return ScheduleTick {
                changed: false,
                profile: None,
            };
        }

        if self.effective.as_ref() == Some(&want) {
            return ScheduleTick {
                changed: false,
                profile: want,
            };
        }

        // Transition: schedules with a profile force it; schedules without
        // one (and window expiry) leave the manual toggle alone.

        let was_active = matches!(self.effective, Some(Some(_)));
        if let Some(name) = &want {
            active.activate(name.clone());
        } else if was_active {
            // A profile the controller activated has expired; clear it.
            active.clear();
        }
        self.effective = Some(want.clone());
        ScheduleTick {
            changed: true,
            profile: want,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_activates_and_clears() {
        let active = ActiveProfile::new();
        assert_eq!(active.active(), None);

        active.toggle("Deep Work");
        assert!(active.is_active("deep work"));

        active.toggle("Deep Work");
        assert_eq!(active.active(), None);
    }

    #[test]
    fn switching_profiles_replaces_previous() {
        let active = ActiveProfile::new();
        active.activate("Deep Work");
        assert_eq!(active.activate("Meetings").as_deref(), Some("Deep Work"));
        assert!(active.is_active("Meetings"));
        assert_eq!(active.clear().as_deref(), Some("Meetings"));
        assert_eq!(active.active(), None);
    }

    #[test]
    fn duplicate_profile_names_are_rejected_case_insensitively() {
        let profiles = vec![
            FocusProfile::new("Deep Work"),
            FocusProfile::new("deep work"),
        ];
        assert!(validate_profiles(&profiles).is_err());

        let ok = vec![
            FocusProfile::new("Deep Work"),
            FocusProfile::new("Meetings"),
        ];
        assert!(validate_profiles(&ok).is_ok());
    }

    #[test]
    fn profiles_round_trip_through_toml() {
        let profile = FocusProfile::new("Deep Work")
            .with_description("No distractions")
            .with_default_decision(Decision::Mute);
        let value = toml::to_string(&profile).unwrap();
        let back: FocusProfile = toml::from_str(&value).unwrap();
        assert_eq!(profile, back);
    }

    mod controller {
        use super::*;
        use chrono::NaiveTime;

        fn at(day: u32, hour: u32, minute: u32) -> NaiveDateTime {
            chrono::NaiveDate::from_ymd_opt(2026, 9, day)
                .unwrap()
                .and_hms_opt(hour, minute, 0)
                .unwrap()
        }

        fn night_schedule() -> Schedule {
            Schedule::new(
                "night",
                NaiveTime::from_hms_opt(22, 0, 0).unwrap(),
                NaiveTime::from_hms_opt(6, 0, 0).unwrap(),
            )
            .activating_profile("Deep Work")
        }

        #[test]
        fn activates_on_window_entry_and_clears_on_exit() {
            let active = ActiveProfile::new();
            let mut controller = ScheduleController::new();
            let schedules = vec![night_schedule()];

            let tick = controller.tick(&schedules, &active, at(25, 23, 0));
            assert!(tick.changed);
            assert_eq!(tick.profile.as_deref(), Some("Deep Work"));
            assert_eq!(active.active().as_deref(), Some("Deep Work"));

            // Still inside the window: no change, no churn.
            let tick = controller.tick(&schedules, &active, at(25, 23, 30));
            assert!(!tick.changed);

            // Window over: profile cleared.
            let tick = controller.tick(&schedules, &active, at(26, 12, 0));
            assert!(tick.changed);
            assert_eq!(tick.profile, None);
            assert_eq!(active.active(), None);
        }

        #[test]
        fn schedule_without_profile_leaves_the_toggle_alone() {
            let schedules = vec![Schedule::new(
                "quiet",
                NaiveTime::from_hms_opt(22, 0, 0).unwrap(),
                NaiveTime::from_hms_opt(6, 0, 0).unwrap(),
            )];

            let active = ActiveProfile::new();
            active.activate("Deep Work");
            let mut controller = ScheduleController::new();

            let tick = controller.tick(&schedules, &active, at(25, 23, 0));
            assert!(tick.changed);
            assert_eq!(tick.profile, None);
            // The live schedule names no profile, so the manual toggle wins.
            assert_eq!(active.active().as_deref(), Some("Deep Work"));
        }

        #[test]
        fn first_live_schedule_wins() {
            let schedules = vec![
                Schedule::new(
                    "a",
                    NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                    NaiveTime::from_hms_opt(17, 0, 0).unwrap(),
                )
                .activating_profile("Meetings"),
                Schedule::new(
                    "b",
                    NaiveTime::from_hms_opt(10, 0, 0).unwrap(),
                    NaiveTime::from_hms_opt(12, 0, 0).unwrap(),
                )
                .activating_profile("Deep Work"),
            ];

            let active = ActiveProfile::new();
            let mut controller = ScheduleController::new();
            let tick = controller.tick(&schedules, &active, at(26, 11, 0));
            assert_eq!(tick.profile.as_deref(), Some("Meetings"));
        }
    }
}
