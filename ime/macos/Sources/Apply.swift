// A reply of the shell library as Swift values, and the conversion of its caret (characters = Unicode scalar values)
// to the UTF-16 units the text client counts in. Pure.

import Foundation
import CTwime

struct ShellReply {
    var flags: UInt32 = 0
    var mode: UInt32 = 0
    var commit = ""
    var preedit = ""
    /// Caret in the preedit, in Unicode scalar values.
    var cursor = 0
    /// (length in Unicode scalar values, TWIME_SEG_* flags) of each stretch of the preedit.
    var segments: [(chars: Int, flags: UInt32)] = []
    var candidates: [String] = []
    var selected = 0
    var page = 0
    var total = 0
    var perPage = 0
    var releaseBefore: UInt32 = 0

    func has(_ flag: Int32) -> Bool { flags & UInt32(flag) != 0 }
    var handled: Bool { has(TWIME_R_HANDLED) }
    var uiChanged: Bool { has(TWIME_R_UI_CHANGED) }
    var candidatesOpen: Bool { has(TWIME_R_CANDIDATES_OPEN) }
    var noEngine: Bool { has(TWIME_R_NO_ENGINE) }

    init() {}

    /// Copies everything out: the C reply is only good until the next event of the context.
    init(_ r: twime_reply) {
        func string(_ s: twime_str) -> String {
            guard s.len > 0, let p = s.ptr else { return "" }
            return String(decoding: UnsafeRawBufferPointer(start: p, count: s.len), as: UTF8.self)
        }
        flags = r.flags
        mode = r.mode
        commit = string(r.commit)
        preedit = string(r.preedit)
        cursor = Int(r.cursor)
        if let s = r.segments {
            segments = (0..<Int(r.n_segments)).map { (Int(s[$0].chars), s[$0].flags) }
        }
        if has(TWIME_R_CANDIDATES_OPEN), let c = r.candidates {
            candidates = (0..<Int(r.n_candidates)).map { string(c[$0]) }
        }
        selected = Int(r.selected)
        page = Int(r.page)
        total = Int(r.total)
        perPage = Int(r.per_page)
        releaseBefore = r.release_before
    }
}

enum TextUnits {
    /// The UTF-16 offset of the position `scalars` Unicode scalar values into `text` (clamped to its end).
    static func utf16Offset(in text: String, scalars: Int) -> Int {
        var units = 0
        for (i, s) in text.unicodeScalars.enumerated() {
            if i >= scalars { break }
            units += s.utf16.count
        }
        return units
    }
}
