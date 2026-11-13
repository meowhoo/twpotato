// The tests of the macOS shell (MAC-M2-plan 4.1). Run by test.sh, one group per run:
//   twpotato-tests unit | modifiers | functional | faults | noassets | cases CASES.tsv REF.out.tsv REF.steps RESULT_PREFIX

import AppKit
import InputMethodKit
import CTwime

let recorder = Recorder()
Shared.candidates = recorder
// The real keyboard must not decide a test: nobody holds Shift unless a test says so.
var shiftHeld = false
Shared.shiftIsDown = { shiftHeld }

// ------------------------------------------------------------------------------------------------------------------
// unit: pure functions

func unitTests() throws {
    // Characters (Unicode scalar values) -> UTF-16 units.
    try equal(TextUnits.utf16Offset(in: "你好", scalars: 1), 1, "BMP")
    try equal(TextUnits.utf16Offset(in: "a😀b", scalars: 2), 3, "after an emoji outside the BMP")
    try equal(TextUnits.utf16Offset(in: "a😀b", scalars: 3), 4, "end")
    try equal(TextUnits.utf16Offset(in: "e\u{301}x", scalars: 2), 2, "after a combining mark")
    try equal(TextUnits.utf16Offset(in: "🇹🇼好", scalars: 2), 4, "after a flag (two scalars, four units)")
    try equal(TextUnits.utf16Offset(in: "👨‍👩‍👧", scalars: 5), 8, "after a joined family (5 scalars)")
    try equal(TextUnits.utf16Offset(in: "好", scalars: 9), 1, "clamped to the end")
    try equal(TextUnits.utf16Offset(in: "", scalars: 0), 0, "empty")

    // The panel: under the caret; above when there is no room; kept inside the screen; a useless rectangle -> nil.
    let screen = NSRect(x: 0, y: 0, width: 1000, height: 800)
    let size = NSSize(width: 200, height: 250)
    func place(_ caret: NSRect, _ screens: [NSRect] = [screen]) -> NSPoint? { PanelPlacement.origin(caret: caret, panel: size, screens: screens) }
    try equal(place(NSRect(x: 300, y: 500, width: 1, height: 18)), NSPoint(x: 300, y: 500 - 4 - 250), "below the caret")
    try equal(place(NSRect(x: 300, y: 100, width: 1, height: 18)), NSPoint(x: 300, y: 118 + 4), "above when there is no room below")
    try equal(place(NSRect(x: 950, y: 500, width: 1, height: 18))?.x, 800, "kept inside on the right")
    try equal(place(NSRect(x: 0, y: 780, width: 1, height: 18))?.y, 780 - 4 - 250, "caret at the top")
    try equal(place(NSRect(x: 5, y: 5, width: 1, height: 900))?.y, 550, "kept on the screen even when neither side has room")
    try equal(place(.zero), nil, "a rectangle with no size")
    try equal(place(NSRect(x: 5000, y: 5000, width: 1, height: 18)), nil, "a caret on no screen")
    let second = NSRect(x: 1000, y: 0, width: 800, height: 600)
    try equal(place(NSRect(x: 1700, y: 300, width: 1, height: 18), [screen, second]), NSPoint(x: 1600, y: 300 - 4 - 250), "on the second screen")

    // Keys by position.
    try equal(KeyMap.key(0), UInt32(Character("a").asciiValue!), "A")
    try equal(KeyMap.key(29), UInt32(Character("0").asciiValue!), "0")
    try equal(KeyMap.key(49), UInt32(TWIME_KEY_SPACE), "Space")
    try equal(KeyMap.key(76), UInt32(TWIME_KEY_ENTER), "number pad Enter")
    try equal(KeyMap.key(82), UInt32(TWIME_KEY_NUMPAD_0), "number pad 0")
    try equal(KeyMap.key(92), UInt32(TWIME_KEY_NUMPAD_0) + 9, "number pad 9")
    try equal(KeyMap.key(Keys.other), UInt32(TWIME_KEY_OTHER), "F13")
    try equal(Set(KeyMap.table.values).count, KeyMap.table.count - 1, "one code per key, but for the two Enter keys")
    try equal(KeyMap.mods([.shift, .command, .capsLock]), UInt32(TWIME_MOD_SHIFT | TWIME_MOD_SUPER | TWIME_MOD_CAPS), "modifier bits")
    try equal(KeyMap.mods([.control, .option]), UInt32(TWIME_MOD_CTRL | TWIME_MOD_ALT), "modifier bits")

    // The modifier tracker, step by step.
    typealias S = ModifierTracker.Step
    let L = ModifierTracker.leftShift, R = ModifierTracker.rightShift
    var m = ModifierTracker()
    func step(_ code: UInt16, shift: Bool, own: Bool = false, sent: Int? = nil) -> [S] {
        let steps = m.flagsChanged(keyCode: code, shift: shift, ownKindDown: own)
        m.delivered(sent ?? steps.count, of: steps)
        return steps
    }
    m.focusSent(shiftDown: false)
    try equal(step(L, shift: true), [.shiftDown(right: false)], "a clean press")
    try equal(step(L, shift: false), [.shiftUp(right: false)], "its release")
    // Both Shift keys, released in the order they were pressed: the flag stays set until the last one.
    try equal(step(L, shift: true) + step(R, shift: true) + step(L, shift: true) + step(R, shift: false),
              [.shiftDown(right: false), .shiftDown(right: true), .shiftUp(right: false), .shiftUp(right: true)], "overlap")
    // The press of the right Shift was lost: releasing the left one leaves the flag set. Spoil first, then release.
    try equal(step(L, shift: true), [.shiftDown(right: false)], "press")
    try equal(step(L, shift: true), [.modifierDown, .shiftUp(right: false)], "contradiction: spoil, then release what the library holds")
    try check(m.untrusted, "untrusted after a contradiction")
    try equal(step(R, shift: false), [.modifierDown], "untrusted: no Shift event")
    try check(!m.untrusted, "trusted again once no Shift is down")
    // After a focus event with a Shift down: nothing is believed until all are up.
    m.focusSent(shiftDown: true)
    try equal(step(R, shift: true), [.modifierDown], "pressed while untrusted")
    try equal(step(R, shift: true), [.modifierDown], "one released, one still down")
    try equal(step(L, shift: false), [.modifierDown], "the last one released")
    try check(!m.untrusted, "trusted")
    // An ordinary key that shows a Shift we never saw: untrusted, and nothing extra to send.
    func key(shift: Bool, sent: Int? = nil) -> [S] {
        let steps = m.keyDown(shift: shift)
        m.delivered(sent ?? steps.count, of: steps)
        return steps
    }
    try equal(key(shift: true), [], "a key with a Shift we never saw: nothing to send")
    try check(m.untrusted, "a key with an unknown Shift")
    try equal(key(shift: false), [], "a key without Shift")
    try check(!m.untrusted, "trusted after a key without Shift")
    // The release of a Shift was lost: the next key without Shift pays it, after the key itself.
    try equal(step(L, shift: true), [.shiftDown(right: false)], "press")
    try equal(key(shift: false), [.shiftUp(right: false)], "the key shows the Shift is up: its release is sent after the key")
    try check(!m.untrusted && !m.left, "trusted, nothing owed")
    try equal(step(L, shift: true) + step(L, shift: false), [.shiftDown(right: false), .shiftUp(right: false)], "the very next tap is a clean one")
    // A release that could not be sent (no engine) is still owed, and paid at the next Shift event.
    try equal(step(L, shift: true), [.shiftDown(right: false)], "press")
    try equal(step(L, shift: true, sent: 1), [.modifierDown, .shiftUp(right: false)], "only the first step went out")
    try check(m.untrusted && m.left, "the release is owed")
    try equal(key(shift: false, sent: 0), [.shiftUp(right: false)], "a key pays it, but the engine did not answer")
    try check(m.untrusted && m.left, "still owed")
    try equal(step(R, shift: false), [.modifierDown, .shiftUp(right: false)], "paid at a Shift event")
    try check(!m.untrusted && !m.left, "trusted")
    // A release is owed after a failure (the key that would have paid it found no engine): the next event without
    // Shift pays it, whatever it is, so that the tap after it is a clean one.
    try equal(step(L, shift: true), [.shiftDown(right: false)], "press; its release is lost")
    try equal(key(shift: false, sent: 0), [.shiftUp(right: false)], "the key finds no engine")
    try equal(step(59, shift: false, own: false), [.modifierDown, .shiftUp(right: false), .modifierDown, .modifierUp], "a Control event without Shift pays it")
    try check(!m.untrusted && !m.left, "trusted, nothing owed")
    try equal(step(L, shift: true) + step(L, shift: false), [.shiftDown(right: false), .shiftUp(right: false)], "the very next tap is a clean one")
    // Other modifiers.
    try equal(step(59, shift: false, own: true), [.modifierDown], "Control down")
    try equal(step(59, shift: false, own: false), [.modifierDown, .modifierUp], "Control up: a press first, so that it spoils a tap")
    try equal(step(Keys.capsLock, shift: false), [.modifierDown, .modifierUp], "Caps Lock")
    try equal(step(200, shift: false), [.modifierDown, .modifierUp], "an unknown key")
    try equal(step(L, shift: true) + step(59, shift: false, own: true), [.shiftDown(right: false), .modifierDown, .shiftUp(right: false), .modifierDown], "Control shows that Shift went up unseen")
}

