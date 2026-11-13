//! Shift single tap: the shell-local state machine behind "tap Shift to switch Chinese / English".
//! Pure: the clock and the key states are passed in, so it is
//! tested without a keyboard. It is not part of the engine session and sends nothing by itself.

use std::time::{Duration, Instant};

/// Uses the key-up scan code; keyboard state can already have changed at release.
pub fn toggle_enabled(toggle: twime_proto::ToggleKey, lparam: isize) -> bool {
    use twime_proto::ToggleKey;
    let right = match (lparam >> 16) & 0xff {
        0x2a => false,
        0x36 => true,
        _ => lparam & (1 << 24) != 0,
    };
    match toggle {
        ToggleKey::Shift => true,
        ToggleKey::LeftShift => !right,
        ToggleKey::RightShift => right,
        ToggleKey::None => false,
    }
}

/// A press longer than this is not a tap.
pub const TAP_LIMIT: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Nothing pending; a release that finds this state is not a tap (it did not start with a clean press we saw).
    Idle,
    /// A clean Shift press, nothing else happened since.
    Armed(Instant),
    /// This press is spoiled (another key, a modifier, the other Shift ...) until every Shift is released.
    Void,
}

/// What the shell knows at the moment of a Shift release.
#[derive(Clone, Copy, Debug, Default)]
pub struct Release {
    /// The other Shift key is held.
    pub other_shift_held: bool,
    /// Ctrl, Alt or Win is held.
    pub command_held: bool,
    /// The setting "Shift tap switches the mode" is on.
    pub enabled: bool,
    /// The context is synchronised with a version 3 engine and writable.
    pub ready: bool,
    /// Caps Lock is on.
    pub caps: bool,
}

impl Release {
    fn first_failure(&self) -> Option<&'static str> {
        if self.other_shift_held {
            Some("other_shift_held")
        } else if self.command_held {
            Some("command_held")
        } else if !self.enabled {
            Some("enabled")
        } else if !self.ready {
            Some("ready")
        } else if self.caps {
            Some("caps")
        } else {
            None
        }
    }
}

#[derive(Debug)]
pub struct ShiftTap {
    state: State,
    /// Diagnostics only: retain the last arming time after the press is spoiled or consumed.
    trace_armed_at: Option<Instant>,
}

impl Default for ShiftTap {
    fn default() -> Self {
        ShiftTap::new()
    }
}

impl ShiftTap {
    fn transition(&mut self, state: State, reason: &'static str) {
        if crate::trace::enabled() && self.state != state {
            crate::trace::log(&format!(
                "shift.state old={} new={} reason={reason}",
                self.state_name(),
                match state {
                    State::Idle => "Idle",
                    State::Armed(_) => "Armed",
                    State::Void => "Void",
                }
            ));
        }
        self.state = state;
    }

