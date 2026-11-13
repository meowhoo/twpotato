// The input controller: InputMethodKit events -> the shell library (twime_ctx_event) -> the reply applied to the text
// client. One twime_ctx per controller, one twime_shell per process (MAC-M2-plan 3.2). Main thread only.

import AppKit
import InputMethodKit
import CTwime

/// The application's text, as the framework hands it to every call.
typealias TextClient = IMKTextInput

/// Text always goes to the current marked text / selection: no range of the document is ever named.
private let noRange = NSRange(location: NSNotFound, length: 0)

extension IMKTextInput {
    func insert(_ text: String) { insertText(text, replacementRange: noRange) }
    /// `selection`: the caret inside the marked text, in UTF-16 units. An empty `text` removes the marked text.
    func setMarked(_ text: NSAttributedString, selection: NSRange) { setMarkedText(text, selectionRange: selection, replacementRange: noRange) }
    /// The caret on the screen, for the candidate window (a zero rectangle when unknown).
    func caretRect() -> NSRect {
        var rect = NSRect.zero
        attributes(forCharacterIndex: 0, lineHeightRectangle: &rect)
        return rect
    }
}

enum Shared {
    /// The one shell of the process. `nil` only if the library could not make it: then every key passes.
    static let shell: OpaquePointer? = {
        var s: OpaquePointer?
        return twime_shell_open(&s) == TWIME_OK ? s : nil
    }()
    /// The candidate window. The application sets the panel; the tests set a recorder.
    static var candidates: CandidateDisplay?
    /// Tells the user the mode changed (0 Chinese, 1 English), at this caret. The application shows it for a moment;
    /// the tests record it.
    static var showMode: ((UInt32, NSRect) -> Void)?
    /// Takes that away at once (the field lost the focus, or the framework wants our windows gone).
    static var hideMode: (() -> Void)?
    /// The clock of the time budget, in nanoseconds. The tests replace it.
    static var now: () -> UInt64 = { DispatchTime.now().uptimeNanoseconds }
    /// Whether a Shift key is down right now. The tests replace it.
    static var shiftIsDown: () -> Bool = { NSEvent.modifierFlags.contains(.shift) }
    /// Replies that claimed a held key (never, without TWIME_CLIENT_DEFER). The tests check it stays 0.
    static var heldReplies = 0
    /// Event kinds and their order (never what was typed, at most how many characters): for the framework checks of
    /// plan 4.3(a). Off unless the file ~/Library/Caches/TwPotato/trace.on exists when the input method starts; then
    /// the lines go to trace.log next to it (to turn it off: remove trace.on and end the input method's process).
    static let traceFile: FileHandle? = {
        let dir = NSHomeDirectory() + "/Library/Caches/TwPotato"
        guard FileManager.default.fileExists(atPath: dir + "/trace.on") else { return nil }
        FileManager.default.createFile(atPath: dir + "/trace.log", contents: nil, attributes: [.posixPermissions: 0o600])
        return FileHandle(forWritingAtPath: dir + "/trace.log")
    }()
    /// Bytes written so far: at 4 MB the trace stops (it is a tool for one sitting, not a log to keep).
    private static var traced = 0
    static func note(_ what: @autoclosure () -> String) {
        guard let f = traceFile, traced < 4_000_000 else { return }
        let line = Data((String(format: "%.3f ", Date().timeIntervalSince1970) + what() + "\n").utf8)
        traced += line.count
        f.seekToEndOfFile()
        f.write(line)
    }
}

@objc(TwPotatoInputController)
final class TwPotatoInputController: IMKInputController {
    private var ctx: OpaquePointer?
    private var modifiers = ModifierTracker()
    /// The client shows marked text of ours.
    private var marked = false
    /// The last reply of the library (for the tests).
    private(set) var lastReply: ShellReply?
    private var active = false

    override init!(server: IMKServer!, delegate: Any!, client inputClient: Any!) {
        super.init(server: server, delegate: delegate, client: inputClient)
        if let shell = Shared.shell {
            var c: OpaquePointer?
            if twime_ctx_new(shell, &c) == TWIME_OK {
                ctx = c
                // No keys held, compositions committed by us on a focus loss.
                twime_ctx_event(c, UInt32(TWIME_EV_CLIENT), 0, 0, nil)
            }
        }
        Shared.note("init")
    }

    deinit {
        twime_ctx_free(ctx)
        Shared.note("deinit")
    }

    // ---- the library ----

    private func textClient(_ sender: Any?) -> TextClient? {
        sender as? IMKTextInput
    }

    /// Time spent in the library since the framework called us. One call of the framework must return within 50 ms,
    /// and any event may wait the library's 30 ms for an engine that does not answer: so another event is only sent
    /// while less than 15 ms are spent (15 + 30, and 5 to spare). Healthy events take well under a millisecond.
    private var spent = 0.0
    private var waited: Bool { spent >= 0.015 }
    /// A FOCUS_IN that was not sent because the FOCUS_OUT before it had waited; the next key sends it first.
    private var owesFocusIn = false