// ------------------------------------------------------------------------------------------------------------------
// modifiers: the tracker against the real library, random sequences (plan 4.1, assertion 4)

/// A small generator with a seed, so a failure can be run again.
struct Random {
    var state: UInt64
    mutating func next() -> UInt64 {
        state = state &* 6364136223846793005 &+ 1442695040888963407
        return state >> 33
    }
    mutating func below(_ n: Int) -> Int { Int(next() % UInt64(n)) }
    mutating func chance(_ percent: Int) -> Bool { below(100) < percent }
}

func modifierTests() throws {
    guard let shell = Shared.shell else { throw Failure(description: "no shell") }
    var ctx: OpaquePointer?
    try check(twime_ctx_new(shell, &ctx) == TWIME_OK, "context")
    defer { twime_ctx_free(ctx) }
    twime_ctx_event(ctx, UInt32(TWIME_EV_CLIENT), 0, 0, nil)

    /// One event; the mode after it, and whether the engine answered.
    func send(_ kind: Int32, _ key: Int32 = 0, _ mods: UInt32 = 0) throws -> (mode: UInt32, noEngine: Bool) {
        var raw: UnsafePointer<twime_reply>?
        try check(twime_ctx_event(ctx, UInt32(kind), UInt32(key), mods, &raw) == TWIME_OK, "event")
        return (raw!.pointee.mode, raw!.pointee.flags & UInt32(TWIME_R_NO_ENGINE) != 0)
    }
    _ = try send(TWIME_EV_FOCUS_IN)
    let t = Date()
    while try send(TWIME_EV_KEY_DOWN, TWIME_KEY_OTHER).noEngine {
        try check(Date().timeIntervalSince(t) < 20, "no engine")
        usleep(20_000)
    }

    var totalToggles = 0, totalEvents = 0, recoveries = 0
    for seed in 1...400 {
        var rng = Random(state: UInt64(seed) &* 0x9E3779B97F4A7C15)
        var tracker = ModifierTracker()
        // The keyboard as it really is.
        var down = [false, false] // left, right Shift
        var control = false
        // A press that may still become a tap: no other Shift, key, modifier or focus event since, and it was delivered.
        var clean = [false, false]
        var mode = try send(TWIME_EV_FOCUS_IN).mode
        var log: [String] = []
        tracker.focusSent(shiftDown: false)
        func spoil() { clean = [false, false] }
        func mods() -> UInt32 { UInt32((down[0] || down[1] ? TWIME_MOD_SHIFT : 0) | (control ? TWIME_MOD_CTRL : 0)) }

        /// Sends the steps as the controller does. `giveUpAfter`: pretend the engine stopped answering there.
        func deliver(_ steps: [ModifierTracker.Step], giveUpAfter: Int?, what: String, mayToggle: Bool) throws {
            var sent = 0
            for step in steps {
                let r: (mode: UInt32, noEngine: Bool)
                switch step {
                case .shiftDown(let right): r = try send(TWIME_EV_KEY_DOWN, right ? TWIME_KEY_SHIFT_RIGHT : TWIME_KEY_SHIFT_LEFT, mods())
                case .shiftUp(let right): r = try send(TWIME_EV_KEY_UP, right ? TWIME_KEY_SHIFT_RIGHT : TWIME_KEY_SHIFT_LEFT, mods())
                case .modifierDown: r = try send(TWIME_EV_KEY_DOWN, TWIME_KEY_MODIFIER, mods())
                case .modifierUp: r = try send(TWIME_EV_KEY_UP, TWIME_KEY_MODIFIER, mods())
                }
                sent += 1
                totalEvents += 1
                if r.mode != mode {
                    totalToggles += 1
                    try check(mayToggle, "seed \(seed): the mode switched at \(what) (step \(step)), which was not a clean tap; before it: \(log.suffix(14).joined(separator: "; "))")
                    mode = r.mode
                }
                if sent == giveUpAfter { break }
            }
            tracker.delivered(sent, of: steps)
        }

        for _ in 0..<60 {
            let lost = rng.chance(12) // this event never reaches the input method
            let giveUp: Int? = rng.chance(8) ? 1 : nil
            let what = rng.below(10)
            log.append("op\(what)\(lost ? " LOST" : "")\(giveUp != nil ? " giveup" : "") [L\(down[0] ? 1 : 0) R\(down[1] ? 1 : 0) C\(control ? 1 : 0) | t:L\(tracker.left ? 1 : 0) R\(tracker.right ? 1 : 0) \(tracker.untrusted ? "untrusted" : "trusted")]")
            switch what {
            case 0, 1, 2, 3, 4, 5: // a Shift key changes
                let side = rng.below(2)
                down[side].toggle()
                let pressed = down[side]
                var tap = false
                // `clean` is what anybody who sees only the delivered events could know: a press or a release that
                // went unseen changes nothing (a release and a new press of the same key, both unseen, cannot be told
                // from the key staying down). An unseen press of the other Shift matters while it is still down.
                if !lost {
                    if pressed {
                        clean[1 - side] = false
                        clean[side] = !down[1 - side] && !control
                    } else {
                        tap = clean[side] && !down[1 - side] && !control
                        clean[side] = false
                    }
                }
                if lost { continue }
                let steps = tracker.flagsChanged(keyCode: side == 1 ? ModifierTracker.rightShift : ModifierTracker.leftShift, shift: down[0] || down[1], ownKindDown: false)
                try deliver(steps, giveUpAfter: giveUp, what: "\(side == 1 ? "right" : "left") Shift \(pressed ? "down" : "up")", mayToggle: tap)
            case 6: // Control
                control.toggle()
                if lost { continue }
                spoil()
                let steps = tracker.flagsChanged(keyCode: 59, shift: down[0] || down[1], ownKindDown: control)
                try deliver(steps, giveUpAfter: giveUp, what: "Control", mayToggle: false)
            case 7, 8: // an ordinary key
                if lost { continue }
                spoil()
                let owed = tracker.keyDown(shift: down[0] || down[1])
                let r = try send(TWIME_EV_KEY_DOWN, TWIME_KEY_OTHER, mods())
                try check(r.mode == mode, "seed \(seed): the mode switched at an ordinary key")
                if giveUp != nil { tracker.delivered(0, of: owed) } else { try deliver(owed, giveUpAfter: nil, what: "releases owed, after a key", mayToggle: false) }
            default: // the composition is ended by the framework, or the focus comes back: both reset the library
                spoil()
                _ = try send(TWIME_EV_FOCUS_OUT)
                let r = try send(TWIME_EV_FOCUS_IN)
                try check(r.mode == mode, "seed \(seed): the mode switched at a focus event")
                tracker.focusSent(shiftDown: down[0] || down[1])
            }
        }

        for side in 0..<2 where down[side] {
            down[side] = false
            // This release may itself complete a clean tap begun above.
            let tap = clean[side] && !down[1 - side] && !control
            clean[side] = false
            let steps = tracker.flagsChanged(keyCode: side == 1 ? ModifierTracker.rightShift : ModifierTracker.leftShift, shift: down[0] || down[1], ownKindDown: false)
            try deliver(steps, giveUpAfter: nil, what: "releasing everything", mayToggle: tap)
        }
        if control {
            control = false
            try deliver(tracker.flagsChanged(keyCode: 59, shift: false, ownKindDown: false), giveUpAfter: nil, what: "releasing Control", mayToggle: false)
        }
        // Once every key is up and one event without Shift arrived (a key, or another modifier), the very next clean
        // tap must switch the mode.
        if seed % 3 == 0 {
            try deliver(tracker.flagsChanged(keyCode: 59, shift: false, ownKindDown: true), giveUpAfter: nil, what: "Control down", mayToggle: false)
            try deliver(tracker.flagsChanged(keyCode: 59, shift: false, ownKindDown: false), giveUpAfter: nil, what: "Control up", mayToggle: false)
        } else {
            let owed = tracker.keyDown(shift: false)
            _ = try send(TWIME_EV_KEY_DOWN, TWIME_KEY_OTHER, 0)
            try deliver(owed, giveUpAfter: nil, what: "releases owed, after a key", mayToggle: false)
        }
        func tap(_ side: Int, mayToggle: Bool) throws {
            let code = side == 1 ? ModifierTracker.rightShift : ModifierTracker.leftShift
            down[side] = true
            try deliver(tracker.flagsChanged(keyCode: code, shift: true, ownKindDown: false), giveUpAfter: nil, what: "tap down", mayToggle: false)
            down[side] = false
            try deliver(tracker.flagsChanged(keyCode: code, shift: false, ownKindDown: false), giveUpAfter: nil, what: "tap up", mayToggle: mayToggle)
        }
        try check(!tracker.untrusted && !tracker.left && !tracker.right, "seed \(seed): still untrusted with every key up and a key typed")
        let before = mode
        try tap(seed % 2, mayToggle: true)
        try check(mode != before, "seed \(seed): the first clean tap did not switch the mode")
        recoveries += 1
    }
    // What this shows: judged by the events that were delivered (nobody can tell a release and a new press that both
    // went unseen from a key held down), the mode only ever switched at a clean tap; and after any sequence the first
    // clean tap switched it. "No engine" is only imitated here, by not sending the rest of the steps: the real thing,
    // and the time each event may take, are in the faults group.
    print("modifiers: 400 random sequences, \(totalEvents) events to the library; the mode switched \(totalToggles) times, each a clean tap as far as the delivered events can tell; after every sequence the first clean tap switched it (\(recoveries) of 400)")
    try check(totalToggles > 450, "too few switches for the test to mean anything: \(totalToggles)")
}