    fn state_name(&self) -> &'static str {
        match self.state {
            State::Idle => "Idle",
            State::Armed(_) => "Armed",
            State::Void => "Void",
        }
    }

    pub fn new() -> ShiftTap {
        ShiftTap {
            state: State::Idle,
            trace_armed_at: None,
        }
    }

    /// Shift went down. `new_press` is false for an auto-repeat notification (bit 30 of the key-down lParam), which
    /// changes nothing. A new press starts a new round whatever was left over, so a missing key-up never sticks.
    pub fn shift_down(
        &mut self,
        now: Instant,
        new_press: bool,
        other_shift_held: bool,
        command_held: bool,
        ready: bool,
    ) {
        if !new_press {
            if crate::trace::enabled() {
                crate::trace::log(&format!(
                    "shift.repeat state={} reason=repeat",
                    self.state_name()
                ));
            }
            return;
        }
        let reason = if other_shift_held {
            "other shift"
        } else if command_held {
            "modifier"
        } else if !ready {
            "not ready"
        } else {
            "new press"
        };
        self.trace_armed_at = if !other_shift_held && !command_held && ready {
            Some(now)
        } else if matches!(self.state, State::Armed(_)) {
            self.trace_armed_at
        } else {
            None
        };
        self.transition(
            if !other_shift_held && !command_held && ready {
                State::Armed(now)
            } else {
                State::Void
            },
            reason,
        );
    }

    /// Any other key went down (key test or key down), or Ctrl / Alt / Win: the tap is spoiled.
    pub fn other_key_down(&mut self) {
        self.other_key_down_reason("other key");
    }

    pub fn other_key_down_reason(&mut self, reason: &'static str) {
        if matches!(self.state, State::Armed(_)) {
            self.transition(State::Void, reason);
        }
    }

    /// Focus or context changed, the TIP was deactivated: forget everything. A Shift still held then is not a tap when
    /// it is released (the state is `Idle`).
    pub fn reset(&mut self) {
        self.reset_reason("focus reset");
    }

    pub fn reset_reason(&mut self, reason: &'static str) {
        self.transition(State::Idle, reason);
        self.trace_armed_at = None;
    }

    fn after_release(&mut self, other_shift_held: bool, reason: &'static str) {
        self.transition(
            if other_shift_held {
                State::Void
            } else {
                State::Idle
            },
            reason,
        );
    }

    fn first_failure(&self, now: Instant, r: &Release) -> Option<&'static str> {
        match self.state {
            State::Idle | State::Void => Some("armed"),
            State::Armed(t) if now.saturating_duration_since(t) >= TAP_LIMIT => Some("timeout"),
            State::Armed(_) => r.first_failure(),
        }
    }

    fn trace_release(
        &self,
        callback: &'static str,
        now: Instant,
        r: &Release,
        failure: Option<&'static str>,
    ) {
        if crate::trace::enabled() {
            let armed_ms = self
                .trace_armed_at
                .map(|t| now.saturating_duration_since(t).as_millis());
            crate::trace::log(&format!(
                "shift.release callback={callback} state={} other_shift_held={} command_held={} enabled={} ready={} caps={} armed_ms={armed_ms:?} result={} first_failure={}",
                self.state_name(),
                r.other_shift_held,
                r.command_held,
                r.enabled,
                r.ready,
                r.caps,
                failure.is_none(),
                failure.unwrap_or("none")
            ));
        }
    }

    /// `OnTestKeyUp(Shift)`: consumes a clean tap before the caller sends `ToggleMode`.
    /// Repeated tests and a following `OnKeyUp` cannot consume the same press again.
    pub fn test_up(&mut self, now: Instant, r: &Release) -> bool {
        let failure = self.first_failure(now, r);
        let ok = failure.is_none();
        self.trace_release("OnTestKeyUp", now, r, failure);
        self.after_release(r.other_shift_held, failure.unwrap_or("tap consumed"));
        ok
    }

    /// `OnKeyUp(Shift)`: consumes a clean tap if no preceding `OnTestKeyUp` consumed it.
    /// Checks the current release conditions; whatever the result, the tap is used up.
    pub fn key_up(&mut self, now: Instant, r: &Release) -> bool {
        let failure = self.first_failure(now, r);
        let ok = failure.is_none();
        self.trace_release("OnKeyUp", now, r, failure);
        self.after_release(r.other_shift_held, failure.unwrap_or("tap consumed"));
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_up_then_key_up_toggles_once_at_test_up() {
        let now = Instant::now();
        let mut t = ShiftTap::new();
        t.shift_down(now, true, false, false, true);
        let mut toggles = usize::from(t.test_up(now, &ok()));
        assert_eq!(toggles, 1, "toggle happens during TestUp");
        toggles += usize::from(t.key_up(now, &ok()));
        assert_eq!(toggles, 1);
    }

    #[test]
    fn duplicate_test_up_toggles_once() {
        let now = Instant::now();
        let mut t = ShiftTap::new();
        t.shift_down(now, true, false, false, true);
        assert!(t.test_up(now, &ok()));
        assert!(!t.test_up(now, &ok()));
        assert!(!t.key_up(now, &ok()));
    }

    #[test]
    fn key_up_without_test_up_toggles_once_at_key_up() {
        let now = Instant::now();
        let mut t = ShiftTap::new();
        t.shift_down(now, true, false, false, true);
        assert!(t.key_up(now, &ok()));
        assert!(!t.key_up(now, &ok()));
        assert!(!t.test_up(now, &ok()));
    }

    #[test]
    fn another_key_while_armed_prevents_either_release_from_toggling() {
        let now = Instant::now();
        for test_first in [false, true] {
            let mut t = ShiftTap::new();
            t.shift_down(now, true, false, false, true);
            t.other_key_down();
            if test_first {
                assert!(!t.test_up(now, &ok()));
            }
            assert!(!t.key_up(now, &ok()));
        }
    }

    #[test]
    fn first_failure_follows_the_actual_short_circuit_order() {
        let now = Instant::now();
        let mut t = ShiftTap::new();
        let mut r = Release {
            other_shift_held: true,
            command_held: true,
            enabled: false,
            ready: false,
            caps: true,
        };
        assert_eq!(t.first_failure(now, &r), Some("armed"));
        t.shift_down(now, true, false, false, true);
        assert_eq!(t.first_failure(now + TAP_LIMIT, &r), Some("timeout"));
        assert_eq!(t.first_failure(now, &r), Some("other_shift_held"));
        r.other_shift_held = false;
        assert_eq!(t.first_failure(now, &r), Some("command_held"));
        r.command_held = false;
        assert_eq!(t.first_failure(now, &r), Some("enabled"));
        r.enabled = true;
        assert_eq!(t.first_failure(now, &r), Some("ready"));
        r.ready = true;
        assert_eq!(t.first_failure(now, &r), Some("caps"));
        r.caps = false;
        assert_eq!(t.first_failure(now, &r), None);
        t.other_key_down();
        assert_eq!(t.first_failure(now, &r), Some("armed"));
    }

    #[test]
    fn key_up_scan_code_selects_the_configured_side() {
        use twime_proto::ToggleKey;
        for (lp, right) in [
            (0x2a << 16, false),
            (0x36 << 16, true),
            (1 << 24, true),
            (0, false),
            ((0x2a << 16) | (1 << 24), false),
        ] {
            assert!(toggle_enabled(ToggleKey::Shift, lp));
            assert_eq!(toggle_enabled(ToggleKey::LeftShift, lp), !right);
            assert_eq!(toggle_enabled(ToggleKey::RightShift, lp), right);
            assert!(!toggle_enabled(ToggleKey::None, lp));
        }
    }

    fn ok() -> Release {
        Release {
            other_shift_held: false,
            command_held: false,
            enabled: true,
            ready: true,
            caps: false,
        }
    }

    struct Clock(Instant);
    impl Clock {
        fn at(&self, ms: u64) -> Instant {
            self.0 + Duration::from_millis(ms)
        }
    }

    /// Plays one press and release; returns the number of ToggleMode events (0 or 1).
    fn tap(
        t: &mut ShiftTap,
        c: &Clock,
        down_ms: u64,
        up_ms: u64,
        r: Release,
        between: impl FnOnce(&mut ShiftTap),
    ) -> usize {
        t.shift_down(c.at(down_ms), true, false, false, true);
        between(t);
        let sent = t.test_up(c.at(up_ms), &r);
        assert!(!t.key_up(c.at(up_ms), &r));
        usize::from(sent)
    }

    #[test]
    fn a_clean_tap_sends_exactly_one_toggle() {
        let c = Clock(Instant::now());
        let mut t = ShiftTap::new();
        assert_eq!(tap(&mut t, &c, 0, 120, ok(), |_| {}), 1);
        // The next tap works the same: no residue.
        assert_eq!(tap(&mut t, &c, 1000, 1100, ok(), |_| {}), 1);
        // The release alone, repeated, never sends again.
        assert!(!t.test_up(c.at(1200), &ok()));
        assert!(!t.key_up(c.at(1200), &ok()));
    }

    #[test]
    fn typing_a_capital_letter_is_not_a_tap() {
        let c = Clock(Instant::now());
        let mut t = ShiftTap::new();
        assert_eq!(tap(&mut t, &c, 0, 200, ok(), |t| t.other_key_down()), 0);
        // And the state does not stay spoiled.
        assert_eq!(tap(&mut t, &c, 1000, 1100, ok(), |_| {}), 1);
    }

    #[test]
    fn a_long_press_is_not_a_tap() {
        let c = Clock(Instant::now());
        let mut t = ShiftTap::new();
        assert_eq!(
            tap(&mut t, &c, 0, 499, ok(), |_| {}),
            1,
            "just under the limit"
        );
        assert_eq!(
            tap(&mut t, &c, 1000, 1500, ok(), |_| {}),
            0,
            "500 ms is no tap"
        );
        assert_eq!(tap(&mut t, &c, 3000, 5000, ok(), |_| {}), 0);
    }

    #[test]
    fn repeat_notifications_do_not_rearm_a_spoiled_press() {
        let c = Clock(Instant::now());
        let mut t = ShiftTap::new();
        t.shift_down(c.at(0), true, false, false, true);
        t.other_key_down();
        t.shift_down(c.at(40), false, false, false, true); // auto repeat
        t.shift_down(c.at(80), false, false, false, true);
        assert!(!t.test_up(c.at(100), &ok()));
        // A repeat does not move the arming time either: 600 ms after the first press it is too late.
        let mut t = ShiftTap::new();
        t.shift_down(c.at(0), true, false, false, true);
        t.shift_down(c.at(550), false, false, false, true);
        assert!(!t.test_up(c.at(600), &ok()));
    }

    #[test]
    fn the_other_shift_and_held_modifiers_spoil_the_tap() {
        let c = Clock(Instant::now());
        let mut t = ShiftTap::new();
        t.shift_down(c.at(0), true, true, false, true); // right Shift already held
        assert!(!t.test_up(c.at(50), &ok()));
        t.shift_down(c.at(100), true, false, true, true); // Ctrl held
        assert!(!t.test_up(c.at(150), &ok()));
        // Released while the other Shift or a modifier is held at the release.
        t.shift_down(c.at(200), true, false, false, true);
        assert!(!t.test_up(
            c.at(250),
            &Release {
                other_shift_held: true,
                ..ok()
            }
        ));
        t.shift_down(c.at(300), true, false, false, true);
        assert!(!t.test_up(
            c.at(350),
            &Release {
                command_held: true,
                ..ok()
            }
        ));
        // Both Shifts: the first release is void, and the second finds nothing armed.
        t.shift_down(c.at(400), true, false, false, true);
        assert!(!t.test_up(
            c.at(450),
            &Release {
                other_shift_held: true,
                ..ok()
            }
        ));
        assert!(
            !t.test_up(c.at(470), &ok()),
            "the other Shift's release after a spoiled press is no tap"
        );
    }

    #[test]
    fn focus_changes_forget_the_press() {
        let c = Clock(Instant::now());
        let mut t = ShiftTap::new();
        t.shift_down(c.at(0), true, false, false, true);
        t.reset(); // focus moved while Shift was held
        assert!(!t.test_up(c.at(100), &ok()));
        // Shift still held across the reset: a repeat notification does not arm either.
        t.shift_down(c.at(150), true, false, false, true);
        t.reset();
        t.shift_down(c.at(200), false, false, false, true);
        assert!(!t.test_up(c.at(250), &ok()));
    }

    #[test]
    fn the_setting_readiness_and_caps_lock_switch_it_off() {
        let c = Clock(Instant::now());
        for r in [
            Release {
                enabled: false,
                ..ok()
            },
            Release {
                ready: false,
                ..ok()
            },
            Release { caps: true, ..ok() },
        ] {
            let mut t = ShiftTap::new();
            assert_eq!(tap(&mut t, &c, 0, 100, r, |_| {}), 0, "{r:?}");
            // A later normal tap still works.
            assert_eq!(tap(&mut t, &c, 1000, 1100, ok(), |_| {}), 1);
        }
    }

    #[test]
    fn test_up_without_key_up_leaves_nothing_behind() {
        let c = Clock(Instant::now());
        let mut t = ShiftTap::new();
        // TestUp consumes the tap even when the host never delivers OnKeyUp.
        t.shift_down(c.at(0), true, false, false, true);
        assert!(t.test_up(c.at(100), &ok()));
        // 3 s later a normal tap: a new press is a new round.
        assert_eq!(tap(&mut t, &c, 3000, 3100, ok(), |_| {}), 1);
        // Same, with the missing key-up and a long pause before the next tap.
        t.shift_down(c.at(4000), true, false, false, true);
        assert!(t.test_up(c.at(4050), &ok()));
        assert_eq!(tap(&mut t, &c, 9000, 9100, ok(), |_| {}), 1);
    }

    #[test]
    fn a_press_that_starts_before_the_context_is_ready_never_becomes_a_tap() {
        let c = Clock(Instant::now());
        let mut t = ShiftTap::new();
        t.shift_down(c.at(0), true, false, false, false); // not synchronised yet
        // It becomes ready while Shift is held: the release is still no tap.
        assert!(!t.test_up(c.at(200), &ok()));
        assert!(!t.key_up(c.at(210), &ok()));
        // The next press, now ready, is a tap again.
        assert_eq!(tap(&mut t, &c, 1000, 1100, ok(), |_| {}), 1);
    }

    #[test]
    fn key_up_checks_current_release_and_is_used_up() {
        let c = Clock(Instant::now());
        let mut t = ShiftTap::new();
        t.shift_down(c.at(0), true, false, false, true);
        t.reset();
        assert!(!t.key_up(c.at(110), &ok()));
        t.shift_down(c.at(200), true, false, false, true);
        assert!(!t.key_up(
            c.at(255),
            &Release {
                ready: false,
                ..ok()
            }
        ));
        assert!(!t.key_up(c.at(260), &ok()), "failed release is consumed");
        t.shift_down(c.at(300), true, false, false, true);
        assert!(t.key_up(c.at(355), &ok()));
        assert!(!t.key_up(c.at(356), &ok()));
    }

    #[test]
    fn a_new_press_after_a_release_while_the_other_shift_is_still_down() {
        let c = Clock(Instant::now());
        let mut t = ShiftTap::new();
        t.shift_down(c.at(0), true, false, false, true); // left down
        t.shift_down(c.at(50), true, true, false, true); // right down while left held: void
        assert!(!t.test_up(
            c.at(100),
            &Release {
                other_shift_held: true,
                ..ok()
            }
        )); // left released, right still held
        assert!(!t.test_up(c.at(150), &ok())); // right released: nothing
        assert_eq!(tap(&mut t, &c, 1000, 1100, ok(), |_| {}), 1, "clean again");
    }
}
