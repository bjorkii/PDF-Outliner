//! 사용자가 고칠 수 있는 단축키.
//!
//! 전에는 `app.rs` 곳곳에서 `i.modifiers.command && i.key_pressed(Key::B)` 식으로 키를 직접 봤다.
//! 그러면 고칠 수 없고, 어떤 키가 이미 쓰이는지도 한눈에 알 수 없다. 여기 표 하나로 모아 두고
//! 설정 창의 `단축키` 탭이 이 표를 그린다.
//!
//! **바꿀 수 없는 기능도 목록에 둔다.** `Tab`(영역 전환)이 그렇다. 목록에서 빼면 사용자가 그 키의
//! 뜻을 알 길이 없다 — 지금은 툴바에 메뉴가 없어 단축키를 안내할 다른 자리가 없기 때문이다(예약 15로
//! '보기' 메뉴를 만들면 거기로 옮기고 여기서 뺀다). 바꾸려 하면 [`Conflict::Fixed`]로 막는다.
//!
//! `Esc`(닫기)·`Enter`(확정)·화살표(목록 이동)는 목록에도 넣지 않는다 — 어느 창에서나 같은 뜻이라
//! 설명할 것이 없다. 다만 **다른 기능에 그 키를 주려 할 때는 막아야** 하므로 [`RESERVED`]에 둔다.
//!
//! **겹침과 발동을 같은 기준으로 가른다.** 기능마다 "어느 포커스에서 듣는가"(`Scope`)를 적어 두고,
//! 겹침 검사도 발동 판정도 그 하나를 본다(`Shortcuts::pressed`). 둘을 따로 적으면 설정 창은 "안
//! 겹친다"고 하는데 실제로는 가로채는 일이 생긴다. 그래서 `F2`가 사이드바(북마크 제목)와 그 밖
//! (파일명)으로 나뉘어 같은 키를 쓴다.
//!
//! **보여 주는 묶음(`Category`)은 그와 따로다.** `Cmd+F`는 어디서나 듣지만 사용자에게는 뷰어 기능으로
//! 읽히는 식이다(2026-10-03 md 확정).

use std::collections::BTreeMap;

/// 단축키 목록에서 묶어 보여 줄 갈래(`planning/shortcuts.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    General,
    Viewer,
    Bookmark,
    Ocr,
}

impl Category {
    pub const ALL: [Category; 4] = [Category::General, Category::Viewer, Category::Bookmark, Category::Ocr];

    pub fn label(self) -> &'static str {
        match self {
            Category::General => "일반",
            Category::Viewer => "뷰어",
            Category::Bookmark => "북마크",
            Category::Ocr => "OCR",
        }
    }
}

/// 단축키를 붙일 수 있는 기능.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Action {
    FocusSwitch,
    RenameFile,
    PageStep,
    HistoryBack,
    HistoryForward,
    ZoomIn,
    ZoomOut,
    ToggleScrollMode,
    Search,
    AddBookmark,
    DeleteBookmark,
    RenameBookmark,
    SaveBookmarks,
    BookmarkStep,
    Undo,
    Redo,
    OcrOverlay,
    OcrBoxStep,
}

/// 그 단축키가 **어느 포커스에서 듣는가**. 겹침을 따질 때도, 실제로 발동시킬 때도 이 하나를 본다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// 어디에 포커스가 있든 듣는다.
    Global,
    /// 북마크 목록이 포커스일 때만.
    Sidebar,
    /// 사이드바 밖(뷰어·검색 결과)에 포커스가 있을 때만.
    Viewer,
}

impl Scope {
    fn overlaps(self, other: Self) -> bool {
        self == Scope::Global || other == Scope::Global || self == other
    }

    /// 지금 포커스가 이 영역에 드는가.
    pub fn allows(self, focus: crate::app::FocusArea) -> bool {
        match self {
            Scope::Global => true,
            Scope::Sidebar => focus == crate::app::FocusArea::Sidebar,
            Scope::Viewer => focus != crate::app::FocusArea::Sidebar,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Scope::Global => "어디서나",
            Scope::Sidebar => "북마크 목록",
            Scope::Viewer => "뷰어",
        }
    }
}

impl Action {
    pub const ALL: [Action; 18] = [
        Action::FocusSwitch,
        Action::RenameFile,
        Action::PageStep,
        Action::HistoryBack,
        Action::HistoryForward,
        Action::ZoomIn,
        Action::ZoomOut,
        Action::ToggleScrollMode,
        Action::Search,
        Action::AddBookmark,
        Action::DeleteBookmark,
        Action::RenameBookmark,
        Action::SaveBookmarks,
        Action::BookmarkStep,
        Action::Undo,
        Action::Redo,
        Action::OcrOverlay,
        Action::OcrBoxStep,
    ];