// ------------------------------------------------------------------------------------------------------------------
// functional: one engine, through the controller

func functionalTests() throws {
    shiftHeld = false
    let a = try Field()
    a.activate()
    let cold = try Engine.wait(a)
    print(String(format: "functional: input possible %.2f s after the first key", cold))

    // A sentence: marked text at every key, committed once by Enter. The field has text selected when the composition
    // starts: the input method names no range of the document, so the client replaces the selection itself.
    a.client.documentSelection = NSRange(location: 3, length: 4)
    try a.type("su3")
    try equal(a.client.marked, "你", "marked text")
    try equal(a.client.selection, NSRange(location: 1, length: 0), "caret after the character")
    try a.type("cl3")
    try equal(a.client.marked, "你好", "marked text")
    try equal(a.client.selection, NSRange(location: 2, length: 0), "caret")
    try check(a.key(Keys.enter), "Enter was not handled")
    try equal(a.client.committed, "你好", "committed")
    try equal(a.client.inserts, 1, "inserted once")
    try equal(a.client.marked, "", "no marked text left")
    try equal(a.client.namedRanges, 0, "replacement ranges that named a place in the document")

    // Without a composition, modifier keys and keys that pass never touch the client's marked text
    // (an empty marked text would replace a selection).
    let calls = a.client.setMarkedCalls
    for e in [flags(Keys.control, .control), flags(Keys.control, []), flags(Keys.capsLock, .capsLock), flags(Keys.capsLock, []), flags(Keys.leftShift, .shift), keyDown(Keys.other, .shift), flags(Keys.leftShift, []), keyDown(Keys.esc), keyDown(Keys.other, .command)] {
        try check(!a.handle(e).handled, "a key without a composition was handled")
    }
    try equal(a.client.setMarkedCalls, calls, "setMarkedText calls without a composition")
    try equal(a.client.inserts, 1, "nothing inserted")

    // Candidates: the page of the reply, the highlight, a digit key, the next page, a click, hidePalettes.
    try a.type("su3")
    try check(recorder.shown == nil, "the list is closed until it is asked for")
    try check(a.key(Keys.down), "Down was not handled")
    guard let first = recorder.shown else { throw Failure(description: "the candidate list did not open") }
    try check(first.items.count >= 2 && first.items[0] == "你" && first.selected == 0 && first.page == 0, "first page: \(first)")
    try check(first.total >= first.items.count && first.perPage >= first.items.count, "counts: \(first)")
    try equal(recorder.caret, a.client.caret, "the panel is placed by the client's caret")
    if first.total > first.perPage {
        try check(a.key(Keys.pageDown), "Page Down was not handled")
        try check(recorder.shown?.page == 1 && recorder.shown?.items != first.items, "second page: \(String(describing: recorder.shown))")
        try check(a.key(116), "Page Up was not handled")
        try equal(recorder.shown, first, "back on the first page")
    }
    // hidePalettes: the panel goes, nothing else happens.
    let sessions = Engine.status()
    a.controller!.hidePalettes()
    try check(recorder.shown == nil, "hidePalettes did not hide the candidates")
    try check(a.client.marked == "你" && a.client.inserts == 1 && Engine.status()?.sessions == sessions?.sessions, "hidePalettes changed something else")
    try check(a.key(Keys.esc), "Esc"); try check(a.key(Keys.down), "Down")
    try check(a.key(Keys.ofCharacter["2"]!), "the digit key was not handled")
    try equal(a.client.marked, first.items[1], "the second candidate, by its digit")
    if recorder.shown == nil { try check(a.key(Keys.down), "Down") }
    guard let page = recorder.shown else { throw Failure(description: "the candidate list did not open again") }
    let other = page.items[0] == a.client.marked ? 1 : 0
    recorder.click(other)
    try equal(a.client.marked, page.items[other], "another candidate, by a click")
    try check(a.key(Keys.enter), "Enter")
    try equal(a.client.committed, "你好" + page.items[other], "committed")

    // UTF-16: an emoji outside the BMP in the marked text, taken from the candidates.
    var emoji: String?
    search: for syllable in ["vul4", "vup ", "al ", "e.3", "94", "tj ", "x94", "g4", "2j6", "c8 "] {
        try a.type(syllable)
        try check(a.key(Keys.down), "Down")
        for _ in 0..<12 {
            guard let page = recorder.shown else { break }
            if let i = page.items.firstIndex(where: { $0.unicodeScalars.contains { $0.value > 0xFFFF } }) {
                emoji = page.items[i]
                recorder.click(i)
                break search
            }
            if (page.page + 1) * page.perPage >= page.total { break }
            a.key(Keys.pageDown)
        }
        a.key(Keys.esc); a.key(Keys.esc)
        try equal(a.client.marked, "", "Esc clears the composition")
    }
    guard let emoji else { throw Failure(description: "no emoji among the candidates tried: the UTF-16 check has nothing to check") }
    try equal(a.client.marked, emoji, "the emoji is the marked text")
    try check(a.client.selection.location == (emoji as NSString).length && (emoji as NSString).length > emoji.unicodeScalars.count,
              "caret after the emoji in UTF-16 units: \(a.client.selection) for \(emoji.unicodeScalars.count) scalars")
    try a.type("su3")
    try equal(a.client.marked, emoji + "你", "a character after the emoji")
    try equal(a.client.selection.location, (emoji as NSString).length + 1, "caret after both")
    try check(a.key(Keys.enter), "Enter")
    print("functional: emoji \(emoji) (\(emoji.unicodeScalars.count) scalars, \((emoji as NSString).length) UTF-16 units)")

    // Shift alone switches the mode; every Shift event passes.
    try equal(a.mode, 0, "Chinese")
    var shownModes: [UInt32] = [], shownAt = NSRect.zero
    Shared.showMode = { mode, caret in shownModes.append(mode); shownAt = caret }
    try a.tapShift()
    try equal(a.mode, 1, "English after a tap")
    try check(shownModes == [1] && shownAt == a.client.caret, "the user is told the mode is English now, at the caret: \(shownModes)")
    try check(!a.key(Keys.ofCharacter["s"]!), "a letter is not taken in English mode")
    try a.tapShift(right: true)
    try equal(a.mode, 0, "Chinese after a tap of the right Shift")
    try equal(shownModes, [1, 0], "and that it is Chinese again")
    try a.type("su3"); try check(a.key(Keys.esc), "Esc")
    try equal(shownModes, [1, 0], "typing does not show the mode")
    // The mode shown goes away with the focus and when the framework wants our windows gone.
    var hidden = 0
    Shared.hideMode = { hidden += 1 }
    a.controller!.hidePalettes()
    a.deactivate(); a.activate()
    try equal(hidden, 2, "the mode display is taken away by hidePalettes and by a focus loss")
    // A Shift whose release was lost, a key that shows it is up, and the same Shift pressed again at once: this press is
    // a real one, not a copy of the last flags-changed event, and the tap counts.
    a.handle(flags(Keys.leftShift, .shift))
    a.key(Keys.other)
    try a.tapShift()
    try equal(a.mode, 1, "the first clean tap after a lost release switches the mode")
    try a.tapShift()
    try equal(a.mode, 0, "and back")
    func noSwitch(_ what: String, _ events: [NSEvent]) throws {
        for e in events { try check(!a.handle(e).handled || e.type == .keyDown, "\(what): a modifier event was handled") }
        try equal(a.mode, 0, what)
        try a.tapShift(); try equal(a.mode, 1, "\(what): the next clean tap switches")
        try a.tapShift(); try equal(a.mode, 0, "\(what): and back")
    }
    let L = Keys.leftShift, R = Keys.rightShift
    // An application that hands every flags-changed event over twice (a browser does): the tap still counts once.
    // (As recorded from a browser: the second event of each pair carries a time stamp about 3.5 ms after the first.)
    func twice(_ code: UInt16, _ f: NSEvent.ModifierFlags) throws {
        let t = nextStamp()
        try check(!a.handle(flags(code, f, stamp: t)).handled && !a.handle(flags(code, f, stamp: t + 0.0035)).handled, "a Shift event was handled")
    }
    try twice(L, .shift); try twice(L, [])
    try equal(a.mode, 1, "a tap whose events all arrive twice switches the mode once")
    try twice(R, .shift); try twice(R, [])
    try equal(a.mode, 0, "and back")
    // Events that look the same but are changes of their own (the press of the right Shift and its release never
    // arrived, so the left one's release, and its next press, carry the same flags as its first press; 50 ms apart
    // by their time stamps, however fast the calls come): nothing is dropped, and the tap that ends this does not count.
    for e in [flags(L, .shift), flags(L, .shift), flags(L, .shift), flags(L, [])] { try check(!a.handle(e).handled, "a Shift event was handled") }
    try equal(a.mode, 0, "look-alike events are not taken for copies: no switch")
    try a.tapShift(); try equal(a.mode, 1, "the next clean tap switches"); try a.tapShift(); try equal(a.mode, 0, "and back")
    try noSwitch("Shift with another key", [flags(L, .shift), keyDown(Keys.other, .shift), flags(L, [])])
    try noSwitch("both Shift keys, first one released first", [flags(L, .shift), flags(R, .shift), flags(L, .shift), flags(R, [])])
    try noSwitch("both Shift keys, second one released first", [flags(L, .shift), flags(R, .shift), flags(R, .shift), flags(L, [])])
    try noSwitch("Caps Lock on during Shift", [flags(L, .shift), flags(Keys.capsLock, [.shift, .capsLock]), flags(L, .capsLock), flags(Keys.capsLock, [])])
    try noSwitch("Control during Shift", [flags(L, .shift), flags(Keys.control, [.shift, .control]), flags(Keys.control, .shift), flags(L, [])])
    try noSwitch("Shift during Control", [flags(Keys.control, .control), flags(L, [.shift, .control]), flags(L, .control), flags(Keys.control, [])])
    try noSwitch("an unknown modifier key during Shift", [flags(L, .shift), flags(200, .shift), flags(L, [])])
    try noSwitch("the press of the other Shift was lost", [flags(L, .shift), flags(L, .shift), flags(R, [])])
    // Shift still down when the field loses the focus, and when it gets it back.
    a.handle(flags(L, .shift)); a.deactivate()
    shiftHeld = true; a.activate(); shiftHeld = false
    try noSwitch("a Shift held across a focus change", [flags(L, [])])
    // A composition ended by the framework while a Shift is down, then the other Shift pressed and released.
    try a.type("su3")
    a.handle(flags(L, .shift)); shiftHeld = true; a.commitComposition(); shiftHeld = false
    try noSwitch("a composition ended while Shift is down", [flags(R, .shift), flags(R, .shift), flags(L, [])])
    try equal(a.client.marked, "", "nothing composing")

    // commitComposition: committed once, nothing left, and nothing more the second time.
    let base = a.client.committed
    try a.type("su3cl3")
    var inserts = a.client.inserts
    a.commitComposition()
    try equal(a.client.committed, base + "你好", "the composition was committed")
    try check(a.client.inserts == inserts + 1 && a.client.marked == "" && recorder.shown == nil, "once, and nothing left")
    a.commitComposition()
    try equal(a.client.inserts, inserts + 1, "a second commitComposition commits nothing")
    try a.type("su3")
    try equal(a.client.marked, "你", "typing goes on after a commitComposition")
    // A mouse button pressed outside the composition: the same, and the click passes.
    inserts = a.client.inserts
    try check(!a.handle(mouseDown()).handled, "the click must pass")
    try check(a.client.committed.hasSuffix("好你") && a.client.inserts == inserts + 1 && a.client.marked == "", "the click commits once: \(a.client.committed)")
    a.commitComposition()
    try check(!a.handle(mouseDown()).handled && a.client.inserts == inserts + 1, "nothing more after the click")

    // The time budget: an event that took 20 ms leaves no room for a second one that might wait 30 ms more. With a
    // clock that makes every event look 20 ms long, ending a composition sends its first event only, and the key
    // after it sends the owed second one and then passes instead of going to the engine.
    try a.type("su3")
    let real = Shared.now
    var skew: UInt64 = 0, readings = 0
    Shared.now = { readings += 1; if readings % 2 == 0 { skew += 20_000_000 }; return real() + skew }
    inserts = a.client.inserts
    try check(!a.handle(mouseDown()).handled && a.client.inserts == inserts + 1 && a.client.marked == "", "the click commits once")
    let passed = a.key(Keys.ofCharacter["s"]!)
    Shared.now = real
    try check(!passed && a.client.marked == "", "the key after a slow event must pass: there is no time left for it")
    try a.type("su3")
    try equal(a.client.marked, "你", "typing goes on")
    try check(a.key(Keys.esc), "Esc")
    try equal(a.client.marked, "", "nothing composing")

    // Two fields in turn: no mixing; a field that loses the focus commits what it composed.
    let b = try Field()
    try a.type("su3")
    a.deactivate()
    try check(a.client.committed.hasSuffix("你") && a.client.marked == "", "deactivate commits the composition")
    b.activate()
    try b.type("cl3")
    try equal(Engine.status()?.sessions, 2, "two sessions")
    try equal(b.client.marked, "好", "the second field composes on its own")
    try check(b.key(Keys.enter), "Enter")
    try equal(b.client.committed, "好", "the second field's text")
    b.deactivate(); a.activate()
    let before = a.client.committed
    try a.type("cl3")
    try check(a.key(Keys.enter), "Enter")
    try equal(a.client.committed, before + "好", "the first field after coming back")
    try equal(b.client.committed, "好", "nothing arrived in the other field")
    // A controller that is released closes its session.
    a.deactivate()
    a.release()
    try equal(Engine.status()?.sessions, 1, "one session after the first controller is gone")
    b.release()
    try equal(Engine.status()?.sessions, 0, "none after the second")
    try equal(Shared.heldReplies, 0, "replies that claimed a held key")
    try check(max(a.slowest, b.slowest) < 0.050, "a handle took \(max(a.slowest, b.slowest)) s")
    try equal(a.client.namedRanges + b.client.namedRanges, 0, "replacement ranges that named a place in the document")

    // A candidate clicked after the call that showed the list has returned, with a client nobody else keeps (a real
    // client is only ever handed to us as the sender of a call): the choice must still reach it.
    let c = try Field()
    c.activate()
    try c.type("su3")
    try check(c.key(Keys.down), "Down")
    guard let list = recorder.shown, list.items.count >= 2 else { throw Failure(description: "the candidate list did not open") }
    weak var client = c.client
    c.dropClient()
    try check(client != nil, "the client must be kept while its candidates are shown")
    // Only to look at it afterwards: the list closes with the choice, and then nothing of ours keeps the client.
    var seen: FakeClient? = client
    recorder.click(1)
    try equal(seen?.marked, list.items[1], "the clicked candidate reached the client")
    try equal(seen?.namedRanges, 0, "replacement ranges that named a place in the document")
    seen = nil
    if recorder.shown != nil { c.controller!.hidePalettes() }
    try check(client == nil, "the client is let go once the candidates are hidden")
    c.release()
    try equal(Engine.status()?.sessions, 0, "no session left")
}

