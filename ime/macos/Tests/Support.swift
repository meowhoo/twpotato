// What the tests share: the recording text client and candidate window, synthetic events, the engine of the stage.
// The test program runs from <stage>/T.app/Contents/MacOS with HOME=<stage>/home (test.sh), so the shell library
// finds the engine and its socket by the product's own rules.

import AppKit
import InputMethodKit
import CTwime

struct Failure: Error, CustomStringConvertible {
    let description: String
}

func check(_ ok: @autoclosure () -> Bool, _ what: @autoclosure () -> String, file: StaticString = #fileID, line: UInt = #line) throws {
    if !ok() { throw Failure(description: "\(file):\(line): \(what())") }
}

func equal<T: Equatable>(_ got: T, _ want: T, _ what: @autoclosure () -> String, file: StaticString = #fileID, line: UInt = #line) throws {
    if got != want { throw Failure(description: "\(file):\(line): \(what()): got \(got), want \(want)") }
}

/// The text client, as the framework's own protocol: what a field would show, and every call made on it. The
/// controller gets it as `sender`, exactly as it gets a real client.
final class FakeClient: NSObject, IMKTextInput {
    private(set) var committed = ""
    private(set) var marked = ""
    /// The caret inside the marked text (the selection range of the last setMarkedText).
    private(set) var selection = NSRange(location: 0, length: 0)
    private(set) var inserts = 0
    private(set) var setMarkedCalls = 0
    /// Replacement ranges that named a place in the document (the input method must never do that).
    private(set) var namedRanges = 0
    /// What the field has selected (asked by selectedRange()).
    var documentSelection = NSRange(location: 0, length: 0)
    var caret = NSRect(x: 300, y: 400, width: 1, height: 18)

    func insertText(_ string: Any!, replacementRange: NSRange) {
        if replacementRange.location != NSNotFound { namedRanges += 1 }
        committed += (string as? String) ?? (string as? NSAttributedString)?.string ?? ""
        inserts += 1
        marked = "" // inserting replaces the marked text
    }

    func setMarkedText(_ string: Any!, selectionRange: NSRange, replacementRange: NSRange) {
        if replacementRange.location != NSNotFound { namedRanges += 1 }
        marked = (string as? NSAttributedString)?.string ?? (string as? String) ?? ""
        selection = selectionRange
        setMarkedCalls += 1
    }

    func selectedRange() -> NSRange { documentSelection }
    func markedRange() -> NSRange { marked.isEmpty ? NSRange(location: NSNotFound, length: 0) : NSRange(location: 0, length: (marked as NSString).length) }
    func attributedSubstring(from range: NSRange) -> NSAttributedString! { NSAttributedString() }
    func length() -> Int { 0 }
    func characterIndex(for point: NSPoint, tracking mappingMode: IMKLocationToOffsetMappingMode, inMarkedRange: UnsafeMutablePointer<ObjCBool>!) -> Int { 0 }
    func attributes(forCharacterIndex index: Int, lineHeightRectangle lineRect: UnsafeMutablePointer<NSRect>!) -> [AnyHashable: Any]! {
        lineRect?.pointee = caret
        return [:]
    }
    func validAttributesForMarkedText() -> [Any]! { [] }
    func overrideKeyboard(withKeyboardNamed keyboardUniqueName: String!) {}
    func selectMode(_ modeIdentifier: String!) {}
    func supportsUnicode() -> Bool { true }
    func bundleIdentifier() -> String! { "test.client" }
    func windowLevel() -> CGWindowLevel { 0 }
    func supportsProperty(_ property: TSMDocumentPropertyTag) -> Bool { false }
    func uniqueClientIdentifierString() -> String! { "test" }
    func string(from range: NSRange, actualRange: NSRangePointer!) -> String! { "" }
    func firstRect(forCharacterRange aRange: NSRange, actualRange: NSRangePointer!) -> NSRect { caret }
}

/// The candidate window: records instead of opening a panel.
final class Recorder: CandidateDisplay {
    struct Shown: Equatable {
        var items: [String]
        var selected: Int
        var page: Int
        var total: Int
        var perPage: Int
    }
    private(set) var shown: Shown?
    private(set) var caret = NSRect.zero
    private(set) var hides = 0
    private var pick: ((Int) -> Void)?

    func show(items: [String], selected: Int, page: Int, total: Int, perPage: Int, caret: NSRect, pick: @escaping (Int) -> Void) {
        shown = Shown(items: items, selected: selected, page: page, total: total, perPage: perPage)
        self.caret = caret
        self.pick = pick
    }