    pub fn category(self) -> Category {
        match self {
            Action::FocusSwitch | Action::RenameFile => Category::General,
            Action::PageStep
            | Action::HistoryBack
            | Action::HistoryForward
            | Action::ZoomIn
            | Action::ZoomOut
            | Action::ToggleScrollMode
            | Action::Search => Category::Viewer,
            Action::AddBookmark
            | Action::DeleteBookmark
            | Action::RenameBookmark
            | Action::SaveBookmarks
            | Action::BookmarkStep
            | Action::Undo
            | Action::Redo => Category::Bookmark,
            Action::OcrOverlay | Action::OcrBoxStep => Category::Ocr,
        }
    }

    pub fn scope(self) -> Scope {
        match self {
            Action::DeleteBookmark | Action::RenameBookmark | Action::BookmarkStep => Scope::Sidebar,
            Action::RenameFile | Action::PageStep | Action::OcrBoxStep => Scope::Viewer,
            _ => Scope::Global,
        }
    }

    /// 바꿀 수 있는가. md에서 `[고정]`으로 표시한 것은 수정 아이콘조차 보이지 않는다.
    pub fn changeable(self) -> bool {
        !matches!(
            self,
            Action::SaveBookmarks
                | Action::FocusSwitch
                | Action::RenameFile
                | Action::PageStep
                | Action::Search
                | Action::DeleteBookmark
                | Action::BookmarkStep
                | Action::Undo
                | Action::Redo
                | Action::OcrOverlay
                | Action::OcrBoxStep
        )
    }

    /// 저장 파일에 적는 이름. **화면에 보이는 이름과 따로 둔다** — 기능 이름을 다듬어도 사용자가
    /// 고쳐 둔 단축키가 날아가지 않는다.
    pub fn id(self) -> &'static str {
        match self {
            Action::SaveBookmarks => "save_bookmarks",
            Action::FocusSwitch => "focus_switch",
            Action::RenameFile => "rename_file",
            Action::PageStep => "page_step",
            Action::HistoryBack => "history_back",
            Action::HistoryForward => "history_forward",
            Action::ZoomIn => "zoom_in",
            Action::ZoomOut => "zoom_out",
            Action::ToggleScrollMode => "toggle_scroll_mode",
            Action::Search => "search",
            Action::AddBookmark => "add_bookmark",
            Action::DeleteBookmark => "delete_bookmark",
            Action::RenameBookmark => "rename_bookmark",
            Action::BookmarkStep => "bookmark_step",
            Action::Undo => "undo",
            Action::Redo => "redo",
            Action::OcrOverlay => "ocr_overlay",
            Action::OcrBoxStep => "ocr_box_step",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Action::SaveBookmarks => "북마크 저장",
            Action::FocusSwitch => "북마크-뷰어 포커스 전환",
            Action::RenameFile => "파일명 변경",
            Action::PageStep => "이전/다음 페이지",
            Action::HistoryBack => "이전 화면",
            Action::HistoryForward => "다음 화면",
            Action::ZoomIn => "확대",
            Action::ZoomOut => "축소",
            Action::ToggleScrollMode => "페이지별 보기 - 연속 보기 전환",
            Action::Search => "내용 검색",
            Action::AddBookmark => "북마크 추가",
            Action::DeleteBookmark => "북마크 삭제",
            Action::RenameBookmark => "북마크 수정",
            Action::BookmarkStep => "이전/다음 북마크",
            Action::Undo => "실행취소",
            Action::Redo => "다시실행",
            Action::OcrOverlay => "OCR 텍스트 보기/숨김",
            Action::OcrBoxStep => "OCR 상자 사이 이동",
        }
    }

    /// 키가 여럿인 것은 글자를 직접 적는다 — `Binding`은 하나만 담는다.
    ///
    /// 그런 기능의 `default_binding`은 **자리만 지키는 값**이다. 실제 발동은 모드까지 보고 가른다
    /// (화살표는 OCR 표시 모드에서 상자를 고른 상태면 상자 이동, 아니면 쪽 이동). 겹침 검사에서는
    /// 그 키를 다른 기능에 주지 못하게 막는 몫만 한다.
    pub fn fixed_display(self) -> Option<&'static str> {
        match self {
            Action::PageStep => Some("←  →"),
            Action::BookmarkStep => Some("↑  ↓"),
            Action::OcrBoxStep => Some("←  ↑  →  ↓"),
            _ => None,
        }
    }

    pub fn default_binding(self) -> Binding {
        use egui::Key;
        let cmd = |key| Binding { command: true, shift: false, alt: false, key };
        let plain = |key| Binding { command: false, shift: false, alt: false, key };
        match self {
            Action::SaveBookmarks => cmd(Key::S),
            Action::FocusSwitch => plain(Key::Tab),
            Action::RenameFile => plain(Key::F2),
            Action::PageStep => plain(Key::ArrowRight),
            Action::HistoryBack => cmd(Key::OpenBracket),
            Action::HistoryForward => cmd(Key::CloseBracket),
            Action::ZoomIn => cmd(Key::Plus),
            Action::ZoomOut => cmd(Key::Minus),
            Action::ToggleScrollMode => plain(Key::C),
            Action::Search => cmd(Key::F),
            Action::AddBookmark => cmd(Key::B),
            Action::DeleteBookmark => plain(Key::Delete),
            Action::RenameBookmark => plain(Key::F2),
            Action::BookmarkStep => plain(Key::ArrowDown),
            Action::Undo => cmd(Key::Z),
            Action::Redo => Binding { command: true, shift: true, alt: false, key: Key::Z },
            Action::OcrOverlay => plain(Key::F1),
            Action::OcrBoxStep => plain(Key::ArrowRight),
        }
    }
}