// ------------------------------------------------------------------------------------------------------------------
// faults: no handle may take more than 50 ms, nothing is committed twice

let limit = 0.050

func faultTests() throws {
    shiftHeld = false
    try check(Engine.running().isEmpty, "an engine of this stage runs before the test")
    let f = try Field()
    f.activate()
    // (i) No engine yet: the first key passes; input is possible within 3 s.
    let first = f.handle(keyDown(Keys.ofCharacter["s"]!))
    try check(!first.handled && f.reply?.noEngine == true && f.client.marked == "" && f.client.inserts == 0, "the first key must pass untouched")
    let cold = try Engine.wait(f, limit: 3)
    try f.type("su3")
    try equal(f.client.marked, "你", "typing after the cold start")
    print(String(format: "faults (i): first key passed in %.1f ms; input possible after %.2f s", first.seconds * 1000, cold))

    // (ii) The engine is killed in the middle of a composition.
    let pid = try Engine.pid()
    kill(pid, SIGKILL)
    while Engine.running().contains(where: { $0.pid == pid }) { usleep(5_000) }
    let dead = f.handle(keyDown(Keys.ofCharacter["c"]!))
    try check(!dead.handled && f.reply?.noEngine == true, "the key that finds the engine gone must pass")
    try check(f.client.marked == "" && f.client.inserts == 0, "the composition is removed and nothing is committed")
    let back = try Engine.wait(f, limit: 3)
    let replaced = try Engine.pid()
    try check(replaced != pid, "a new engine")
    try f.type("cl3")
    try equal(f.client.marked, "好", "typing after the engine came back")
    print(String(format: "faults (ii): the key passed in %.1f ms; input possible again after %.2f s", dead.seconds * 1000, back))

    // (iii) The engine stops answering in the middle of a composition: the key waits the time limit, then passes.
    let stopped = try Engine.pid()
    try check(stopped > 0, "process id")
    f.handle(flags(Keys.leftShift, .shift)) // a Shift the library knows is down (used below)
    kill(stopped, SIGSTOP)
    defer { kill(stopped, SIGCONT) }
    let t = Date()
    while !Engine.running().contains(where: { $0.pid == stopped && $0.state.hasPrefix("T") }) {
        try check(Date().timeIntervalSince(t) < 5, "the engine is not seen stopped")
        usleep(2_000)
    }
    let inserts = f.client.inserts, calls = f.client.setMarkedCalls
    let late = f.handle(keyDown(Keys.ofCharacter["c"]!, .shift))
    try check(!late.handled && f.reply?.noEngine == true, "the key must pass when the reply does not come")
    try check(late.seconds >= 0.030 && late.seconds < limit, String(format: "the key returned after %.1f ms: it must wait the 30 ms limit and no longer than 50 ms", late.seconds * 1000))
    try check(f.client.marked == "" && f.client.inserts == inserts, "the composition is removed and nothing is committed")
    // The connection is gone now, and what we believe about Shift is contradicted: still at most one wait per event.
    let contradicted = f.handle(flags(Keys.leftShift, .shift))
    try check(contradicted.seconds < limit, String(format: "a contradicted Shift event took %.1f ms", contradicted.seconds * 1000))
    let unknownShift = f.handle(keyDown(Keys.other, .shift))
    try check(!unknownShift.handled && unknownShift.seconds < limit, String(format: "a key with an unknown Shift took %.1f ms", unknownShift.seconds * 1000))
    let afterTimeout = f.client.setMarkedCalls
    kill(stopped, SIGCONT)
    usleep(200_000)
    RunLoop.current.run(until: Date().addingTimeInterval(0.05))
    try check(f.client.inserts == inserts && f.client.setMarkedCalls == afterTimeout && f.client.marked == "", "a late reply was applied (\(f.client.setMarkedCalls - calls) marked-text calls since the stop)")
    f.handle(flags(Keys.leftShift, [])); f.handle(flags(Keys.rightShift, []))
    try Engine.wait(f, limit: 3)
    try f.type("cl3")
    try equal(f.client.marked, "好", "typing after the engine answers again")
    try check(f.key(Keys.enter), "Enter")
    try equal(f.client.committed, "好", "committed once")
    print(String(format: "faults (iii): the key returned after %.1f ms; contradicted Shift %.1f ms; key with unknown Shift %.1f ms", late.seconds * 1000, contradicted.seconds * 1000, unknownShift.seconds * 1000))

    // The same engine stops again, and the composition is ended by a click: the click may wait once, not twice
    // (ending a composition is two events for the library), and neither may the key after it.
    try f.type("su3")
    kill(stopped, SIGSTOP)
    let t2 = Date()
    while !Engine.running().contains(where: { $0.pid == stopped && $0.state.hasPrefix("T") }) {
        try check(Date().timeIntervalSince(t2) < 5, "the engine is not seen stopped")
        usleep(2_000)
    }
    let click = f.handle(mouseDown())
    try check(!click.handled && click.seconds >= 0.030 && click.seconds < limit, String(format: "the click returned after %.1f ms", click.seconds * 1000))
    try check(f.client.marked == "" && f.client.committed == "好", "the composition is removed and nothing is committed")
    let after = f.handle(keyDown(Keys.ofCharacter["c"]!))
    try check(!after.handled && after.seconds < limit, String(format: "the key after the click returned after %.1f ms", after.seconds * 1000))
    kill(stopped, SIGCONT)
    usleep(200_000)
    try Engine.wait(f, limit: 3)
    try f.type("su3")
    try equal(f.client.marked, "你", "typing after the engine answers again")
    try check(f.key(Keys.enter), "Enter")
    try equal(f.client.committed, "好你", "committed once")
    try equal(f.client.namedRanges, 0, "replacement ranges that named a place in the document")
    print(String(format: "faults (iii, click): the click returned after %.1f ms; the key after it %.1f ms", click.seconds * 1000, after.seconds * 1000))
    try check(f.slowest < limit, String(format: "the slowest handle took %.1f ms", f.slowest * 1000))
    try equal(Shared.heldReplies, 0, "replies that claimed a held key")
    f.deactivate()
    f.release()
}

