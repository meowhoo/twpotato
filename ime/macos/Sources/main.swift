// The input method application: the server the system's text input connects to, and the run loop.

import AppKit
import InputMethodKit

guard let name = Bundle.main.infoDictionary?["InputMethodConnectionName"] as? String,
      let server = IMKServer(name: name, bundleIdentifier: Bundle.main.bundleIdentifier) else {
    FileHandle.standardError.write(Data("twpotato: cannot create the input method server\n".utf8))
    exit(1)
}
_ = server
Shared.candidates = CandidatePanel()
let modeToast = ModeToast()
Shared.showMode = { mode, caret in modeToast.show(mode: mode, caret: caret) }
Shared.hideMode = { modeToast.hide() }
NSApplication.shared.run()