    /// One event to the library and its reply applied. `nil`: the library failed (the display is cleared).
    @discardableResult
    private func send(_ kind: Int32, _ a: UInt32 = 0, _ b: UInt32 = 0, to client: TextClient?) -> ShellReply? {
        var raw: UnsafePointer<twime_reply>?
        let began = Shared.now()
        let status = ctx == nil ? TWIME_ERR_ARGUMENT : twime_ctx_event(ctx, UInt32(kind), a, b, &raw)
        spent += Double(Shared.now() - began) / 1e9
        guard status == TWIME_OK, let raw else {
            clearDisplay(client)
            return nil
        }
        let reply = ShellReply(raw.pointee)
        if Shared.traceFile != nil {
            Shared.note("  event \(kind): commit \(reply.commit.count) chars, preedit \(reply.preedit.count) chars, flags \(reply.flags), client \(client == nil ? "nil" : "ok")")
        }
        // We never declare TWIME_CLIENT_DEFER: no key is ever held.
        if reply.releaseBefore != 0 || reply.has(TWIME_R_HELD) || reply.has(TWIME_R_RELEASED) || reply.has(TWIME_R_HOLDING) {
            Shared.heldReplies += 1
        }
        lastReply = reply
        apply(reply, to: client)
        return reply
    }

    private func clearDisplay(_ client: TextClient?) {
        if marked {
            client?.setMarked(NSAttributedString(), selection: NSRange(location: 0, length: 0))
            marked = false
        }
        Shared.candidates?.hide()
    }

    /// In the order the header prescribes: commit, then (only with UI_CHANGED) the marked text and the candidates.
    private func apply(_ r: ShellReply, to client: TextClient?) {
        if !r.commit.isEmpty {
            client?.insert(r.commit)
            marked = false // inserting replaces the marked text
        }
        if r.has(TWIME_R_MODE_CHANGED), let client {
            Shared.showMode?(r.mode, client.caretRect())
        }
        guard r.uiChanged else { return }
        if !r.preedit.isEmpty {
            client?.setMarked(markedText(r), selection: NSRange(location: TextUnits.utf16Offset(in: r.preedit, scalars: r.cursor), length: 0))
            marked = true
        } else if marked {
            // Only where marked text of ours is shown: an empty marked text elsewhere could replace a selection.
            client?.setMarked(NSAttributedString(), selection: NSRange(location: 0, length: 0))
            marked = false
        }
        if r.candidatesOpen, let client {
            // The click comes later, from the panel: the client is kept until the panel is hidden or shown again.
            Shared.candidates?.show(items: r.candidates, selected: r.selected, page: r.page, total: r.total, perPage: r.perPage, caret: client.caretRect()) { [weak self] index in
                self?.spent = 0
                self?.send(TWIME_EV_PICK, UInt32(index), to: client)
            }
        } else {
            Shared.candidates?.hide()
        }
    }

    /// The preedit, underlined, one clause per segment.
    private func markedText(_ r: ShellReply) -> NSAttributedString {
        let text = NSMutableAttributedString(string: r.preedit, attributes: [.underlineStyle: NSUnderlineStyle.single.rawValue])
        var scalars = 0
        for (i, seg) in r.segments.enumerated() {
            let from = TextUnits.utf16Offset(in: r.preedit, scalars: scalars)
            scalars += seg.chars
            let to = TextUnits.utf16Offset(in: r.preedit, scalars: scalars)
            text.addAttribute(.markedClauseSegment, value: i, range: NSRange(location: from, length: to - from))
        }
        return text
    }

    /// The framework, or a click outside the composition, ends it: committed once (the library decides what), and the
    /// text typed before is forgotten. Without a composition there is nothing to do.
    private func finishComposition(_ client: TextClient?) {
        guard marked else { return }
        send(TWIME_EV_FOCUS_OUT, to: client)
        if active {
            // An engine that did not answer the first is not asked again in the same call: the next key does it.
            if waited { owesFocusIn = true } else { send(TWIME_EV_FOCUS_IN, to: client) }
        }
        modifiers.focusSent(shiftDown: Shared.shiftIsDown())
    }

    // ---- InputMethodKit ----

    override func recognizedEvents(_ sender: Any!) -> Int {
        // More than key-down: so the framework no longer commits on a click outside the composition; we do (below).
        let mask: NSEvent.EventTypeMask = [.keyDown, .flagsChanged, .leftMouseDown, .rightMouseDown, .otherMouseDown]
        return Int(mask.rawValue)
    }

    override func activateServer(_ sender: Any!) {
        Shared.note("activateServer")
        spent = 0
        owesFocusIn = false
        lastFlags = nil
        active = true
        send(TWIME_EV_FOCUS_IN, to: textClient(sender))
        modifiers.focusSent(shiftDown: Shared.shiftIsDown())
    }

    override func deactivateServer(_ sender: Any!) {
        Shared.note("deactivateServer marked=\(marked)")
        spent = 0
        owesFocusIn = false
        lastFlags = nil
        active = false
        send(TWIME_EV_FOCUS_OUT, to: textClient(sender))
        modifiers.focusSent(shiftDown: Shared.shiftIsDown())
        Shared.candidates?.hide()
        Shared.hideMode?() // or it would stay over a field whose own mode it does not tell
    }