/// (iv) The assets are missing (test.sh makes this stage without them): every key passes at once, and the engine is
/// started at most every 3 seconds.
func noAssetsTest() throws {
    let f = try Field()
    f.activate()
    let t = Date()
    var seen: [Double] = []
    var keys = 0
    while Date().timeIntervalSince(t) < 7.5 {
        let r = f.handle(keyDown(Keys.ofCharacter["s"]!))
        keys += 1
        try check(!r.handled && f.reply?.noEngine == true && f.client.marked == "" && f.client.inserts == 0, "every key must pass untouched")
        try check(r.seconds < limit, String(format: "a key took %.1f ms", r.seconds * 1000))
        let n = Engine.starts().count
        if n > seen.count { seen.append(Date().timeIntervalSince(t)) }
        usleep(50_000)
    }
    try equal(seen.count, 3, "starts of the engine in 7.5 s (at 0, 3 and 6 s): \(seen)")
    try check(zip(seen, seen.dropFirst()).allSatisfy { $1 - $0 >= 2.9 }, "at least 3 s apart: \(seen)")
    usleep(500_000)
    try check(Engine.running().isEmpty, "no engine is left")
    print(String(format: "faults (iv): %d keys, all passed, slowest %.1f ms; the engine was started at %@ s", keys, f.slowest * 1000, seen.map { String(format: "%.1f", $0) }.joined(separator: ", ")))
    f.release()
}