/// 누른 키 하나와 그때 함께 눌린 보조키.
///
/// `egui::Modifiers`를 그대로 담지 않는 이유: 그쪽은 macOS의 `mac_cmd`와 `ctrl`을 따로 들고 있어
/// 같은 뜻의 조합이 여러 모양으로 표현된다. 여기서는 **`command`(macOS는 ⌘, 그 밖은 Ctrl) 하나로**
/// 모아 둔다 — 사용자가 보는 것도 그 한 가지다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub command: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: egui::Key,
}

impl Binding {
    /// 지금 눌린 것이 이 단축키인가.
    ///
    /// **보조키가 정확히 맞아야 한다.** 예전처럼 `command && key_pressed(Z)`로 느슨하게 보면
    /// `Cmd+Shift+Z`도 `Cmd+Z`로 잡힌다(실제로 그 버그가 있었다).
    pub fn matches(self, modifiers: &egui::Modifiers, key: egui::Key) -> bool {
        // `+`는 자판에서 Shift+`=`이므로, 그 조합일 때만 Shift를 따지지 않는다.
        let shift_free = self.key == egui::Key::Plus && key == egui::Key::Equals;
        self.same_key(key)
            && modifiers.command == self.command
            && (shift_free || modifiers.shift == self.shift)
            && modifiers.alt == self.alt
    }

    /// 같은 뜻으로 들어오는 다른 키까지 받아 준다.
    ///
    /// - `Delete` ← `Backspace`: macOS에서 `delete`라고 적힌 키는 `Backspace`로 들어오고, 앞으로
    ///   지우기(fn+delete)만 `Delete`로 들어온다. 사용자에게는 둘 다 "Delete"다.
    /// - `+` ← `=`: 대부분의 자판에서 `+`는 `=` 키를 Shift와 함께 누른 것이라, 눌린 키는 `Equals`로
    ///   들어온다. 전용 `+` 키(숫자 자판)만 `Plus`다.
    fn same_key(self, key: egui::Key) -> bool {
        use egui::Key;
        key == self.key
            || (self.key == Key::Delete && key == Key::Backspace)
            || (self.key == Key::Plus && key == Key::Equals)
    }