    override func commitComposition(_ sender: Any!) {
        Shared.note("commitComposition marked=\(marked)")
        spent = 0
        lastFlags = nil
        finishComposition(textClient(sender))
    }

    override func hidePalettes() {
        Shared.note("hidePalettes")
        Shared.candidates?.hide()
        Shared.hideMode?()
    }

    override func handle(_ event: NSEvent!, client sender: Any!) -> Bool {
        guard let event else { return false }
        let client = textClient(sender)
        spent = 0
        if Shared.traceFile != nil {
            Shared.note("handle type \(event.type.rawValue)\(event.type == .flagsChanged ? " keyCode \(event.keyCode) flags \(String(event.modifierFlags.rawValue, radix: 16)) stamp \(String(format: "%.4f", event.timestamp))" : "") marked=\(marked)")
        }
        // Only two flags-changed events in a row can be copies of each other.
        if event.type != .flagsChanged { lastFlags = nil }
        switch event.type {
        case .keyDown:
            if owesFocusIn {
                owesFocusIn = false
                send(TWIME_EV_FOCUS_IN, to: client)
                modifiers.focusSent(shiftDown: Shared.shiftIsDown())
                if waited { return false } // still no engine: the key passes, as any key without an engine
            }
            let owed = modifiers.keyDown(shift: event.modifierFlags.contains(.shift))
            let reply = send(TWIME_EV_KEY_DOWN, KeyMap.key(event.keyCode), KeyMap.mods(event.modifierFlags), to: client)
            // Shift releases the library is still owed: after the key (which spoiled any tap), and only if it answered.
            let mayGoOn = reply.map { !$0.noEngine } ?? false
            deliver(mayGoOn ? owed : [], of: owed, mods: KeyMap.mods(event.modifierFlags), to: client)
            return reply?.handled ?? false
        case .flagsChanged:
            flagsChanged(event, client)
            return false
        case .leftMouseDown, .rightMouseDown, .otherMouseDown:
            Shared.note("mouseDown marked=\(marked)")
            finishComposition(client)
            return false
        default:
            return false
        }
    }

    /// The last flags-changed event: key, flags, and the time stamp the event itself carries.
    private var lastFlags: (code: UInt16, flags: UInt, stamp: TimeInterval)?

    private func flagsChanged(_ event: NSEvent, _ client: TextClient?) {
        let f = event.modifierFlags
        let code = event.keyCode
        // Some applications give every change of a modifier key to the input method twice (seen with a browser): a
        // second event for the same key with the same flags, whose own time stamp is 3 to 4 ms after the first one's.
        // It is not the same event again (the stamps differ), so it can only be known by how close the stamps are: the
        // time the key itself changed, not when the calls happen to arrive. Less than 8 ms between two changes of one
        // key into the same state is no keyboard's doing. Such a second event is dropped, or the tracker would take
        // it for the key going the other way.
        if let last = lastFlags, last.code == code, last.flags == f.rawValue, abs(event.timestamp - last.stamp) < 0.008 {
            Shared.note("  (the same flags-changed event again: dropped)")
            return
        }
        lastFlags = (code, f.rawValue, event.timestamp)
        let own = ModifierTracker.control.contains(code) ? f.contains(.control) : ModifierTracker.option.contains(code) ? f.contains(.option) : f.contains(.command)
        let steps = modifiers.flagsChanged(keyCode: code, shift: f.contains(.shift), ownKindDown: own)
        deliver(steps, of: steps, mods: KeyMap.mods(f), to: client)
    }

    /// Sends `steps` (all of `all`, or none of it) until one finds no engine, and tells the tracker how far it got.
    private func deliver(_ steps: [ModifierTracker.Step], of all: [ModifierTracker.Step], mods: UInt32, to client: TextClient?) {
        var sent = 0
        for step in steps {
            let reply: ShellReply?
            switch step {
            case .shiftDown(let right): reply = send(TWIME_EV_KEY_DOWN, UInt32(right ? TWIME_KEY_SHIFT_RIGHT : TWIME_KEY_SHIFT_LEFT), mods, to: client)
            case .shiftUp(let right): reply = send(TWIME_EV_KEY_UP, UInt32(right ? TWIME_KEY_SHIFT_RIGHT : TWIME_KEY_SHIFT_LEFT), mods, to: client)
            case .modifierDown: reply = send(TWIME_EV_KEY_DOWN, UInt32(TWIME_KEY_MODIFIER), mods, to: client)
            case .modifierUp: reply = send(TWIME_EV_KEY_UP, UInt32(TWIME_KEY_MODIFIER), mods, to: client)
            }
            guard let reply else { break }
            sent += 1
            // At most one wait for an engine that does not answer in one event of the framework.
            if reply.noEngine || waited { break }
        }
        modifiers.delivered(sent, of: all)
    }
}
