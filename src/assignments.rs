//! Which set each monitor shows.
//!
//! Monitors are keyed by their "make model serial" identifier (the string
//! kanshi and sway's `output` command match on), so a monitor keeps its set
//! when it is plugged into a different port. A set is shown on at most one
//! monitor at a time.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Assignments(BTreeMap<String, String>);

/// What showing a set on a monitor involves.
#[derive(Debug, PartialEq, Eq)]
pub enum Change {
    /// The monitor already shows the set.
    Unchanged,
    /// Another connected monitor shows the set; it takes `old` in exchange.
    Swap { monitor: String, old: String },
    /// No connected monitor shows the set; `old` is put away.
    Replace { old: String },
}

impl Assignments {
    pub fn get(&self, monitor: &str) -> Option<&str> {
        self.0.get(monitor).map(String::as_str)
    }

    /// The set `monitor` shows. A monitor without one gets the first of
    /// `known`, then of `main`, `main2`, `main3`..., not shown on another
    /// connected monitor.
    pub fn resolve(&self, monitor: &str, connected: &[String], known: &[String]) -> String {
        if let Some(set) = self.get(monitor) {
            return set.to_string();
        }
        let free = |set: &String| self.holder(monitor, set, connected).is_none();
        if let Some(set) = known.iter().find(|s| free(s)) {
            return set.clone();
        }
        (1..)
            .map(|n| match n {
                1 => "main".to_string(),
                n => format!("main{n}"),
            })
            .find(free)
            .unwrap_or_default()
    }

    /// Plan showing `set` on `monitor`, which currently shows `current`.
    pub fn plan(&self, monitor: &str, current: &str, set: &str, connected: &[String]) -> Change {
        if current == set {
            return Change::Unchanged;
        }
        match self.holder(monitor, set, connected) {
            Some(other) => Change::Swap {
                monitor: other.to_string(),
                old: current.to_string(),
            },
            None => Change::Replace {
                old: current.to_string(),
            },
        }
    }

    /// Show `set` on `monitor`, taking it off any other monitor.
    pub fn assign(&mut self, monitor: &str, set: &str) {
        self.0.retain(|_, s| s != set);
        self.0.insert(monitor.to_string(), set.to_string());
    }