    /// 저장용 글자. **플랫폼과 무관하게** `Mod`로 적는다 — 설정 파일을 다른 OS로 옮겨도 뜻이 같다.
    pub fn to_storage(self) -> String {
        let mut parts = Vec::new();
        if self.command {
            parts.push("Mod");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.alt {
            parts.push("Alt");
        }
        parts.push(self.key.name());
        parts.join("+")
    }

    pub fn parse(text: &str) -> Option<Self> {
        let (mut command, mut shift, mut alt) = (false, false, false);
        let mut key = None;
        for part in text.split('+').map(str::trim).filter(|p| !p.is_empty()) {
            match part {
                "Mod" => command = true,
                "Shift" => shift = true,
                "Alt" => alt = true,
                name => key = egui::Key::from_name(name),
            }
        }
        key.map(|key| Binding { command, shift, alt, key })
    }

    /// 화면용 글자 — macOS면 `Cmd`, 그 밖에서는 `Ctrl`.
    pub fn display(self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.command {
            parts.push(if cfg!(target_os = "macos") { "Cmd" } else { "Ctrl" });
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.alt {
            parts.push(if cfg!(target_os = "macos") { "Opt" } else { "Alt" });
        }
        let name = match self.key {
            egui::Key::OpenBracket => "[",
            egui::Key::CloseBracket => "]",
            other => other.name(),
        };
        parts.push(name);
        parts.join("+")
    }
}

/// 어느 창에서나 뜻이 같아 목록에 올리지도 않는 키 — 다른 기능에 줄 수 없다.
///
/// `Tab`과 화살표는 여기 없다. 목록에 `FocusSwitch`·`PageStep`·`BookmarkStep`·`OcrBoxStep`으로
/// 올라가 있어서 겹침 검사에 저절로 걸린다.
const RESERVED: &[egui::Key] = &[egui::Key::Escape, egui::Key::Enter];

/// 단축키가 겹친 이유.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conflict {
    /// 같은 영역에서 듣는 다른 기능이나, 목록에 없는 구조적인 키가 이미 쓰고 있다.
    App,
    /// 이 기능의 단축키는 바꿀 수 없다(`Action::changeable`).
    Fixed,
}

impl Conflict {
    pub fn message(self) -> &'static str {
        match self {
            Conflict::App => "이 단축키는 앱에서 이미 사용 중입니다.",
            Conflict::Fixed => "이 단축키는 변경할 수 없습니다.",
        }
    }
}

/// 기능마다의 단축키. 고치지 않은 것은 담지 않는다 — 기본값이 바뀌면 그대로 따라간다.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shortcuts {
    changed: BTreeMap<&'static str, Binding>,
}

impl Shortcuts {
    pub fn get(&self, action: Action) -> Binding {
        self.changed.get(action.id()).copied().unwrap_or_else(|| action.default_binding())
    }

    pub fn is_default(&self, action: Action) -> bool {
        !self.changed.contains_key(action.id())
    }

    /// 이 조합을 `action`에 줄 수 있는가. 줄 수 없으면 그 이유.
    ///
    /// **겹침은 같은 포커스 영역 안에서만 따진다.** 사이드바에서만 듣는 키와 뷰어에서만 듣는 키는
    /// 같아도 서로 가로채지 않는다 — `F2`가 그렇게 둘로 나뉘어 있다.
    pub fn conflict(&self, action: Action, binding: Binding) -> Option<Conflict> {
        if !action.changeable() {
            return Some(Conflict::Fixed);
        }
        if RESERVED.contains(&binding.key) && !binding.command && !binding.alt {
            return Some(Conflict::App);
        }
        Action::ALL
            .iter()
            .any(|other| {
                *other != action && action.scope().overlaps(other.scope()) && self.get(*other) == binding
            })
            .then_some(Conflict::App)
    }

    /// 겹치지 않으면 넣는다. 겹치면 그대로 두고 이유를 돌려준다.
    pub fn set(&mut self, action: Action, binding: Binding) -> Result<(), Conflict> {
        if let Some(conflict) = self.conflict(action, binding) {
            return Err(conflict);
        }
        if binding == action.default_binding() {
            self.changed.remove(action.id());
        } else {
            self.changed.insert(action.id(), binding);
        }
        Ok(())
    }

    pub fn reset(&mut self, action: Action) {
        self.changed.remove(action.id());
    }

    pub fn reset_all(&mut self) {
        self.changed.clear();
    }

    /// 지금 이 기능의 단축키가 눌렸는가.
    ///
    /// **포커스 영역까지 여기서 본다.** 겹침 검사와 **같은 `Scope`**를 쓰므로, 설정 창이 "안
    /// 겹친다"고 말한 조합이 실제로는 가로채는 일이 생기지 않는다. 부르는 쪽에서 따로 포커스를
    /// 확인하던 것을 이리로 모았다(2026-10-03 사용자 제안).
    pub fn pressed(&self, ctx: &egui::Context, action: Action, focus: crate::app::FocusArea) -> bool {
        if !action.scope().allows(focus) {
            return false;
        }
        let binding = self.get(action);
        ctx.input(|i| {
            i.events.iter().any(|event| match event {
                egui::Event::Key { key, pressed: true, modifiers, repeat: false, .. } => {
                    binding.matches(modifiers, *key)
                }
                _ => false,
            })
        })
    }