// ------------------------------------------------------------------------------------------------------------------
// cases: every case typed through the controller, compared reply by reply with what the engine itself answers (xcmp)

let keyOf: [Character: Character] = Dictionary(uniqueKeysWithValues: zip("ㄅㄆㄇㄈㄉㄊㄋㄌㄍㄎㄏㄐㄑㄒㄓㄔㄕㄖㄗㄘㄙㄧㄨㄩㄚㄛㄜㄝㄞㄟㄠㄡㄢㄣㄤㄥㄦ", "1qaz2wsxedcrfv5tgbyhnujm8ik,9ol.0p;/-"))
let toneKey: [Character: Character] = ["ˊ": "6", "ˇ": "3", "ˋ": "4", "˙": "7"]

/// The keys of one syllable on the standard layout: initial, medial, final, then the tone key (first tone: Space).
func keys(ofSyllable s: Substring) throws -> [UInt16] {
    var body: [Character] = [], tone: Character = " "
    for ch in s {
        if let t = toneKey[ch] { tone = t } else if let k = keyOf[ch] { body.append(k) } else { throw Failure(description: "unreadable zhuyin \(s)") }
    }
    return (body + [tone]).map { Keys.ofCharacter[$0]! }
}

/// A string as Rust's {:?} prints it, without the quotes.
func unescape(_ s: Substring) -> String {
    guard s.contains("\\") else { return String(s) }
    var out = "", i = s.startIndex
    while i < s.endIndex {
        let c = s[i]
        i = s.index(after: i)
        guard c == "\\", i < s.endIndex else { out.append(c); continue }
        let e = s[i]
        i = s.index(after: i)
        switch e {
        case "n": out.append("\n")
        case "t": out.append("\t")
        case "r": out.append("\r")
        case "0": out.append("\0")
        case "u":
            let close = s[i...].firstIndex(of: "}")!
            out.unicodeScalars.append(UnicodeScalar(UInt32(s[s.index(after: i)..<close], radix: 16)!)!)
            i = s.index(after: close)
        default: out.append(e)
        }
    }
    return out
}

