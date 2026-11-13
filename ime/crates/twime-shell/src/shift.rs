//! Shift single tap ("tap Shift to switch Chinese / English"): a pure state machine, the clock is passed in.
//! Same rules as the Windows shell (`twime-tsf/src/shift.rs`), written again here so that the released Windows shell
//! stays untouched. It keeps track of the two Shift keys itself: the modifier bits a framework reports with a Shift
//! event differ between frameworks (the key's own bit is set on the release, sometimes on the press).

use std::time::{Duration, Instant};

use twime_proto::ToggleKey;

/// A press longer than this is not a tap.
pub const TAP_LIMIT: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Nothing pending: a release that finds this state did not start with a clean press we saw.
    Idle,
    /// A clean press of this Shift (`true` = the right one); nothing else happened since.
    Armed(Instant, bool),
    /// This press is spoiled (another key, a modifier, the other Shift ...) until every Shift is released.
    Void,
}

#[derive(Debug)]
pub struct ShiftTap {
    state: State,
    /// Left, right.
    held: [bool; 2],
}

impl ShiftTap {
    pub fn new() -> ShiftTap {
        ShiftTap { state: State::Idle, held: [false; 2] }
    }

    /// A Shift went down. `command`: Ctrl, Alt or Super is held. `ready`: the context has a live session that takes
    /// keys; a press that starts while it is not ready never becomes a tap. A press of a Shift that is already down is
    /// an auto-repeat and changes nothing.
    pub fn down(&mut self, now: Instant, right: bool, command: bool, ready: bool) {
        if self.held[usize::from(right)] {
            return;
        }
        self.held[usize::from(right)] = true;
        let other = self.held[usize::from(!right)];
        self.state = if !other && !command && ready { State::Armed(now, right) } else { State::Void };
    }

    /// Any other key went down: the tap is spoiled.
    pub fn other_key(&mut self) {
        if matches!(self.state, State::Armed(..)) {
            self.state = State::Void;
        }
    }

    /// A Shift was released. True for a clean tap: the caller sends `ToggleMode`. `ready` as in `down`, at this moment.
    pub fn up(&mut self, now: Instant, right: bool, command: bool, caps: bool, ready: bool, toggle: ToggleKey) -> bool {
        self.held[usize::from(right)] = false;
        let other = self.held[usize::from(!right)];
        let enabled = match toggle {
            ToggleKey::Shift => true,
            ToggleKey::LeftShift => !right,
            ToggleKey::RightShift => right,
            ToggleKey::None => false,
        };
        let tap = matches!(self.state, State::Armed(t, r) if r == right && now.saturating_duration_since(t) < TAP_LIMIT)
            && !other && !command && !caps && ready && enabled;
        self.state = if other { State::Void } else { State::Idle };
        tap
    }

    /// Focus changed: forget everything. A Shift still held then is not a tap when it is released.
    pub fn reset(&mut self) {
        *self = ShiftTap::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANY: ToggleKey = ToggleKey::Shift;
    fn ms(t: Instant, n: u64) -> Instant {
        t + Duration::from_millis(n)
    }

    #[test]
    fn clean_tap_toggles_once() {
        let (t, mut s) = (Instant::now(), ShiftTap::new());
        s.down(t, false, false, true);
        assert!(s.up(ms(t, 80), false, false, false, true, ANY));
        assert!(!s.up(ms(t, 90), false, false, false, true, ANY), "a second release of the same press");
    }

    #[test]
    fn another_key_spoils_the_tap() {
        let (t, mut s) = (Instant::now(), ShiftTap::new());
        s.down(t, false, false, true);
        s.other_key();
        assert!(!s.up(ms(t, 80), false, false, false, true, ANY));
        // The next press is a new round.
        s.down(ms(t, 200), false, false, true);
        assert!(s.up(ms(t, 250), false, false, false, true, ANY));
    }

    #[test]
    fn long_press_is_not_a_tap_and_repeats_do_not_rearm() {
        let (t, mut s) = (Instant::now(), ShiftTap::new());
        s.down(t, true, false, true);
        s.down(ms(t, 400), true, false, true); // auto-repeat
        s.down(ms(t, 480), true, false, true);
        assert!(!s.up(ms(t, 500), true, false, false, true, ANY));
    }

    #[test]
    fn both_shifts_never_toggle() {
        let (t, mut s) = (Instant::now(), ShiftTap::new());
        s.down(t, false, false, true);
        s.down(ms(t, 10), true, false, true);
        assert!(!s.up(ms(t, 20), true, false, false, true, ANY));
        assert!(!s.up(ms(t, 30), false, false, false, true, ANY));
        s.down(ms(t, 40), false, false, true);
        assert!(s.up(ms(t, 50), false, false, false, true, ANY), "clean again after both were released");
    }

    #[test]
    fn modifiers_caps_and_readiness() {
        let t = Instant::now();
        // (command at press, ready at press, command at release, caps, ready at release)
        for c in [(true, true, false, false, true), (false, false, false, false, true), (false, true, true, false, true),
            (false, true, false, true, true), (false, true, false, false, false)] {
            let mut s = ShiftTap::new();
            s.down(t, false, c.0, c.1);
            assert!(!s.up(ms(t, 50), false, c.2, c.3, c.4, ANY), "{c:?}");
        }
    }

    #[test]
    fn toggle_key_setting_selects_the_side() {
        let t = Instant::now();
        for (toggle, right, want) in [(ToggleKey::Shift, false, true), (ToggleKey::Shift, true, true),
            (ToggleKey::LeftShift, false, true), (ToggleKey::LeftShift, true, false),
            (ToggleKey::RightShift, false, false), (ToggleKey::RightShift, true, true),
            (ToggleKey::None, false, false), (ToggleKey::None, true, false)] {
            let mut s = ShiftTap::new();
            s.down(t, right, false, true);
            assert_eq!(s.up(ms(t, 50), right, false, false, true, toggle), want, "{toggle:?} right={right}");
        }
    }

    #[test]
    fn reset_forgets_a_held_shift() {
        let (t, mut s) = (Instant::now(), ShiftTap::new());
        s.down(t, false, false, true);
        s.reset();
        assert!(!s.up(ms(t, 50), false, false, false, true, ANY));
    }

    #[test]
    fn release_without_a_press_is_nothing() {
        let mut s = ShiftTap::new();
        assert!(!s.up(Instant::now(), false, false, false, true, ANY));
    }
}
