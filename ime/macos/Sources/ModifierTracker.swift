// flagsChanged -> the Shift / modifier key events of the shell library (MAC-M2-plan 3.2.1). Pure state machine.
//
// A flagsChanged event names the key that changed and the modifier flags after the change. The Shift flag alone cannot
// tell which Shift went which way, and events get lost (keys pressed while another application had the focus, keys
// already down when we are activated). The library's Shift-tap logic keeps the two Shift keys itself, so what we send
// must be true: when we cannot know, no Shift event is sent and a pending tap is spoiled first.

struct ModifierTracker {
    enum Step: Equatable {
        case shiftDown(right: Bool)
        case shiftUp(right: Bool)
        /// TWIME_KEY_MODIFIER: spoils a pending tap in the library.
        case modifierDown
        case modifierUp
    }

    /// The Shift keys we have told the library are down (a mirror of what it keeps).
    private(set) var left = false
    private(set) var right = false
    /// We do not know which Shift keys are really down.
    private(set) var untrusted = true
    /// The event being handled showed no Shift at all.
    private var noShiftSeen = false

    static let leftShift: UInt16 = 56
    static let rightShift: UInt16 = 60
    /// Control (left, right), Option (left, right), Command (left, right).
    static let control: Set<UInt16> = [59, 62]
    static let option: Set<UInt16> = [58, 61]
    static let command: Set<UInt16> = [55, 54]

    /// We sent the library a focus event: it forgot its Shift state, and so do we. `shiftDown`: a Shift key is down at
    /// this moment (asked from the system); then we cannot say which, and trust returns with the next event that shows
    /// no Shift.
    mutating func focusSent(shiftDown: Bool) {
        (left, right, untrusted) = (false, false, shiftDown)
    }

    /// Spoil a pending tap, then release what the library still holds: in this order.
    private mutating func distrust() -> [Step] {
        untrusted = true
        var steps: [Step] = [.modifierDown]
        if left { steps.append(.shiftUp(right: false)) }
        if right { steps.append(.shiftUp(right: true)) }
        (left, right) = (false, false)
        return steps
    }

    /// A flagsChanged event. `down`: whether the flag of that key's own kind is set now (Control for a Control key ...).
    /// Returns what to send, in order; then call `delivered` with how many were sent.
    mutating func flagsChanged(keyCode: UInt16, shift: Bool, ownKindDown: Bool) -> [Step] {
        noShiftSeen = !shift
        if keyCode == Self.leftShift || keyCode == Self.rightShift {
            if untrusted {
                return distrust()
            }
            let isRight = keyCode == Self.rightShift
            let (l, r) = isRight ? (left, !right) : (!left, right)
            guard (l || r) == shift else {
                return distrust()
            }
            (left, right) = (l, r)
            let nowDown = isRight ? r : l
            return [nowDown ? .shiftDown(right: isRight) : .shiftUp(right: isRight)]
        }
        // Another modifier. What we believe about Shift must still agree with the flag; and when the flag shows no
        // Shift at all, releases the library is still owed are paid here too (after the spoiling press, as always).
        var steps: [Step] = []
        if (left || right) != shift && (!untrusted || !shift) {
            steps = distrust()
        }
        // Always a press first: only a press spoils a pending tap in the library, and a modifier released between the
        // press and the release of a Shift (its own press unseen) must spoil it too. Caps Lock and keys we do not
        // know count as pressed and released.
        let held = (Self.control.contains(keyCode) || Self.option.contains(keyCode) || Self.command.contains(keyCode)) && ownKindDown
        steps += held ? [.modifierDown] : [.modifierDown, .modifierUp]
        return steps
    }

    /// An ordinary key went down with these flags. Nothing is sent before the key itself: the key spoils a tap anyway.
    /// Returns what to send after the key: with no Shift down, the releases the library is still owed (the key has
    /// spoiled any tap, so they cannot switch the mode). Then call `delivered`, also for an empty list.
    mutating func keyDown(shift: Bool) -> [Step] {
        if !untrusted && (left || right) != shift {
            untrusted = true
        }
        noShiftSeen = !shift
        var steps: [Step] = []
        if !shift {
            if left { steps.append(.shiftUp(right: false)) }
            if right { steps.append(.shiftUp(right: true)) }
            (left, right) = (false, false)
        }
        return steps
    }

    /// Of `steps`, only the first `count` reached the library (the rest were given up: no engine, see 3.2.1 rule 7).
    /// A Shift release that did not go out is still owed.
    mutating func delivered(_ count: Int, of steps: [Step]) {
        for step in steps.dropFirst(count) {
            switch step {
            case .shiftUp(let isRight):
                if isRight { right = true } else { left = true }
                untrusted = true
            case .shiftDown(let isRight):
                // Never happens (a Shift press is always the first and only step); kept consistent all the same.
                if isRight { right = false } else { left = false }
                untrusted = true
            case .modifierDown, .modifierUp:
                break
            }
        }
        if noShiftSeen && !left && !right {
            untrusted = false
        }
        noShiftSeen = false
    }
}