struct RefStep {
    var kind: Substring
    var handled: Bool
    var composing: Bool
    var commit: String
    /// The composition this reply carries: text and caret in characters. `nil`: the reply carries none.
    var comp: (text: String, cursor: Int)?
}

/// One line of xcmp's XCMP_STEPS output: seq TAB kind TAB handled|composing|list open|commit|composition.
func parseStep(_ line: Substring) throws -> RefStep {
    let f = line.split(separator: "\t", maxSplits: 2, omittingEmptySubsequences: false)
    guard f.count == 3 else { throw Failure(description: "unreadable reply line: \(line)") }
    var rest = f[2]
    func bool() throws -> Bool {
        guard let bar = rest.firstIndex(of: "|") else { throw Failure(description: "unreadable reply line: \(line)") }
        let v = rest[..<bar] == "true"
        rest = rest[rest.index(after: bar)...]
        return v
    }
    /// The quoted string `rest` starts with; `rest` moves past its closing quote.
    func quoted() throws -> String {
        var i = rest.index(after: rest.startIndex)
        while i < rest.endIndex, rest[i] != "\"" {
            i = rest.index(i, offsetBy: rest[i] == "\\" ? 2 : 1)
        }
        guard i < rest.endIndex else { throw Failure(description: "unreadable reply line: \(line)") }
        let s = unescape(rest[rest.index(after: rest.startIndex)..<i])
        rest = rest[rest.index(after: i)...]
        return s
    }
    let (handled, composing, _) = (try bool(), try bool(), try bool())
    var commit = ""
    if rest.hasPrefix("None|") {
        rest = rest.dropFirst(5)
    } else {
        rest = rest.dropFirst(5) // Some(
        commit = try quoted()
        rest = rest.dropFirst(2) // )|
    }
    var comp: (String, Int)?
    if rest != "None" {
        rest = rest.dropFirst(6) // Some((
        let text = try quoted()
        guard rest.hasPrefix(", "), let cursor = Int(rest.dropFirst(2).dropLast(2)) else { throw Failure(description: "unreadable reply line: \(line)") }
        comp = (text, cursor)
    }
    return RefStep(kind: f[1], handled: handled, composing: composing, commit: commit, comp: comp)
}

