// The candidate window: one panel for the process, drawn by us, that never takes the keyboard focus. It only shows
// what a reply says (the page, the highlight, the page number); paging and selection are the engine's.

import AppKit

/// What the controller needs of a candidate window (the tests record the calls instead of opening a panel).
protocol CandidateDisplay: AnyObject {
    func show(items: [String], selected: Int, page: Int, total: Int, perPage: Int, caret: NSRect, pick: @escaping (Int) -> Void)
    func hide()
}

enum PanelPlacement {
    /// Where the panel's origin (bottom left) goes: under the caret; above it when there is no room below; inside the
    /// screen from side to side. `nil`: the caret rectangle is of no use (no size at all, or on no screen) and the
    /// panel stays where it was.
    static func origin(caret: NSRect, panel: NSSize, screens: [NSRect]) -> NSPoint? {
        guard caret.width > 0 || caret.height > 0 else { return nil }
        let probe = NSPoint(x: caret.minX, y: caret.midY)
        guard let screen = screens.first(where: { $0.insetBy(dx: -1, dy: -1).contains(probe) }) else { return nil }
        let gap: CGFloat = 4
        var y = caret.minY - gap - panel.height
        if y < screen.minY {
            y = caret.maxY + gap
        }
        y = min(max(y, screen.minY), max(screen.minY, screen.maxY - panel.height))
        let x = min(max(caret.minX, screen.minX), max(screen.minX, screen.maxX - panel.width))
        return NSPoint(x: x, y: y)
    }
}

final class CandidateView: NSView {
    var items: [String] = []
    var selected = 0
    var footer = ""
    var pick: ((Int) -> Void)?

    static let font = NSFont.systemFont(ofSize: 16)
    static let small = NSFont.systemFont(ofSize: 11)
    static let rowHeight: CGFloat = 26
    static let footerHeight: CGFloat = 18
    static let pad: CGFloat = 8
    static let labelWidth: CGFloat = 20

    override var isFlipped: Bool { true }

    func fittingSize(for items: [String]) -> NSSize {
        let widest = items.map { ($0 as NSString).size(withAttributes: [.font: Self.font]).width }.max() ?? 0
        return NSSize(width: ceil(Self.pad * 2 + Self.labelWidth + widest), height: CGFloat(items.count) * Self.rowHeight + Self.footerHeight)
    }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.windowBackgroundColor.setFill()
        bounds.fill()
        for (i, item) in items.enumerated() {
            let row = NSRect(x: 0, y: CGFloat(i) * Self.rowHeight, width: bounds.width, height: Self.rowHeight)
            let on = i == selected
            if on {
                NSColor.selectedContentBackgroundColor.setFill()
                row.fill()
            }
            let color = on ? NSColor.alternateSelectedControlTextColor : NSColor.labelColor
            let label = on ? NSColor.alternateSelectedControlTextColor : NSColor.secondaryLabelColor
            // Labels 1..9 then 0, as the digit keys select.
            ("\((i + 1) % 10)" as NSString).draw(at: NSPoint(x: Self.pad, y: row.minY + 6), withAttributes: [.font: Self.small, .foregroundColor: label])
            (item as NSString).draw(at: NSPoint(x: Self.pad + Self.labelWidth, y: row.minY + 3), withAttributes: [.font: Self.font, .foregroundColor: color])
        }
        (footer as NSString).draw(at: NSPoint(x: Self.pad, y: CGFloat(items.count) * Self.rowHeight + 2), withAttributes: [.font: Self.small, .foregroundColor: NSColor.secondaryLabelColor])
    }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        let i = Int(p.y / Self.rowHeight)
        if p.y >= 0 && i < items.count {
            pick?(i)
        }
    }
}

final class CandidatePanel: CandidateDisplay {
    private let panel: NSPanel
    private let view = CandidateView()

    init() {
        panel = NSPanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: true)
        panel.level = .popUpMenu
        panel.hidesOnDeactivate = false
        panel.becomesKeyOnlyIfNeeded = true
        panel.isFloatingPanel = true
        panel.hasShadow = true
        // Follows the user to every desktop space and shows above a full-screen application.
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .transient]
        panel.contentView = view
    }

    func show(items: [String], selected: Int, page: Int, total: Int, perPage: Int, caret: NSRect, pick: @escaping (Int) -> Void) {
        view.items = items
        view.selected = selected
        view.pick = pick
        let pages = perPage > 0 ? (total + perPage - 1) / perPage : 1
        view.footer = "\(page + 1) / \(max(pages, 1))"
        let size = view.fittingSize(for: items)
        panel.setContentSize(size)
        if let origin = PanelPlacement.origin(caret: caret, panel: size, screens: NSScreen.screens.map(\.visibleFrame)) {
            panel.setFrameOrigin(origin)
        }
        view.needsDisplay = true
        panel.orderFrontRegardless()
    }

    func hide() {
        panel.orderOut(nil)
        view.pick = nil // and with it the client the callback keeps
    }
}

/// Chinese / English, shown for a moment at the caret when the mode changes (Shift alone switches it and nothing else
/// on the screen would say so).
final class ModeToast {
    private let panel: NSPanel
    private let label = NSTextField(labelWithString: "")
    private var hiding: DispatchWorkItem?
    private static let size = NSSize(width: 34, height: 30)

    init() {
        panel = NSPanel(contentRect: NSRect(origin: .zero, size: Self.size), styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: true)
        panel.level = .popUpMenu
        panel.hidesOnDeactivate = false
        panel.isFloatingPanel = true
        panel.hasShadow = true
        panel.ignoresMouseEvents = true
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .transient]
        panel.backgroundColor = .windowBackgroundColor
        label.font = NSFont.systemFont(ofSize: 17, weight: .medium)
        label.textColor = .labelColor
        label.alignment = .center
        label.frame = NSRect(x: 0, y: 4, width: Self.size.width, height: 22)
        panel.contentView?.addSubview(label)
    }

    /// `mode`: 0 Chinese, 1 English. Without a usable caret rectangle it shows at the mouse pointer.
    func show(mode: UInt32, caret: NSRect) {
        label.stringValue = mode == 0 ? "中" : "英"
        let screens = NSScreen.screens.map(\.visibleFrame)
        let mouse = NSEvent.mouseLocation
        let origin = PanelPlacement.origin(caret: caret, panel: Self.size, screens: screens)
            ?? PanelPlacement.origin(caret: NSRect(x: mouse.x, y: mouse.y, width: 1, height: 1), panel: Self.size, screens: screens)
        if let origin { panel.setFrameOrigin(origin) }
        panel.orderFrontRegardless()
        hiding?.cancel()
        let work = DispatchWorkItem { [weak self] in self?.panel.orderOut(nil) }
        hiding = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.0, execute: work)
    }

    func hide() {
        hiding?.cancel()
        panel.orderOut(nil)
    }
}