    /// 저장용 — 고친 것만 `기능 id → 단축키` 꼴로.
    pub fn to_storage(&self) -> BTreeMap<String, String> {
        self.changed.iter().map(|(id, binding)| ((*id).to_string(), binding.to_storage())).collect()
    }

    pub fn from_storage(stored: &BTreeMap<String, String>) -> Self {
        let mut shortcuts = Self::default();
        for action in Action::ALL {
            // 읽을 수 없는 값은 조용히 버리고 기본값을 쓴다 — 설정 파일이 상해도 앱은 떠야 한다.
            if let Some(binding) = stored.get(action.id()).and_then(|text| Binding::parse(text)) {
                shortcuts.changed.insert(action.id(), binding);
            }
        }
        shortcuts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Key;

    fn mod_(key: Key) -> Binding {
        Binding { command: true, shift: false, alt: false, key }
    }

    /// **같은 영역에서 듣는** 기본값끼리 겹치면 둘 중 하나는 영영 먹지 않는다. 다른 영역이면
    /// 겹쳐도 된다 — F2가 그렇다(사이드바면 북마크 제목, 그 밖에서는 파일명).
    #[test]
    fn no_two_defaults_collide_within_one_scope() {
        for (index, a) in Action::ALL.iter().enumerate() {
            for b in &Action::ALL[index + 1..] {
                // 키가 여럿인 기능의 기본값은 자리만 지키는 값이라 서로 겹쳐도 된다 — 실제 발동은
                // 모드까지 보고 가른다(`fixed_display` 주석).
                if a.fixed_display().is_some() || b.fixed_display().is_some() {
                    continue;
                }
                if !a.scope().overlaps(b.scope()) {
                    continue;
                }
                assert_ne!(
                    a.default_binding(),
                    b.default_binding(),
                    "{}({})와 {}({})의 기본 단축키가 같다",
                    a.label(),
                    a.scope().label(),
                    b.label(),
                    b.scope().label()
                );
            }
        }
    }

    /// 키가 여럿인 기능은 모두 고정이어야 한다 — 자리만 지키는 값을 사용자가 고치게 두면 안 된다.
    #[test]
    fn placeholder_bindings_are_all_fixed() {
        for action in Action::ALL.iter().filter(|a| a.fixed_display().is_some()) {
            assert!(!action.changeable(), "{}는 키가 여럿인데 고칠 수 있다", action.label());
        }
    }

    /// 포커스가 다르면 같은 키를 나눠 쓸 수 있다. 그것이 F2를 둘로 나눈 이유다.
    #[test]
    fn different_scopes_may_share_a_key() {
        let shortcuts = Shortcuts::default();
        let f2 = Action::RenameBookmark.default_binding();
        assert_eq!(Action::RenameFile.default_binding(), f2);
        assert_eq!(shortcuts.conflict(Action::RenameBookmark, f2), None);

        // 어디서나 듣는 기능은 영역을 가리지 않고 겹친다 — 양쪽 다.
        assert_eq!(
            shortcuts.conflict(Action::AddBookmark, f2),
            Some(Conflict::App),
            "F2를 어디서나 듣는 기능에 주면 겹친다"
        );
        let save = Action::SaveBookmarks.default_binding();
        assert_eq!(shortcuts.conflict(Action::RenameBookmark, save), Some(Conflict::App));
    }

    /// 바꿀 수 없는 기능은 목록에 남되 고쳐지지 않는다.
    #[test]
    fn a_fixed_action_cannot_be_changed() {
        let mut shortcuts = Shortcuts::default();
        assert!(!Action::FocusSwitch.changeable());
        let f7 = Binding { command: false, shift: false, alt: false, key: Key::F7 };
        assert_eq!(shortcuts.conflict(Action::FocusSwitch, f7), Some(Conflict::Fixed));
        assert_eq!(shortcuts.set(Action::FocusSwitch, f7), Err(Conflict::Fixed));
        assert_eq!(shortcuts.get(Action::FocusSwitch), Action::FocusSwitch.default_binding());

        // 다른 기능에 Tab이나 화살표를 주려 해도 막힌다 — 목록에 있으니 겹침 검사에 걸린다.
        let tab = Binding { command: false, shift: false, alt: false, key: Key::Tab };
        assert_eq!(shortcuts.conflict(Action::AddBookmark, tab), Some(Conflict::App));
        let right = Binding { command: false, shift: false, alt: false, key: Key::ArrowRight };
        assert_eq!(shortcuts.conflict(Action::AddBookmark, right), Some(Conflict::App));
    }

    /// 기능 id는 저장 파일의 열쇠다 — 겹치면 서로 덮어쓴다.
    #[test]
    fn every_action_has_its_own_id() {
        let ids: std::collections::BTreeSet<&str> = Action::ALL.iter().map(|a| a.id()).collect();
        assert_eq!(ids.len(), Action::ALL.len());
    }

    /// 저장했다 읽으면 그대로여야 한다.
    #[test]
    fn a_binding_survives_a_round_trip() {
        for action in Action::ALL {
            let binding = action.default_binding();
            assert_eq!(Binding::parse(&binding.to_storage()), Some(binding), "{}", action.label());
        }
        let odd = Binding { command: true, shift: true, alt: true, key: Key::F7 };
        assert_eq!(Binding::parse(&odd.to_storage()), Some(odd));
        assert_eq!(Binding::parse("쓰레기"), None);
    }

    /// 보조키가 정확히 맞아야 한다 — Cmd+Shift+Z가 Cmd+Z로 잡히면 안 된다(예전 버그).
    #[test]
    fn modifiers_must_match_exactly() {
        let undo = mod_(Key::Z);
        let only_command = egui::Modifiers { command: true, ..Default::default() };
        let with_shift = egui::Modifiers { command: true, shift: true, ..Default::default() };
        assert!(undo.matches(&only_command, Key::Z));
        assert!(!undo.matches(&with_shift, Key::Z));
        assert!(!undo.matches(&Default::default(), Key::Z));
    }

    /// macOS에서 `delete`라고 적힌 키는 Backspace로 들어온다 — 둘 다 받아야 한다.
    #[test]
    fn delete_also_answers_to_backspace() {
        let binding = Action::DeleteBookmark.default_binding();
        let none = egui::Modifiers::default();
        assert!(binding.matches(&none, Key::Delete));
        assert!(binding.matches(&none, Key::Backspace));
        // 반대는 아니다 — Backspace로 묶어 둔 단축키가 Delete까지 먹지는 않는다.
        let backspace = Binding { command: false, shift: false, alt: false, key: Key::Backspace };
        assert!(!backspace.matches(&none, Key::Delete));
    }

    /// 겹치는 자리는 막고 이유를 말해 준다.
    #[test]
    fn conflicts_are_reported_with_a_reason() {
        let mut shortcuts = Shortcuts::default();

        // 다른 기능이 쓰고 있다.
        let save = Action::SaveBookmarks.default_binding();
        assert_eq!(shortcuts.conflict(Action::AddBookmark, save), Some(Conflict::App));
        assert_eq!(shortcuts.set(Action::AddBookmark, save), Err(Conflict::App));

        // 목록에 올리지 않은 구조적인 키.
        let esc = Binding { command: false, shift: false, alt: false, key: Key::Escape };
        assert_eq!(shortcuts.conflict(Action::AddBookmark, esc), Some(Conflict::App));

        // 자기 자신과는 겹치지 않는다. 단, 고정된 기능은 그 자체로 막힌다.
        assert_eq!(shortcuts.conflict(Action::SaveBookmarks, save), Some(Conflict::Fixed));
        let add = Action::AddBookmark.default_binding();
        assert_eq!(shortcuts.conflict(Action::AddBookmark, add), None);
    }

    /// 기본값으로 되돌리면 저장할 것이 없다 — 나중에 기본값이 바뀌면 그대로 따라간다.
    #[test]
    fn only_changed_bindings_are_stored() {
        let mut shortcuts = Shortcuts::default();
        assert!(shortcuts.to_storage().is_empty());

        let f7 = Binding { command: false, shift: false, alt: false, key: Key::F7 };
        shortcuts.set(Action::AddBookmark, f7).unwrap();
        assert_eq!(shortcuts.to_storage().len(), 1);
        assert_eq!(shortcuts.get(Action::AddBookmark), f7);
        assert!(!shortcuts.is_default(Action::AddBookmark));

        // 되읽어도 같다.
        let restored = Shortcuts::from_storage(&shortcuts.to_storage());
        assert_eq!(restored, shortcuts);

        shortcuts.set(Action::AddBookmark, Action::AddBookmark.default_binding()).unwrap();
        assert!(shortcuts.to_storage().is_empty());
        assert!(shortcuts.is_default(Action::AddBookmark));
    }
}
