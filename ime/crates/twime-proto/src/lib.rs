//! Shared types and wire format between the TSF shell and the engine.
//!
//! No dependencies and no platform types. The wire format is little-endian and length-prefixed
//! (`u32` payload length, then the payload); a payload is at most `MAX_FRAME` bytes. All decoders are bounds-checked
//! and return `None` on malformed input; they never panic.

/// Version 2 (N3b) adds `Event::Hello` and the candidate tag 2 (`Cands::total` / `per_page`). Version 3 (UX1)
/// adds the new key codes, `Mods::CAPS`, `Event::ToggleMode`, the reply message
/// tag 6 (`Update::punct_half` / `run` / `symbols`, `Segment::run`). Older peers keep working: a connection that did not
/// negotiate version 3 gets exactly the version 1 or 2 bytes and the engine switches the new behaviour off. Version 4
/// adds `Event::DocText`; the shell sends it only on a connection that negotiated 4.
/// Version 5 adds user settings/lexicon requests, capabilities and Update toggle key/notices.
pub const PROTOCOL_VERSION: u16 = 7;
pub const CAP_DOC_TEXT: u8 = 1;
pub const USER_PAGE_BYTES: usize = 60 * 1024;
/// Most UTF-16 units of document text the shell sends in one `Event::DocText` (CACHE plan 3.1).
pub const DOC_TEXT_UNITS: usize = 8192;
/// Maximum payload length of one frame (64 KB).
pub const MAX_FRAME: usize = 64 * 1024;
/// Composition limit: a 40th syllable first commits (AI.md).
pub const MAX_SYLLABLES: usize = 39;

/// Fixed identifiers shared by the TSF shell and the setup tool (random GUIDs generated for this project).
pub mod ids {
    /// COM class of the text input processor.
    pub const CLSID_TIP: u128 = 0xc2dd0c9b_f6e9_489d_9adf_f97658502774;
    /// Language profile (zh-TW, 0x0404) of the input method.
    pub const PROFILE: u128 = 0xdddad6e8_b6b8_4941_a61b_7bb8046d7aed;
    pub const LANGID_ZH_TW: u16 = 0x0404;
    /// Display name in the language list.
    pub const DESCRIPTION: &str = "地瓜輸入法";
    pub const DLL_NAME: &str = "twime_tsf.dll";
    pub const ENGINE_NAME: &str = "twime-engined.exe";
    /// Marker file in the install root: set while an uninstall is in progress.
    pub const UNINSTALL_MARKER: &str = "uninstalling";

    /// `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}` form.
    pub fn guid_string(g: u128) -> String {
        format!(
            "{{{:08X}-{:04X}-{:04X}-{:04X}-{:012X}}}",
            (g >> 96) as u32,
            (g >> 80) as u16,
            (g >> 64) as u16,
            (g >> 48) as u16,
            (g & 0xFFFF_FFFF_FFFF) as u64
        )
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Keys

/// Physical key position (US layout names). The shell converts VK / scan code into this; the zhuyin mapping lives
/// in `twime-core`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// 'a'..='z' (lower case letter of the key position)
    Letter(u8),
    /// 0..=9 on the main row
    Digit(u8),
    Minus,
    Comma,
    Period,
    Slash,
    Semicolon,
    Space,
    Enter,
    Esc,
    Backspace,
    Delete,
    Tab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    /// Any other key; never handled.
    Other,
    // ---- version 3 (the shell only produces these on a version 3 connection) ----
    /// `` ` `` (the key left of 1).
    Backquote,
    LBracket,
    RBracket,
    /// `'`
    Quote,
    Backslash,
    Equals,
    /// Number pad 0..=9.
    NumpadDigit(u8),
    NumpadDecimal,
    /// Number pad operator: 0 `+`, 1 `-`, 2 `*`, 3 `/`.
    NumpadOp(u8),
}

impl Key {
    pub fn code(self) -> u8 {
        match self {
            Key::Letter(c) => c, // 'a'..'z' = 97..122
            Key::Digit(d) => 48 + d, // 48..57
            Key::Minus => 1,
            Key::Comma => 2,
            Key::Period => 3,
            Key::Slash => 4,
            Key::Semicolon => 5,
            Key::Space => 6,
            Key::Enter => 7,
            Key::Esc => 8,
            Key::Backspace => 9,
            Key::Delete => 10,
            Key::Tab => 11,
            Key::Left => 12,
            Key::Right => 13,
            Key::Up => 14,
            Key::Down => 15,
            Key::Home => 16,
            Key::End => 17,
            Key::PageUp => 18,
            Key::PageDown => 19,
            Key::Other => 0,
            Key::Backquote => 20,
            Key::LBracket => 21,
            Key::RBracket => 22,
            Key::Quote => 23,
            Key::Backslash => 24,
            Key::Equals => 25,
            Key::NumpadDecimal => 26,
            Key::NumpadDigit(d) => 30 + d, // 30..39
            Key::NumpadOp(o) => 40 + o,    // 40..43
        }
    }