    func hide() {
        shown = nil
        hides += 1
        pick = nil // as the panel does: the callback keeps the client
    }

    func click(_ index: Int) { pick?(index) }
}

enum Keys {
    static let enter: UInt16 = 36, esc: UInt16 = 53, space: UInt16 = 49, down: UInt16 = 125, pageDown: UInt16 = 121
    static let leftShift: UInt16 = 56, rightShift: UInt16 = 60, control: UInt16 = 59, capsLock: UInt16 = 57
    /// A key the input method never takes (F13): reaches the engine, changes nothing.
    static let other: UInt16 = 105
    /// The key at the position that prints this ASCII character on a US keyboard.
    static let ofCharacter: [Character: UInt16] = {
        var m: [Character: UInt16] = [" ": space]
        for (code, key) in KeyMap.table where key >= 48 && key <= 122 {
            m[Character(UnicodeScalar(UInt8(key)))] = code
        }
        m[","] = 43; m["."] = 47; m["/"] = 44; m[";"] = 41; m["-"] = 27
        return m
    }()
}

/// Every event made here has a time stamp of its own, as every change of a real key has.
var eventClock: TimeInterval = 1000
func nextStamp() -> TimeInterval { eventClock += 0.050; return eventClock }

func keyDown(_ code: UInt16, _ flags: NSEvent.ModifierFlags = []) -> NSEvent {
    NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: flags, timestamp: nextStamp(), windowNumber: 0, context: nil, characters: "", charactersIgnoringModifiers: "", isARepeat: false, keyCode: code)!
}

func flags(_ code: UInt16, _ flags: NSEvent.ModifierFlags, stamp: TimeInterval? = nil) -> NSEvent {
    NSEvent.keyEvent(with: .flagsChanged, location: .zero, modifierFlags: flags, timestamp: stamp ?? nextStamp(), windowNumber: 0, context: nil, characters: "", charactersIgnoringModifiers: "", isARepeat: false, keyCode: code)!
}

func mouseDown() -> NSEvent {
    NSEvent.mouseEvent(with: .leftMouseDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil, eventNumber: 0, clickCount: 1, pressure: 1)!
}

/// A controller with its client, as the framework would pair them. The controller only ever sees the client as the
/// `sender` of a call, as with a real one.
final class Field {
    private(set) var client: FakeClient! = FakeClient()
    private(set) var controller: TwPotatoInputController?
    /// The longest `handle` so far, in seconds.
    private(set) var slowest = 0.0

    // Every call in a pool of its own: nothing the framework autoreleases may keep the controller alive (a released
    // controller must close its session there and then).
    init() throws {
        autoreleasepool { controller = TwPotatoInputController(server: nil, delegate: nil, client: client) }
        try check(controller != nil, "the controller could not be made")
    }

    func activate() { autoreleasepool { controller!.activateServer(client) } }
    func deactivate() { autoreleasepool { controller!.deactivateServer(client) } }
    func commitComposition() { autoreleasepool { controller!.commitComposition(client) } }
    func release() { autoreleasepool { controller = nil } }
    /// The field's own reference to its client goes: as with a real client, which the input method never owns.
    func dropClient() { autoreleasepool { client = nil } }
    var reply: ShellReply? { controller!.lastReply }

    /// `handle` and how long it took.
    @discardableResult
    func handle(_ event: NSEvent) -> (handled: Bool, seconds: Double) {
        let t = DispatchTime.now().uptimeNanoseconds
        let handled = autoreleasepool { controller!.handle(event, client: client) }
        let seconds = Double(DispatchTime.now().uptimeNanoseconds - t) / 1e9
        slowest = max(slowest, seconds)
        return (handled, seconds)
    }

    @discardableResult
    func key(_ code: UInt16, _ f: NSEvent.ModifierFlags = []) -> Bool { handle(keyDown(code, f)).handled }

    /// Types the keys that print these characters on a US keyboard; every one must be taken.
    func type(_ characters: String) throws {
        for ch in characters {
            try check(key(Keys.ofCharacter[ch]!), "key \(ch) was not handled")
        }
    }

    /// One Shift pressed and released alone.
    func tapShift(right: Bool = false) throws {
        let code = right ? Keys.rightShift : Keys.leftShift
        try check(!handle(flags(code, .shift)).handled && !handle(flags(code, [])).handled, "a Shift event was reported handled")
    }