func percentile(_ sorted: [Double], _ p: Double) -> Double {
    sorted.isEmpty ? 0 : sorted[max(1, min(sorted.count, Int((Double(sorted.count) * p).rounded(.up)))) - 1]
}

func caseTests(cases casesPath: String, refOut: String, refSteps: String, prefix: String) throws {
    shiftHeld = false
    let lines = try String(contentsOfFile: casesPath, encoding: .utf8).split(separator: "\n")
    let ref = try String(contentsOfFile: refOut, encoding: .utf8).split(separator: "\n").map { $0.split(separator: "\t", omittingEmptySubsequences: false) }
    var steps: [[RefStep]] = []
    for line in try String(contentsOfFile: refSteps, encoding: .utf8).split(separator: "\n") {
        let s = try parseStep(line)
        if s.kind == "focus" { steps.append([]) }
        steps[steps.count - 1].append(s)
    }
    try check(!lines.isEmpty && lines.count == ref.count && lines.count == steps.count, "the reference does not cover the cases: \(lines.count) cases, \(ref.count) results, \(steps.count) groups of replies")

    let f = try Field()
    f.activate()
    let cold = try Engine.wait(f)
    let engine = try Engine.pid()
    f.deactivate()

    var convert: [Double] = [], every: [Double] = []
    var bad: [String] = [], badCases = Set<Substring>()
    var compared = 0
    let started = Date()
    for (n, line) in lines.enumerated() {
        let c = line.split(separator: "\t", omittingEmptySubsequences: false)
        let id = c[0]
        try check(ref[n][0] == id, "case \(id): the reference is in another order")
        var typed: [(code: UInt16, convert: Bool)] = []
        for syllable in c[3].split(separator: " ") {
            let k = try keys(ofSyllable: syllable)
            typed += k.enumerated().map { ($1, $0 == k.count - 1) }
        }
        typed += [(Keys.down, false), (Keys.esc, false), (Keys.enter, false)]
        let want = steps[n]
        try check(want.count == typed.count + 1, "case \(id): \(want.count) reference replies for \(typed.count) keys")
        f.activate()
        let committedBefore = f.client.committed
        var state = (text: "", cursor: 0)
        func differ(_ i: Int, _ what: String, _ w: String, _ g: String) {
            badCases.insert(id)
            if bad.count < 200 { bad.append("DIFF \(id) reply \(i) (\(want[i].kind)) \(what): engine \(w) macOS \(g)") }
        }
        for (i, key) in typed.enumerated() {
            let w = want[i + 1]
            if let comp = w.comp { state = comp } else if !w.composing { state = ("", 0) }
            let before = f.client.committed
            let r = f.handle(keyDown(key.code))
            every.append(r.seconds)
            if key.convert { convert.append(r.seconds) }
            compared += 1
            if r.handled != w.handled { differ(i + 1, "handled", "\(w.handled)", "\(r.handled)") }
            let got = String(f.client.committed.dropFirst(before.count))
            if got != w.commit { differ(i + 1, "commit", w.commit, got) }
            let units = TextUnits.utf16Offset(in: state.text, scalars: state.cursor)
            if f.client.marked != state.text || (!state.text.isEmpty && f.client.selection != NSRange(location: units, length: 0)) {
                differ(i + 1, "marked text", "\(state.text) @\(units)", "\(f.client.marked) @\(f.client.selection.location)+\(f.client.selection.length)")
            }
            if w.kind == "list" {
                let shown = (recorder.shown?.items ?? []).joined(separator: "/")
                if shown != ref[n][3] { differ(i + 1, "candidates", String(ref[n][3]), shown) }
                if let s = recorder.shown, s.selected != 0 || s.page != 0 { differ(i + 1, "highlight / page", "0, 0", "\(s.selected), \(s.page)") }
            } else if recorder.shown != nil {
                differ(i + 1, "candidates", "closed", "open")
            }
        }
        let all = String(f.client.committed.dropFirst(committedBefore.count))
        if all != ref[n][4] { differ(typed.count, "all commits", String(ref[n][4]), all) }
        f.deactivate()
        if (n + 1) % 2000 == 0 {
            print("  \(n + 1) cases, \(badCases.count) differ, \(Int(Date().timeIntervalSince(started))) s")
        }
    }
    let took = Date().timeIntervalSince(started)
    let same = (try? Engine.pid()) == engine
    convert.sort(); every.sort()
    func ms(_ v: Double) -> String { String(format: "%.2f", v * 1000) }
    func summary(_ v: [Double]) -> String {
        "n=\(v.count) p50=\(ms(percentile(v, 0.50))) ms p95=\(ms(percentile(v, 0.95))) ms p99=\(ms(percentile(v, 0.99))) ms max=\(ms(v.last ?? 0)) ms over10ms=\(v.filter { $0 > 0.010 }.count)"
    }
    let p99ok = !convert.isEmpty && percentile(convert, 0.99) <= 0.010
    var report = [
        "cases: \(lines.count); replies compared: \(compared); cases with a difference: \(badCases.count); time \(Int(took)) s",
        "engine: the same process for the whole run: \(same)",
        String(format: "cold start: input possible %.2f s after the first key", cold),
        "handle(_:client:), conversion keys (the last key of every syllable): " + summary(convert),
        "handle(_:client:), every key: " + summary(every),
    ] + bad
    report.append("assertions 1, 2 and the candidate page: \(badCases.isEmpty ? "passed" : "FAILED"); latency p99 <= 10 ms: \(p99ok ? "passed" : "FAILED"); replies that claimed a held key: \(Shared.heldReplies)")
    try (report.joined(separator: "\n") + "\n").write(toFile: prefix + ".result.txt", atomically: true, encoding: .utf8)
    print(report.joined(separator: "\n"))
    f.release()
    try check(badCases.isEmpty && p99ok && same && Shared.heldReplies == 0, "the cases did not pass")
}

// ------------------------------------------------------------------------------------------------------------------

let args = CommandLine.arguments
do {
    switch args.dropFirst().first {
    case "unit": try unitTests()
    case "modifiers": try modifierTests()
    case "functional": try functionalTests()
    case "faults": try faultTests()
    case "noassets": try noAssetsTest()
    case "cases" where args.count == 6: try caseTests(cases: args[2], refOut: args[3], refSteps: args[4], prefix: args[5])
    default:
        print("usage: twpotato-tests unit | modifiers | functional | faults | noassets | cases CASES REF.out.tsv REF.steps RESULT_PREFIX")
        exit(2)
    }
    print("PASS \(args[1])")
} catch {
    print("FAIL \(args[1]): \(error)")
    exit(1)
}