    pub fn from_code(c: u8) -> Option<Key> {
        Some(match c {
            97..=122 => Key::Letter(c),
            48..=57 => Key::Digit(c - 48),
            1 => Key::Minus,
            2 => Key::Comma,
            3 => Key::Period,
            4 => Key::Slash,
            5 => Key::Semicolon,
            6 => Key::Space,
            7 => Key::Enter,
            8 => Key::Esc,
            9 => Key::Backspace,
            10 => Key::Delete,
            11 => Key::Tab,
            12 => Key::Left,
            13 => Key::Right,
            14 => Key::Up,
            15 => Key::Down,
            16 => Key::Home,
            17 => Key::End,
            18 => Key::PageUp,
            19 => Key::PageDown,
            0 => Key::Other,
            20 => Key::Backquote,
            21 => Key::LBracket,
            22 => Key::RBracket,
            23 => Key::Quote,
            24 => Key::Backslash,
            25 => Key::Equals,
            26 => Key::NumpadDecimal,
            30..=39 => Key::NumpadDigit(c - 30),
            40..=43 => Key::NumpadOp(c - 40),
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods(pub u8);

impl Mods {
    pub const SHIFT: u8 = 1;
    pub const CTRL: u8 = 2;
    pub const ALT: u8 = 4;
    pub const WIN: u8 = 8;
    /// Caps Lock is on (version 3; an overlay on the mode, not a modifier of the key).
    pub const CAPS: u8 = 16;
    /// All defined bits.
    pub const ALL: u8 = 31;
    pub fn shift(self) -> bool {
        self.0 & Self::SHIFT != 0
    }
    pub fn caps(self) -> bool {
        self.0 & Self::CAPS != 0
    }
    /// Ctrl, Alt or Win held: a shortcut, never handled.
    pub fn command(self) -> bool {
        self.0 & (Self::CTRL | Self::ALT | Self::WIN) != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub mods: Mods,
    pub down: bool,
}

impl KeyEvent {
    pub fn press(key: Key) -> KeyEvent {
        KeyEvent { key, mods: Mods(0), down: true }
    }
}

/// The part of the session state `peek` needs; the shell keeps it from the last confirmed `Update`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PeekView {
    /// A composition exists (bopomofo, an English run or the symbol table anchor).
    pub composing: bool,
    pub candidates_open: bool,
    /// Input mode (version 3 sessions).
    pub mode: Mode,
    /// The composition is an English run.
    pub run: bool,
    /// The symbol table is open.
    pub symbols: bool,
    /// The connection speaks version 3: the new rules apply. The shell sets it; an `Update` does not carry it.
    pub ux: bool,
}

/// Standard bopomofo keyboard: does this key type a bopomofo symbol?
pub fn is_zhuyin_key(k: Key) -> bool {
    matches!(
        k,
        Key::Letter(_) | Key::Digit(_) | Key::Minus | Key::Comma | Key::Period | Key::Slash | Key::Semicolon
    )
}

/// Pure, side-effect-free prediction of whether the engine may handle (swallow) the key. Used by `OnTestKeyDown`;
/// `twime-core` is tested never to swallow a key for which this is false (it may let a key pass although this is true:
/// Caps Lock and the keys that end an English run).
///
/// Rules: key-up and Ctrl/Alt/Win combinations are never handled. Version 1/2 (`ux` false): not composing, a bare
/// bopomofo key starts a composition (Shift+key and everything else pass through); composing, every other key is
/// swallowed, so the host caret cannot move under a composition. Version 3: a composition (also an English run or the
/// symbol table) is open to every key; without one, nothing is handled in English mode or with Caps Lock on, otherwise
/// every key that types a character is handled (bopomofo, Shift+letter, punctuation, number pad).
pub fn peek(view: &PeekView, ev: KeyEvent) -> bool {
    if !ev.down || ev.mods.command() || ev.key == Key::Other {
        return false;
    }
    if view.composing || (view.ux && (view.run || view.symbols)) {
        return true;
    }
    if !view.ux {
        return !ev.mods.shift() && is_zhuyin_key(ev.key);
    }
    if view.mode == Mode::English || ev.mods.caps() {
        return false;
    }
    matches!(
        ev.key,
        Key::Letter(_)
            | Key::Digit(_)
            | Key::Minus
            | Key::Comma
            | Key::Period
            | Key::Slash
            | Key::Semicolon
            | Key::Backquote
            | Key::LBracket
            | Key::RBracket
            | Key::Quote
            | Key::Backslash
            | Key::Equals
            | Key::NumpadDigit(_)
            | Key::NumpadDecimal
            | Key::NumpadOp(_)
    )
}

// ---------------------------------------------------------------------------------------------------------------
// Events, results, updates

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    /// The Update had no such part (e.g. an ordinary key has no `commit`).
    NotApplicable,
    Applied,
    Failed,
}

/// Shell confirmation of the previous Update, piggy-backed on the next event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommitResult {
    pub seq: u32,
    pub commit: Applied,
    pub comp: Applied,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Idle(u32),
    DownCompletion(u32),
    Key(KeyEvent),
    /// Focus entered (true) or left (false) the context.
    Focus(bool),
    /// Text changed or selection moved by something other than this input method.
    ExternalEdit,
    /// The host terminated the composition.
    CompositionAborted,
    /// A host write failed; the host state is unknown.
    Unsynced,
    CandidatePick(u16),
    Password(bool),
    /// Drop the session.
    Close,
    /// Liveness / latency probe; the engine answers with an empty Update.
    Ping,
    /// Test and tooling only: replaces the remembered context (at most 64 characters are used). The engine refuses it
    /// unless it was started with test events enabled; the TSF shell never sends it.
    SetContext(String),
    /// Control connection (uninstall tool): ask for session counts. Never creates a session.
    Status,
    /// Control connection: end the engine process cleanly. Only honored while the uninstall marker exists.
    Shutdown,
    /// Version negotiation (version 2): the highest protocol version the shell speaks. Sent before any session event on a
    /// connection; creates no session. The engine answers `Reply::Hello`.
    Hello(u16),
    /// Version 3: toggle Chinese / English (a clean Shift tap, or the language bar button). The shell never sends it
    /// with Caps Lock on.
    ToggleMode,
    /// Version 4: the document text before the start of the composition that the next key begins
    /// (at most `DOC_TEXT_UNITS` UTF-16 units; empty when it could not be read). Only the bigram cache uses it; the engine
    /// never logs or keeps it beyond the current composition.
    DocText(String),
    /// Version 5 only.
    AddToLexicon,
    User(UserReq),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Chinese,
    English,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Segment {
    /// 0: none, 1: dotted underline, 2: active dotted underline (v6).
    pub hint: u8,
    pub text: String,
    /// The user locked this position with a candidate.
    pub locked: bool,
    /// Bopomofo still being typed (not yet converted).
    pub pending: bool,
    /// Syllable without a legal character: shown as typed.
    pub unconverted: bool,
    /// Part of an English run (version 3).
    pub run: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Comp {
    pub segments: Vec<Segment>,
    /// Cursor position in characters of the concatenated text.
    pub cursor: u16,
}

impl Comp {
    pub fn text(&self) -> String {
        self.segments.iter().map(|s| s.text.as_str()).collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Cands {
    pub items: Vec<String>,
    pub selected: u16,
    pub page: u16,
    /// Number of candidates in all pages (0 = unknown, version 1 peer).
    pub total: u16,
    /// Candidates per page (0 = unknown).
    pub per_page: u8,
}

/// Frozen LEXUX cycle. Original output is option zero, or one for automatic swaps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LexuxHint {
    pub options: Vec<String>,
    pub current: usize,
    pub auto: bool,
    /// AUTOCOMP (AUTOCOMP-plan 1.3): option zero (the original output) stays in the cycle but is not listed;
    /// listed item i is cycle index i + 1, and current = 0 highlights nothing. Sent as hint kind 3 (never with `auto`).
    pub hide_first: bool,
}
impl LexuxHint {
    pub fn valid(&self) -> bool {
        self.options.len() >= 2 && self.options.len() <= u16::MAX as usize
            && self.current < self.options.len() && self.options.iter().all(|s| !s.is_empty())
            && !(self.hide_first && self.auto)
    }
    /// Identical labels for painting, TSF and the Win32 accessibility proxy (the listed items only).
    pub fn labels(&self) -> Vec<String> {
        let skip = usize::from(self.hide_first);
        self.options.iter().enumerate().skip(skip).map(|(i, s)| {
            format!("\\ {s}{}{}", if i == usize::from(self.auto) { "（原）" } else { "" },
                if i == self.current { "（目前）" } else { "" })
        }).collect()
    }
    pub fn text(&self) -> String {
        format!("按反斜線切換：{}", self.labels().join("　"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionHint {
    pub id: u32,
    pub options: Vec<String>,
}
impl CompletionHint {
    pub fn valid(&self) -> bool {
        self.id != 0 && (1..=9).contains(&self.options.len()) && self.options.iter().all(|s| !s.is_empty())
    }
    pub fn labels(&self) -> Vec<String> {
        self.options.iter().enumerate().map(|(i, s)| format!("{}.{}", i + 1, s)).collect()
    }
    pub fn text(&self) -> String { format!("↓ 選取　{}", self.labels().join(" ")) }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Update {
    pub pop_gen: u32,
    pub completion: Option<CompletionHint>,
    /// Frozen options and current index, only while switching is available (v6).
    pub hint: Option<LexuxHint>,
    pub toggle_key: ToggleKey,
    pub notice: Option<String>,
    pub handled: bool,
    pub commit: Option<String>,
    pub composition: Option<Comp>,
    pub candidates: Option<Cands>,
    pub composing: bool,
    pub candidates_open: bool,
    /// Every syllable has a legal character (or is literal bopomofo): a commit is allowed on every path. Enter and
    /// punctuation also commit a composition with a syllable without a character, showing it as typed (NEUTRAL).
    pub committable: bool,
    pub mode: Mode,
    /// Engine processing time in microseconds (for measurement only).
    pub engine_us: u32,
    /// Version 3: punctuation is half-width style (default: full-width).
    pub punct_half: bool,
    /// Version 3: the composition is an English run.
    pub run: bool,
    /// Version 3: the symbol table is open (the composition is its anchor).
    pub symbols: bool,
}

impl Update {
    /// The view for `peek`; `ux` (the connection speaks version 3) is known to the shell, not to the update.
    pub fn peek_view(&self, ux: bool) -> PeekView {
        PeekView { composing: self.composing, candidates_open: self.candidates_open, mode: self.mode, run: self.run, symbols: self.symbols, ux }
    }
}

/// One request of the shell: a session id, a sequence number, the confirmation of the previous Update and the event.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub session: u32,
    pub seq: u32,
    pub ack: Option<CommitResult>,
    pub event: Event,
}

/// Engine answer; `seq` echoes the request. `Disabled` is sent by an engine that has seen the uninstall marker.
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    HelloCaps { version: u16, caps: u8 },
    User(UserReply),
    Update { session: u32, seq: u32, update: Update },
    /// The engine refuses new input sessions (uninstalling); the shell goes to pass-through.
    Disabled { session: u32, seq: u32 },
    Hello { version: u16 },
    /// Answer to `Event::Status`: open input sessions, how many are composing, input connections that ended without closing
    /// their sessions since the engine started.
    Status { sessions: u16, composing: u16, abnormal: u16 },
}

// ---------------------------------------------------------------------------------------------------------------
// Encoding

struct W(Vec<u8>);
impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bool(&mut self, v: bool) {
        self.u8(u8::from(v));
    }
    fn str(&mut self, s: &str) {
        let b = s.as_bytes();
        self.u16(b.len().min(u16::MAX as usize) as u16);
        self.0.extend_from_slice(&b[..b.len().min(u16::MAX as usize)]);
    }
}

struct R<'a>(&'a [u8]);
impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn bool(&mut self) -> Option<bool> {
        match self.u8()? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
    fn str(&mut self) -> Option<String> {
        let n = self.u16()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
    fn done(&self) -> bool {
        self.0.is_empty()
    }
}

fn applied_code(a: Applied) -> u8 {
    match a {
        Applied::NotApplicable => 0,
        Applied::Applied => 1,
        Applied::Failed => 2,
    }
}
fn applied_from(c: u8) -> Option<Applied> {
    Some(match c {
        0 => Applied::NotApplicable,
        1 => Applied::Applied,
        2 => Applied::Failed,
        _ => return None,
    })
}

impl Request {
    pub fn encode(&self) -> Vec<u8> { self.encode_for(PROTOCOL_VERSION) }
    pub fn encode_for(&self, version: u16) -> Vec<u8> {
        // A new event cannot be represented on an older connection. Callers must not send it.
        if version < 7 && matches!(self.event, Event::Idle(_) | Event::DownCompletion(_)) { return Vec::new(); }
        let mut w = W(Vec::with_capacity(24));
        w.u8(1); // message type: request
        w.u32(self.session);
        w.u32(self.seq);
        match &self.ack {
            None => w.u8(0),
            Some(a) => {
                w.u8(1);
                w.u32(a.seq);
                w.u8(applied_code(a.commit));
                w.u8(applied_code(a.comp));
            }
        }
        match &self.event {
            Event::Idle(g) => { w.u8(18); w.u32(*g); }
            Event::DownCompletion(id) => { w.u8(19); w.u32(*id); }
            Event::Key(k) => {
                w.u8(1);
                w.u8(k.key.code());
                w.u8(k.mods.0);
                w.bool(k.down);
            }
            Event::Focus(f) => {
                w.u8(2);
                w.bool(*f);
            }
            Event::ExternalEdit => w.u8(3),
            Event::CompositionAborted => w.u8(4),
            Event::Unsynced => w.u8(5),
            Event::CandidatePick(i) => {
                w.u8(6);
                w.u16(*i);
            }
            Event::Password(p) => {
                w.u8(7);
                w.bool(*p);
            }
            Event::Close => w.u8(8),
            Event::Ping => w.u8(9),
            Event::SetContext(s) => {
                w.u8(10);
                w.str(s);
            }
            Event::Status => w.u8(11),
            Event::Shutdown => w.u8(12),
            Event::Hello(v) => {
                w.u8(13);
                w.u16(*v);
            }
            Event::ToggleMode => w.u8(14),
            Event::AddToLexicon => w.u8(16),
            Event::User(req) => { w.u8(17); enc_req(&mut w, req); }
            Event::DocText(s) => {
                w.u8(15);
                w.str(s);
            }
        }
        w.0
    }

    pub fn decode(b: &[u8]) -> Option<Request> {
        let mut r = R(b);
        if r.u8()? != 1 {
            return None;
        }
        let session = r.u32()?;
        let seq = r.u32()?;
        let ack = match r.u8()? {
            0 => None,
            1 => Some(CommitResult { seq: r.u32()?, commit: applied_from(r.u8()?)?, comp: applied_from(r.u8()?)? }),
            _ => return None,
        };
        let event = match r.u8()? {
            1 => {
                let key = Key::from_code(r.u8()?)?;
                let mods = Mods(r.u8()?);
                if mods.0 > Mods::ALL {
                    return None;
                }
                Event::Key(KeyEvent { key, mods, down: r.bool()? })
            }
            2 => Event::Focus(r.bool()?),
            3 => Event::ExternalEdit,
            4 => Event::CompositionAborted,
            5 => Event::Unsynced,
            6 => Event::CandidatePick(r.u16()?),
            7 => Event::Password(r.bool()?),
            8 => Event::Close,
            9 => Event::Ping,
            10 => Event::SetContext(r.str()?),
            11 => Event::Status,
            12 => Event::Shutdown,
            13 => Event::Hello(r.u16()?),
            14 => Event::ToggleMode,
            15 => Event::DocText(r.str()?),
            16 => Event::AddToLexicon,
            17 => Event::User(dec_req(&mut r)?),
            18 => Event::Idle(r.u32()?),
            19 => Event::DownCompletion(r.u32()?),
            _ => return None,
        };
        r.done().then_some(Request { session, seq, ack, event })
    }
}

fn enc_update(w: &mut W, u: &Update, version: u16) {
    w.bool(u.handled);
    w.bool(u.composing);
    w.bool(u.candidates_open);
    w.bool(u.committable);
    w.u8(match u.mode {
        Mode::Chinese => 0,
        Mode::English => 1,
    });
    w.u32(u.engine_us);
    match &u.commit {
        None => w.u8(0),
        Some(s) => {
            w.u8(1);
            w.str(s);
        }
    }
    match &u.composition {
        None => w.u8(0),
        Some(c) => {
            w.u8(1);
            w.u16(c.cursor);
            w.u16(c.segments.len().min(255) as u16);
            for s in c.segments.iter().take(255) {
                let mut f = u8::from(s.locked) | (u8::from(s.pending) << 1) | (u8::from(s.unconverted) << 2);
                if version >= 3 {
                    f |= u8::from(s.run) << 3;
                }
                w.u8(f);
                if version >= 6 { w.u8(s.hint); }
                w.str(&s.text);
            }
        }
    }
    match &u.candidates {
        None => w.u8(0),
        Some(c) => {
            w.u8(if version >= 2 { 2 } else { 1 });
            w.u16(c.selected);
            w.u16(c.page);
            w.u16(c.items.len().min(255) as u16);
            for s in c.items.iter().take(255) {
                w.str(s);
            }
            if version >= 2 {
                w.u16(c.total);
                w.u8(c.per_page);
            }
        }
    }
    if version >= 3 {
        w.u8(u8::from(u.punct_half) | (u8::from(u.run) << 1) | (u8::from(u.symbols) << 2));
    }
}

/// `v3`: the message is tag 6 (segment bit 3 and the trailing flags byte exist).
fn dec_update(r: &mut R, v3: bool, v6: bool) -> Option<Update> {
    let handled = r.bool()?;
    let composing = r.bool()?;
    let candidates_open = r.bool()?;
    let committable = r.bool()?;
    let mode = match r.u8()? {
        0 => Mode::Chinese,
        1 => Mode::English,
        _ => return None,
    };
    let engine_us = r.u32()?;
    let commit = match r.u8()? {
        0 => None,
        1 => Some(r.str()?),
        _ => return None,
    };
    let composition = match r.u8()? {
        0 => None,
        1 => {
            let cursor = r.u16()?;
            let n = r.u16()? as usize;
            let mut segments = Vec::new();
            for _ in 0..n {
                let f = r.u8()?;
                if f > if v3 { 15 } else { 7 } {
                    return None;
                }
                let hint = if v6 { let h = r.u8()?; if h > 2 { return None } h } else { 0 };
                segments.push(Segment { hint, locked: f & 1 != 0, pending: f & 2 != 0, unconverted: f & 4 != 0, run: f & 8 != 0, text: r.str()? });
            }
            Some(Comp { segments, cursor })
        }
        _ => return None,
    };
    let candidates = match r.u8()? {
        0 => None,
        tag @ (1 | 2) => {
            let selected = r.u16()?;
            let page = r.u16()?;
            let n = r.u16()? as usize;
            let mut items = Vec::new();
            for _ in 0..n {
                items.push(r.str()?);
            }
            let (total, per_page) = if tag == 2 { (r.u16()?, r.u8()?) } else { (0, 0) };
            Some(Cands { items, selected, page, total, per_page })
        }
        _ => return None,
    };
    let (punct_half, run, symbols) = if v3 {
        let f = r.u8()?;
        if f > 7 {
            return None;
        }
        (f & 1 != 0, f & 2 != 0, f & 4 != 0)
    } else {
        (false, false, false)
    };
    Some(Update { pop_gen: 0, completion: None, hint: None, toggle_key: ToggleKey::default(), notice: None, handled, commit, composition, candidates, composing, candidates_open, committable, mode, engine_us, punct_half, run, symbols })
}

impl Reply {
    /// Version 1 bytes (what a peer that has not negotiated version 2 receives).
    pub fn encode(&self) -> Vec<u8> {
        self.encode_for(1)
    }

    /// Bytes for a connection that speaks `version` (1 through 6). Fields a version does not have are dropped.
    pub fn encode_for(&self, version: u16) -> Vec<u8> {
        let mut w = W(Vec::with_capacity(64));
        match self {
            Reply::Update { session, seq, update } => {
                w.u8(if version >= 7 { 11 } else if version >= 6 { 10 } else if version >= 5 { 7 } else if version >= 3 { 6 } else { 2 });
                w.u32(*session);
                w.u32(*seq);
                enc_update(&mut w, update, version);
                if version >= 5 { w.u8(update.toggle_key as u8); enc_opt(&mut w, &update.notice); }
                if version >= 6 {
                    if let Some(h) = update.completion.as_ref().filter(|h| version >= 7 && h.valid()) {
                        w.u8(4); w.u32(h.id); w.u8(h.options.len() as u8);
                        for option in &h.options { w.str(option); }
                    } else { match update.hint.as_ref().filter(|h| h.valid()) {
                        None => w.u8(0),
                        Some(h) => {
                            // Keep ordinary v6 hints byte-identical; tag 2 carries auto provenance, tag 3 a hidden option zero.
                            w.u8(if h.hide_first { 3 } else if h.auto { 2 } else { 1 }); w.u16(h.options.len() as u16); w.u16(h.current as u16);
                            for option in &h.options { w.str(option); }
                        }
                    }
                    }
                }
                if version >= 7 { w.u32(update.pop_gen); }
            }
            Reply::HelloCaps { version, caps } => { w.u8(8); w.u16(*version); w.u8(*caps); }
            Reply::User(rep) => { w.u8(9); enc_user(&mut w, rep); }
            Reply::Disabled { session, seq } => {
                w.u8(3);
                w.u32(*session);
                w.u32(*seq);
            }
            Reply::Hello { version } => {
                w.u8(4);
                w.u16(*version);
            }
            Reply::Status { sessions, composing, abnormal } => {
                w.u8(5);
                w.u16(*sessions);
                w.u16(*composing);
                w.u16(*abnormal);
            }
        }
        w.0
    }

    pub fn decode(b: &[u8]) -> Option<Reply> {
        let mut r = R(b);
        let rep = match r.u8()? {
            2 => {
                let session = r.u32()?;
                let seq = r.u32()?;
                Reply::Update { session, seq, update: dec_update(&mut r, false, false)? }
            }
            6 => {
                let session = r.u32()?;
                let seq = r.u32()?;
                Reply::Update { session, seq, update: dec_update(&mut r, true, false)? }
            }
            7 => {
                let session = r.u32()?; let seq = r.u32()?;
                let mut update = dec_update(&mut r, true, false)?;
                update.toggle_key = dec_toggle(&mut r)?; update.notice = dec_opt(&mut r)?;
                Reply::Update { session, seq, update }
            }
            tag @ (10 | 11) => {
                let session = r.u32()?; let seq = r.u32()?;
                let mut update = dec_update(&mut r, true, true)?;
                update.toggle_key = dec_toggle(&mut r)?;
                update.notice = dec_opt(&mut r)?;
                let hint_kind = r.u8()?;
                update.hint = match hint_kind {
                    0 => None,
                    1..=3 => {
                        let count = r.u16()? as usize;
                        let current = r.u16()? as usize;
                        let mut options = Vec::new();
                        for _ in 0..count { options.push(r.str()?); }
                        let hint = LexuxHint { options, current, auto: hint_kind == 2, hide_first: hint_kind == 3 };
                        if !hint.valid() { return None; }
                        Some(hint)
                    }
                    4 if tag == 11 => {
                        let id = r.u32()?; let count = r.u8()? as usize;
                        if !(1..=9).contains(&count) { return None; }
                        let mut options = Vec::with_capacity(count);
                        for _ in 0..count { options.push(r.str()?); }
                        let h = CompletionHint { id, options };
                        if !h.valid() { return None; }
                        update.completion = Some(h);
                        None
                    }
                    _ => return None,
                };
                if tag == 11 { update.pop_gen = r.u32()?; }
                Reply::Update { session, seq, update }
            }
            8 => Reply::HelloCaps { version: r.u16()?, caps: r.u8()? },
            9 => Reply::User(dec_user(&mut r)?),
            3 => Reply::Disabled { session: r.u32()?, seq: r.u32()? },
            4 => Reply::Hello { version: r.u16()? },
            5 => Reply::Status { sessions: r.u16()?, composing: r.u16()?, abnormal: r.u16()? },
            _ => return None,
        };
        r.done().then_some(rep)
    }
}

/// Prepends the 4-byte length; `None` if the payload exceeds `MAX_FRAME`.
pub fn frame(payload: &[u8]) -> Option<Vec<u8>> {
    if payload.len() > MAX_FRAME {
        return None;
    }
    let mut v = Vec::with_capacity(4 + payload.len());
    v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    v.extend_from_slice(payload);
    Some(v)
}

/// Length announced by a 4-byte header; `None` if it exceeds `MAX_FRAME`.
pub fn frame_len(header: [u8; 4]) -> Option<usize> {
    let n = u32::from_le_bytes(header) as usize;
    (n <= MAX_FRAME).then_some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_keys() -> Vec<Key> {
        let mut v: Vec<Key> = (b'a'..=b'z').map(Key::Letter).collect();
        v.extend((0..10).map(Key::Digit));
        v.extend([
            Key::Minus, Key::Comma, Key::Period, Key::Slash, Key::Semicolon, Key::Space, Key::Enter, Key::Esc,
            Key::Backspace, Key::Delete, Key::Tab, Key::Left, Key::Right, Key::Up, Key::Down, Key::Home, Key::End,
            Key::PageUp, Key::PageDown, Key::Other, Key::Backquote, Key::LBracket, Key::RBracket, Key::Quote, Key::Backslash,
            Key::Equals, Key::NumpadDecimal,
        ]);
        v.extend((0..10).map(Key::NumpadDigit));
        v.extend((0..4).map(Key::NumpadOp));
        v
    }

    #[test]
    fn key_codes_round_trip() {
        for k in all_keys() {
            assert_eq!(Key::from_code(k.code()), Some(k));
        }
        assert_eq!(Key::from_code(200), None);
    }

    #[test]
    fn request_round_trip() {
        let evs = [
            Event::Key(KeyEvent { key: Key::Letter(b'q'), mods: Mods(Mods::SHIFT | Mods::CTRL), down: true }),
            Event::Focus(true), Event::Focus(false), Event::ExternalEdit, Event::CompositionAborted, Event::Unsynced,
            Event::CandidatePick(3), Event::Password(true), Event::Close, Event::Ping, Event::SetContext("ab".into()), Event::Status, Event::Shutdown, Event::Hello(3), Event::ToggleMode,
            Event::DocText("ab".into()), Event::DocText(String::new()),
        ];
        for (i, e) in evs.into_iter().enumerate() {
            for ack in [None, Some(CommitResult { seq: 9, commit: Applied::Applied, comp: Applied::Failed })] {
                let r = Request { session: 7, seq: i as u32, ack, event: e.clone() };
                assert_eq!(Request::decode(&r.encode()), Some(r));
            }
        }
    }

    #[test]
    fn reply_round_trip() {
        let u = Update {
            handled: true,
            commit: Some("今天".into()),
            composition: Some(Comp {
                segments: vec![
                    Segment { text: "天".into(), locked: true, ..Default::default() },
                    Segment { text: "ㄊㄧ".into(), pending: true, ..Default::default() },
                    Segment { text: "ㄅㄅ".into(), unconverted: true, ..Default::default() },
                ],
                cursor: 2,
            }),
            candidates: Some(Cands { items: vec!["天".into(), "添".into()], selected: 1, page: 0, total: 0, per_page: 0 }),
            composing: true,
            candidates_open: true,
            committable: false,
            mode: Mode::English,
            engine_us: 1234,
            ..Default::default()
        };
        for r in [Reply::Update { session: 1, seq: 2, update: u }, Reply::Update { session: 1, seq: 2, update: Update::default() }, Reply::Disabled { session: 3, seq: 4 }, Reply::Hello { version: PROTOCOL_VERSION }, Reply::Status { sessions: 3, composing: 1, abnormal: 2 }] {
            assert_eq!(Reply::decode(&r.encode()), Some(r));
        }
    }

    #[test]
    fn version_2_candidates_carry_total_and_version_1_does_not() {
        let c = Cands { items: vec!["權利".into(), "權力".into()], selected: 0, page: 1, total: 47, per_page: 9 };
        let r = Reply::Update { session: 1, seq: 2, update: Update { candidates: Some(c.clone()), candidates_open: true, ..Default::default() } };
        assert_eq!(Reply::decode(&r.encode_for(2)), Some(r.clone()));
        let v1 = Reply::decode(&r.encode_for(1)).unwrap();
        let Reply::Update { update, .. } = v1 else { panic!() };
        assert_eq!(update.candidates, Some(Cands { total: 0, per_page: 0, ..c }));
        assert_eq!(r.encode(), r.encode_for(1));
        assert_ne!(r.encode_for(2), r.encode_for(1));
        // Tag 2 truncated before its two extra fields is rejected.
        let b = r.encode_for(2);
        for n in 0..b.len() {
            assert_eq!(Reply::decode(&b[..n]), None, "truncated to {n}");
        }
        let hello = Request { session: 0, seq: 0, ack: None, event: Event::Hello(PROTOCOL_VERSION) };
        assert_eq!(Request::decode(&hello.encode()), Some(hello));
    }

    #[test]
    fn malformed_input_is_rejected_without_panic() {
        let good = Request { session: 1, seq: 2, ack: None, event: Event::Key(KeyEvent::press(Key::Space)) }.encode();
        for n in 0..good.len() {
            assert_eq!(Request::decode(&good[..n]), None, "truncated to {n}");
        }
        let mut extra = good.clone();
        extra.push(0);
        assert_eq!(Request::decode(&extra), None);
        // Deterministic fuzz: random bytes never panic.
        let mut s = 0x1234_5678_9abc_def0u64;
        for _ in 0..20_000 {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            let len = (s % 40) as usize;
            let buf: Vec<u8> = (0..len).map(|i| (s >> ((i % 8) * 8)) as u8 ^ (i as u8).wrapping_mul(31)).collect();
            let _ = Request::decode(&buf);
            let _ = Reply::decode(&buf);
        }
        assert!(frame(&vec![0; MAX_FRAME + 1]).is_none());
        assert_eq!(frame_len(((MAX_FRAME + 1) as u32).to_le_bytes()), None);
        assert_eq!(frame_len(7u32.to_le_bytes()), Some(7));
    }

    #[test]
    fn peek_rules_of_version_3() {
        let ux = PeekView { ux: true, ..Default::default() };
        let k = |key, mods| KeyEvent { key, mods: Mods(mods), down: true };
        for key in [Key::Letter(b'a'), Key::Digit(3), Key::Minus, Key::Backquote, Key::LBracket, Key::Quote, Key::Backslash, Key::Equals, Key::NumpadDigit(4), Key::NumpadDecimal, Key::NumpadOp(1)] {
            assert!(peek(&ux, k(key, 0)) && peek(&ux, k(key, Mods::SHIFT)), "{key:?}");
            assert!(!peek(&ux, k(key, Mods::CAPS)) && !peek(&ux, k(key, Mods::CTRL)), "{key:?}");
            let english = PeekView { mode: Mode::English, ..ux };
            assert!(!peek(&english, k(key, 0)) && !peek(&english, k(key, Mods::SHIFT)), "{key:?}");
        }
        for key in [Key::Space, Key::Enter, Key::Esc, Key::Backspace, Key::Tab, Key::Left, Key::Other] {
            assert!(!peek(&ux, k(key, 0)) && !peek(&ux, k(key, Mods::SHIFT)), "{key:?}");
        }
        for v in [PeekView { composing: true, ..ux }, PeekView { run: true, ..ux }, PeekView { symbols: true, ..ux }, PeekView { composing: true, mode: Mode::English, ..ux }] {
            assert!(peek(&v, k(Key::Space, 0)) && peek(&v, k(Key::Letter(b'a'), Mods::CAPS)) && peek(&v, k(Key::Left, Mods::SHIFT)));
            assert!(!peek(&v, k(Key::Letter(b'v'), Mods::CTRL)) && !peek(&v, k(Key::Other, 0)));
        }
        // Version 1/2 rules do not change: Shift+letter and the new keys pass when idle.
        let old = PeekView::default();
        assert!(!peek(&old, k(Key::Letter(b'a'), Mods::SHIFT)) && !peek(&old, k(Key::Backquote, 0)) && !peek(&old, k(Key::NumpadDigit(1), 0)));
        assert!(peek(&old, k(Key::Letter(b'a'), 0)) && peek(&old, k(Key::Comma, 0)));
    }

    #[test]
    fn peek_rules() {
        let idle = PeekView::default();
        let busy = PeekView { composing: true, ..Default::default() };
        let down = |k| KeyEvent::press(k);
        assert!(peek(&idle, down(Key::Letter(b'q'))));
        assert!(peek(&idle, down(Key::Digit(1))));
        assert!(!peek(&idle, down(Key::Space)));
        assert!(!peek(&idle, down(Key::Enter)));
        assert!(!peek(&idle, down(Key::Backspace)));
        assert!(peek(&busy, down(Key::Space)));
        assert!(peek(&busy, down(Key::Left)));
        assert!(!peek(&busy, KeyEvent { key: Key::Letter(b'c'), mods: Mods(Mods::CTRL), down: true }));
        assert!(!peek(&busy, KeyEvent { key: Key::Letter(b'c'), mods: Mods(0), down: false }));
        assert!(!peek(&idle, KeyEvent { key: Key::Letter(b'a'), mods: Mods(Mods::SHIFT), down: true }));
        assert!(!peek(&busy, down(Key::Other)));
    }
}
// Version 5 pure data types. Positions and rejected indices are zero-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum ToggleKey {
    #[default]
    Shift = 0,
    LeftShift = 1,
    RightShift = 2,
    None = 3,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserSettings {
    pub toggle_key: ToggleKey,
    pub default_mode: Mode,
    pub punct_half: bool,
    pub symbol_table: bool,
    pub cache: bool,
}
impl Default for UserSettings {
    fn default() -> Self {
        Self {
            toggle_key: ToggleKey::Shift,
            default_mode: Mode::Chinese,
            punct_half: false,
            symbol_table: true,
            cache: false,
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct LexEntry {
    pub word: String,
    /// Syllables separated by one space.
    pub reading: String,
    pub weight: Option<f32>,
}
/// Wire codes 0..=9, with an additional position byte for codes 1..=3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LexError {
    Length,
    UnknownChar(u8),
    IllegalSyllable(u8),
    IllegalChar(u8),
    Weight,
    Full,
    Io,
    Busy,
    Disabled,
    BadReading,
}
#[derive(Clone, Debug, PartialEq)]
pub enum UserReq {
    GetSettings,
    SetSettings(UserSettings),
    List {
        query: String,
        offset: u32,
        limit: u16,
    },
    Add(Vec<LexEntry>),
    Remove(Vec<(String, String)>),
    ReadingOptions(String),
}
#[derive(Clone, Debug, PartialEq)]
pub enum UserReply {
    Settings {
        settings: UserSettings,
        file_error: bool,
        last_error: Option<String>,
    },
    Page {
        total: u32,
        offset: u32,
        entries: Vec<LexEntry>,
        invalid_lines: u32,
    },
    Added {
        added: u32,
        updated: u32,
        rejected: Vec<(u32, LexError)>,
    },
    Removed(u32),
    Readings {
        default: Option<String>,
        per_char: Vec<Vec<String>>,
    },
    Error(LexError),
}
fn enc_opt(w: &mut W, s: &Option<String>) {
    w.bool(s.is_some());
    if let Some(s) = s {
        w.str(s);
    }
}
fn dec_opt(r: &mut R) -> Option<Option<String>> {
    Some(if r.bool()? { Some(r.str()?) } else { None })
}
fn dec_toggle(r: &mut R) -> Option<ToggleKey> {
    Some(match r.u8()? {
        0 => ToggleKey::Shift,
        1 => ToggleKey::LeftShift,
        2 => ToggleKey::RightShift,
        3 => ToggleKey::None,
        _ => return None,
    })
}
fn enc_settings(w: &mut W, s: &UserSettings) {
    w.u8(s.toggle_key as u8);
    w.u8(if s.default_mode == Mode::Chinese {
        0
    } else {
        1
    });
    w.bool(s.punct_half);
    w.bool(s.symbol_table);
    w.bool(s.cache);
}
fn dec_settings(r: &mut R) -> Option<UserSettings> {
    Some(UserSettings {
        toggle_key: dec_toggle(r)?,
        default_mode: match r.u8()? {
            0 => Mode::Chinese,
            1 => Mode::English,
            _ => return None,
        },
        punct_half: r.bool()?,
        symbol_table: r.bool()?,
        cache: r.bool()?,
    })
}
fn enc_entry(w: &mut W, e: &LexEntry) {
    w.str(&e.word);
    w.str(&e.reading);
    w.bool(e.weight.is_some());
    if let Some(v) = e.weight {
        w.u32(v.to_bits());
    }
}
fn dec_entry(r: &mut R) -> Option<LexEntry> {
    Some(LexEntry {
        word: r.str()?,
        reading: r.str()?,
        weight: if r.bool()? {
            Some(f32::from_bits(r.u32()?))
        } else {
            None
        },
    })
}
pub fn encoded_len_entry(e: &LexEntry) -> usize {
    5 + e.word.len().min(u16::MAX as usize)
        + e.reading.len().min(u16::MAX as usize)
        + if e.weight.is_some() { 4 } else { 0 }
}
fn enc_error(w: &mut W, e: LexError) {
    match e {
        LexError::Length => w.u8(0),
        LexError::UnknownChar(p) => {
            w.u8(1);
            w.u8(p);
        }
        LexError::IllegalSyllable(p) => {
            w.u8(2);
            w.u8(p);
        }
        LexError::IllegalChar(p) => {
            w.u8(3);
            w.u8(p);
        }
        LexError::Weight => w.u8(4),
        LexError::Full => w.u8(5),
        LexError::Io => w.u8(6),
        LexError::Busy => w.u8(7),
        LexError::Disabled => w.u8(8),
        LexError::BadReading => w.u8(9),
    }
}
fn dec_error(r: &mut R) -> Option<LexError> {
    Some(match r.u8()? {
        0 => LexError::Length,
        1 => LexError::UnknownChar(r.u8()?),
        2 => LexError::IllegalSyllable(r.u8()?),
        3 => LexError::IllegalChar(r.u8()?),
        4 => LexError::Weight,
        5 => LexError::Full,
        6 => LexError::Io,
        7 => LexError::Busy,
        8 => LexError::Disabled,
        9 => LexError::BadReading,
        _ => return None,
    })
}
// Counts are u32. Never reserve memory from untrusted counts.
fn enc_req(w: &mut W, req: &UserReq) {
    match req {
        UserReq::GetSettings => w.u8(0),
        UserReq::SetSettings(s) => {
            w.u8(1);
            enc_settings(w, s);
        }
        UserReq::List {
            query,
            offset,
            limit,
        } => {
            w.u8(2);
            w.str(query);
            w.u32(*offset);
            w.u16(*limit);
        }
        UserReq::Add(v) => {
            w.u8(3);
            w.u32(v.len() as u32);
            for e in v {
                enc_entry(w, e);
            }
        }
        UserReq::Remove(v) => {
            w.u8(4);
            w.u32(v.len() as u32);
            for (a, b) in v {
                w.str(a);
                w.str(b);
            }
        }
        UserReq::ReadingOptions(s) => {
            w.u8(5);
            w.str(s);
        }
    }
}
fn dec_req(r: &mut R) -> Option<UserReq> {
    Some(match r.u8()? {
        0 => UserReq::GetSettings,
        1 => UserReq::SetSettings(dec_settings(r)?),
        2 => UserReq::List {
            query: r.str()?,
            offset: r.u32()?,
            limit: r.u16()?,
        },
        3 => {
            let n = r.u32()?;
            let mut v = vec![];
            for _ in 0..n {
                v.push(dec_entry(r)?);
            }
            UserReq::Add(v)
        }
        4 => {
            let n = r.u32()?;
            let mut v = vec![];
            for _ in 0..n {
                v.push((r.str()?, r.str()?));
            }
            UserReq::Remove(v)
        }
        5 => UserReq::ReadingOptions(r.str()?),
        _ => return None,
    })
}
fn enc_user(w: &mut W, rep: &UserReply) {
    match rep {
        UserReply::Settings {
            settings,
            file_error,
            last_error,
        } => {
            w.u8(0);
            enc_settings(w, settings);
            w.bool(*file_error);
            enc_opt(w, last_error);
        }
        UserReply::Page {
            total,
            offset,
            entries,
            invalid_lines,
        } => {
            w.u8(1);
            w.u32(*total);
            w.u32(*offset);
            w.u32(entries.len() as u32);
            for e in entries {
                enc_entry(w, e);
            }
            w.u32(*invalid_lines);
        }
        UserReply::Added {
            added,
            updated,
            rejected,
        } => {
            w.u8(2);
            w.u32(*added);
            w.u32(*updated);
            w.u32(rejected.len() as u32);
            for (i, e) in rejected {
                w.u32(*i);
                enc_error(w, *e);
            }
        }
        UserReply::Removed(n) => {
            w.u8(3);
            w.u32(*n);
        }
        UserReply::Readings { default, per_char } => {
            w.u8(4);
            enc_opt(w, default);
            w.u32(per_char.len() as u32);
            for v in per_char {
                w.u32(v.len() as u32);
                for s in v {
                    w.str(s);
                }
            }
        }
        UserReply::Error(e) => {
            w.u8(5);
            enc_error(w, *e);
        }
    }
}
fn dec_user(r: &mut R) -> Option<UserReply> {
    Some(match r.u8()? {
        0 => UserReply::Settings {
            settings: dec_settings(r)?,
            file_error: r.bool()?,
            last_error: dec_opt(r)?,
        },
        1 => {
            let total = r.u32()?;
            let offset = r.u32()?;
            let n = r.u32()?;
            let mut entries = vec![];
            for _ in 0..n {
                entries.push(dec_entry(r)?);
            }
            UserReply::Page {
                total,
                offset,
                entries,
                invalid_lines: r.u32()?,
            }
        }
        2 => {
            let added = r.u32()?;
            let updated = r.u32()?;
            let n = r.u32()?;
            let mut rejected = vec![];
            for _ in 0..n {
                rejected.push((r.u32()?, dec_error(r)?));
            }
            UserReply::Added {
                added,
                updated,
                rejected,
            }
        }
        3 => UserReply::Removed(r.u32()?),
        4 => {
            let default = dec_opt(r)?;
            let n = r.u32()?;
            let mut per_char = vec![];
            for _ in 0..n {
                let m = r.u32()?;
                let mut v = vec![];
                for _ in 0..m {
                    v.push(r.str()?);
                }
                per_char.push(v);
            }
            UserReply::Readings { default, per_char }
        }
        5 => UserReply::Error(dec_error(r)?),
        _ => return None,
    })
}