    fn holder<'a>(&self, monitor: &str, set: &str, connected: &'a [String]) -> Option<&'a str> {
        connected
            .iter()
            .find(|m| m.as_str() != monitor && self.get(m) == Some(set))
            .map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use crate::assignments::{Assignments, Change};

    const DELL: &str = "Dell Inc. DELL U3415W PXF798BL0E4L";
    const BENQ: &str = "BNQ BenQ GL2780 ETXAK01950SL0";
    const LAPTOP: &str = "Sharp Corporation LQ156M1JW03 Unknown";

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn known() -> Vec<String> {
        strings(&["main", "work", "scratch"])
    }

    fn with(pairs: &[(&str, &str)]) -> Assignments {
        let mut a = Assignments::default();
        for (monitor, set) in pairs {
            a.assign(monitor, set);
        }
        a
    }

    #[test]
    fn resolve_returns_existing_assignment() {
        let a = with(&[(DELL, "work")]);
        assert_eq!(a.resolve(DELL, &strings(&[DELL]), &known()), "work");
    }

    #[test]
    fn resolve_keeps_assignment_to_unknown_set() {
        let a = with(&[(DELL, "adhoc")]);
        assert_eq!(a.resolve(DELL, &strings(&[DELL]), &known()), "adhoc");
    }

    #[test]
    fn resolve_new_monitor_gets_first_known_set() {
        let a = Assignments::default();
        assert_eq!(a.resolve(DELL, &strings(&[DELL]), &known()), "main");
    }

    #[test]
    fn resolve_new_monitor_skips_sets_on_other_connected_monitors() {
        let a = with(&[(DELL, "main"), (BENQ, "work")]);
        let connected = strings(&[DELL, BENQ, LAPTOP]);
        assert_eq!(a.resolve(LAPTOP, &connected, &known()), "scratch");
    }

    #[test]
    fn resolve_new_monitor_may_take_set_of_disconnected_monitor() {
        let a = with(&[(BENQ, "main")]);
        assert_eq!(a.resolve(DELL, &strings(&[DELL]), &known()), "main");
    }

    #[test]
    fn resolve_invents_set_when_all_known_are_taken() {
        let a = with(&[(DELL, "main"), (BENQ, "work")]);
        let connected = strings(&[DELL, BENQ, LAPTOP]);
        let known = strings(&["main", "work"]);
        assert_eq!(a.resolve(LAPTOP, &connected, &known), "main2");
    }

    #[test]
    fn resolve_invented_set_avoids_known_names() {
        let a = with(&[(DELL, "main"), (BENQ, "main2")]);
        let connected = strings(&[DELL, BENQ, LAPTOP]);
        let known = strings(&["main", "main2"]);
        assert_eq!(a.resolve(LAPTOP, &connected, &known), "main3");
    }

    #[test]
    fn resolve_with_no_known_sets_uses_main() {
        let a = Assignments::default();
        assert_eq!(a.resolve(DELL, &strings(&[DELL]), &[]), "main");
    }

    #[test]
    fn resolve_invented_set_avoids_unknown_sets_on_other_monitors() {
        let a = with(&[(DELL, "main"), (BENQ, "main2")]);
        let connected = strings(&[DELL, BENQ, LAPTOP]);
        assert_eq!(a.resolve(LAPTOP, &connected, &strings(&["main"])), "main3");
    }

    #[test]
    fn plan_same_set_is_unchanged() {
        let a = with(&[(DELL, "work")]);
        let change = a.plan(DELL, "work", "work", &strings(&[DELL]));
        assert_eq!(change, Change::Unchanged);
    }

    #[test]
    fn plan_set_on_other_connected_monitor_swaps() {
        let a = with(&[(DELL, "main"), (BENQ, "work")]);
        let change = a.plan(DELL, "main", "work", &strings(&[DELL, BENQ]));
        assert_eq!(
            change,
            Change::Swap {
                monitor: BENQ.to_string(),
                old: "main".to_string(),
            }
        );
    }

    #[test]
    fn plan_set_on_disconnected_monitor_replaces() {
        let a = with(&[(DELL, "main"), (BENQ, "work")]);
        let change = a.plan(DELL, "main", "work", &strings(&[DELL]));
        assert_eq!(
            change,
            Change::Replace {
                old: "main".to_string(),
            }
        );
    }

    #[test]
    fn plan_unshown_set_replaces() {
        let a = with(&[(DELL, "main"), (BENQ, "work")]);
        let change = a.plan(DELL, "main", "scratch", &strings(&[DELL, BENQ]));
        assert_eq!(
            change,
            Change::Replace {
                old: "main".to_string(),
            }
        );
    }

    #[test]
    fn assign_takes_set_off_other_monitor() {
        let mut a = with(&[(BENQ, "work")]);
        a.assign(DELL, "work");
        assert_eq!(a.get(DELL), Some("work"));
        assert_eq!(a.get(BENQ), None);
    }

    #[test]
    fn assign_replaces_monitor_set() {
        let mut a = with(&[(DELL, "main")]);
        a.assign(DELL, "work");
        assert_eq!(a.get(DELL), Some("work"));
    }

    #[test]
    fn swap_leaves_each_set_on_one_monitor() {
        let mut a = with(&[(DELL, "main"), (BENQ, "work")]);
        a.assign(DELL, "work");
        a.assign(BENQ, "main");
        assert_eq!(a, with(&[(DELL, "work"), (BENQ, "main")]));
    }

    #[test]
    fn serializes_as_plain_object() -> Result<(), serde_json::Error> {
        let a = with(&[(DELL, "work"), (LAPTOP, "main")]);
        let json = serde_json::to_string(&a)?;
        assert_eq!(json, format!(r#"{{"{DELL}":"work","{LAPTOP}":"main"}}"#));
        assert_eq!(serde_json::from_str::<Assignments>(&json)?, a);
        Ok(())
    }
}