    var mode: UInt32 { reply?.mode ?? 0 }
}

/// The engine of this stage: started by the shell library through the wrapper test.sh installs, which notes its
/// process id (macOS cannot tell the process at the other end of a socket).
enum Engine {
    static let stage = ProcessInfo.processInfo.environment["TWP_STAGE"]!
    static let home = ProcessInfo.processInfo.environment["HOME"]!
    static var socket: String { home + "/Library/Caches/TwPotato/engine.sock" }
    static var startsFile: String { stage + "/engine-starts" }

    /// One line per start of the engine: its process id.
    static func starts() -> [Int32] {
        ((try? String(contentsOfFile: startsFile, encoding: .utf8)) ?? "").split(separator: "\n").compactMap { Int32($0) }
    }

    private static func ps() -> [(pid: Int32, state: String, command: String)] {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/bin/ps")
        p.arguments = ["-axww", "-o", "pid=,stat=,command="]
        let pipe = Pipe()
        p.standardOutput = pipe
        try! p.run()
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        p.waitUntilExit()
        return String(decoding: data, as: UTF8.self).split(separator: "\n").compactMap { line in
            let f = line.split(separator: " ", maxSplits: 2, omittingEmptySubsequences: true)
            guard f.count == 3, let pid = Int32(f[0]) else { return nil }
            return (pid, String(f[1]), String(f[2]))
        }
    }

    /// The engines of this stage that are alive (not zombies): processes the wrapper noted whose command line is the
    /// engine with this stage's assets.
    static func running() -> [(pid: Int32, state: String)] {
        let noted = Set(starts())
        return ps().filter { noted.contains($0.pid) && !$0.state.hasPrefix("Z") && $0.command.contains("twime-engined") && $0.command.contains(stage + "/T.app/Contents/Resources/assets") }
            .map { ($0.pid, $0.state) }
    }

    /// The one engine. Never 0 or less: a signal to such an id would reach this whole process group.
    static func pid() throws -> Int32 {
        let all = running()
        try check(all.count == 1, "engines of this stage: \(all)")
        try check(all[0].pid > 0, "process id \(all[0].pid)")
        return all[0].pid
    }

    static func killAll() {
        let until = Date().addingTimeInterval(10)
        while true {
            let all = running()
            if all.isEmpty { return }
            for e in all where e.pid > 0 {
                kill(e.pid, SIGCONT)
                kill(e.pid, SIGKILL)
            }
            precondition(Date() < until, "the engine does not end")
            usleep(10_000)
        }
    }

    /// (open sessions, sessions composing), asked from the engine itself (twime-proto: session 0, event Status).
    /// `nil`: no engine answers.
    static func status() -> (sessions: Int, composing: Int)? {
        let fd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { return nil }
        defer { close(fd) }
        var tv = timeval(tv_sec: 2, tv_usec: 0)
        setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let path = Array(socket.utf8)
        guard path.count < MemoryLayout.size(ofValue: addr.sun_path) else { return nil }
        withUnsafeMutableBytes(of: &addr.sun_path) { $0.copyBytes(from: path) }
        let connected = withUnsafePointer(to: &addr) { $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) } }
        guard connected == 0 else { return nil }
        // version 1, session 0, sequence 0, no acknowledgement, event 11 (Status); a 4-byte length in front.
        let request: [UInt8] = [11, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 11]
        guard write(fd, request, request.count) == request.count else { return nil }
        var buf = [UInt8](repeating: 0, count: 64)
        var got = 0
        while got < 11 {
            let n = read(fd, &buf[got], buf.count - got)
            if n <= 0 { return nil }
            got += n
        }
        guard buf[4] == 5 else { return nil }
        return (Int(buf[5]) | Int(buf[6]) << 8, Int(buf[7]) | Int(buf[8]) << 8)
    }

    /// Presses a key the engine does nothing with until the field has a session in a running engine; how long it took.
    @discardableResult
    static func wait(_ field: Field, sessions: Int = 1, limit: Double = 20) throws -> Double {
        let t = Date()
        while true {
            field.key(Keys.other)
            if let s = status(), s.sessions >= sessions, field.reply?.noEngine == false { break }
            try check(Date().timeIntervalSince(t) < limit, "no engine after \(limit) s")
            usleep(20_000)
        }
        return Date().timeIntervalSince(t)
    }
}
