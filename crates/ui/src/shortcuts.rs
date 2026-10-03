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
//! **겹침은 포커스 영역(`Scope`)별로 따진다.** 사이드바에서만 듣는 키와 뷰어에서만 듣는 키는 같아도
//! 된다 — `F2`가 그렇다(사이드바면 북마크 제목, 그 밖에서는 파일명).

use std::collections::BTreeMap;

/// 단축키를 붙일 수 있는 기능.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Action {
    AddBookmark,
    DeleteBookmark,
    RenameBookmark,
    RenameFile,
    SaveBookmarks,
    Undo,
    Redo,
    Search,
    HistoryBack,
    HistoryForward,
    ToggleScrollMode,
    OcrOverlay,
    FocusSwitch,
}

/// 그 단축키가 **어느 포커스에서 듣는가**. 겹침을 따질 때 쓴다 — 서로 다른 영역에서만 듣는 둘은
/// 같은 키를 써도 된다.
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

    pub fn label(self) -> &'static str {
        match self {
            Scope::Global => "어디서나",
            Scope::Sidebar => "북마크 목록",
            Scope::Viewer => "뷰어",
        }
    }
}

impl Action {
    pub const ALL: [Action; 13] = [
        Action::AddBookmark,
        Action::DeleteBookmark,
        Action::RenameBookmark,
        Action::RenameFile,
        Action::SaveBookmarks,
        Action::Undo,
        Action::Redo,
        Action::Search,
        Action::HistoryBack,
        Action::HistoryForward,
        Action::ToggleScrollMode,
        Action::OcrOverlay,
        Action::FocusSwitch,
    ];

    /// 어느 포커스에서 듣는가.
    pub fn scope(self) -> Scope {
        match self {
            Action::DeleteBookmark | Action::RenameBookmark => Scope::Sidebar,
            Action::RenameFile => Scope::Viewer,
            _ => Scope::Global,
        }
    }

    /// 바꿀 수 있는가. `Tab`은 영역 전환이라 바꾸면 키보드만으로 앱을 못 돌아다니게 된다.
    pub fn changeable(self) -> bool {
        self != Action::FocusSwitch
    }

    /// 저장 파일에 적는 이름. **화면에 보이는 이름과 따로 둔다** — 기능 이름을 다듬어도 사용자가
    /// 고쳐 둔 단축키가 날아가지 않는다.
    pub fn id(self) -> &'static str {
        match self {
            Action::AddBookmark => "add_bookmark",
            Action::DeleteBookmark => "delete_bookmark",
            Action::RenameBookmark => "rename_bookmark",
            Action::RenameFile => "rename_file",
            Action::SaveBookmarks => "save_bookmarks",
            Action::Undo => "undo",
            Action::Redo => "redo",
            Action::Search => "search",
            Action::HistoryBack => "history_back",
            Action::HistoryForward => "history_forward",
            Action::ToggleScrollMode => "toggle_scroll_mode",
            Action::OcrOverlay => "ocr_overlay",
            Action::FocusSwitch => "focus_switch",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Action::AddBookmark => "북마크 추가",
            Action::DeleteBookmark => "북마크 삭제",
            Action::RenameBookmark => "북마크 제목 수정",
            Action::RenameFile => "파일명 변경",
            Action::SaveBookmarks => "북마크 저장",
            Action::Undo => "실행취소",
            Action::Redo => "다시 실행",
            Action::Search => "내용 검색",
            Action::HistoryBack => "이전 화면",
            Action::HistoryForward => "다음 화면",
            Action::ToggleScrollMode => "쪽 단위 / 연속 스크롤 전환",
            Action::OcrOverlay => "OCR 표시 모드",
            Action::FocusSwitch => "영역 전환",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Action::AddBookmark => "선택한 항목의 하위에, 선택이 없으면 최상위에 넣습니다.",
            Action::DeleteBookmark => "사이드바에서 고른 북마크를 지웁니다.",
            Action::RenameBookmark => "고른 북마크의 제목을 바로 고칩니다.",
            Action::RenameFile => "열려 있는 PDF의 파일명을 바꿉니다.",
            Action::SaveBookmarks => "바뀐 북마크를 PDF에 씁니다.",
            Action::Undo => "북마크 편집을 되돌립니다.",
            Action::Redo => "되돌린 북마크 편집을 다시 합니다.",
            Action::Search => "문서 안의 글자를 찾습니다.",
            Action::HistoryBack => "직전에 보던 자리로 돌아갑니다.",
            Action::HistoryForward => "되돌아오기 전 자리로 다시 갑니다.",
            Action::ToggleScrollMode => "한 쪽씩 보기와 이어서 보기를 오갑니다.",
            Action::OcrOverlay => "보이지 않는 텍스트의 자리와 글자를 덮어 보여 줍니다.",
            Action::FocusSwitch => "북마크 목록과 뷰어를 오갑니다. 검색 결과에서는 직전 영역으로 돌아갑니다.",
        }
    }

    pub fn default_binding(self) -> Binding {
        use egui::Key;
        let mod_ = |key| Binding { command: true, shift: false, alt: false, key };
        let plain = |key| Binding { command: false, shift: false, alt: false, key };
        match self {
            Action::AddBookmark => mod_(Key::B),
            Action::DeleteBookmark => plain(Key::Delete),
            Action::RenameBookmark => plain(Key::F2),
            Action::RenameFile => plain(Key::F2),
            Action::SaveBookmarks => mod_(Key::S),
            Action::Undo => mod_(Key::Z),
            Action::Redo => Binding { command: true, shift: true, alt: false, key: Key::Z },
            Action::Search => mod_(Key::F),
            Action::HistoryBack => mod_(Key::OpenBracket),
            Action::HistoryForward => mod_(Key::CloseBracket),
            Action::ToggleScrollMode => plain(Key::C),
            Action::OcrOverlay => plain(Key::F1),
            Action::FocusSwitch => plain(Key::Tab),
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
        self.same_key(key)
            && modifiers.command == self.command
            && modifiers.shift == self.shift
            && modifiers.alt == self.alt
    }

    /// `Delete`와 `Backspace`를 한 키로 본다. macOS에서 `delete`라고 적힌 키는 `Backspace`로
    /// 들어오고, 앞으로 지우기(fn+delete)만 `Delete`로 들어온다. 사용자에게는 둘 다 "Delete"다.
    fn same_key(self, key: egui::Key) -> bool {
        key == self.key || (self.key == egui::Key::Delete && key == egui::Key::Backspace)
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

/// 어느 창에서나 뜻이 같아 목록에 올리지도 않는 키 — 다른 기능에 줄 수 없다. `Tab`은 여기 없다.
/// 목록에 `Action::FocusSwitch`로 올라가 있어서 겹침 검사에 저절로 걸린다.
const RESERVED: &[egui::Key] = &[
    egui::Key::Escape,
    egui::Key::Enter,
    egui::Key::ArrowUp,
    egui::Key::ArrowDown,
    egui::Key::ArrowLeft,
    egui::Key::ArrowRight,
];

/// OS가 먼저 가로채는 조합. 눌러도 앱에 오지 않으므로 받아 줘 봐야 "안 먹는 단축키"가 된다.
///
/// **손으로 적은 목록이다.** 시스템이 지금 어떤 키를 쓰는지 물어보는 공개 API가 없다 — macOS는
/// `com.apple.symbolichotkeys` 환경설정에 사용자가 바꾼 것까지 들어 있지만 문서화되지 않은
/// 형식이고, Windows는 `RegisterHotKey`로 **잡아 봐야** 알 수 있다(잡으면 그 키를 우리가 먹는
/// 셈이라 확인용으로 쓸 수 없다). 그래서 널리 쓰이는 것만 적어 두고, 빠진 것은 "눌러도 아무 일이
/// 없다"로 남는다(2026-10-03 논의).
fn taken_by_system(binding: Binding) -> bool {
    use egui::Key;
    if cfg!(target_os = "macos") {
        let plain_command = binding.command && !binding.shift && !binding.alt;
        // ⌘Q 끝내기, ⌘W 닫기, ⌘H 숨기기, ⌘M 최소화, ⌘, 환경설정, ⌘Tab 앱 전환,
        // ⌘Space Spotlight, ⌘⌥Esc 강제 종료.
        (plain_command && matches!(binding.key, Key::Q | Key::W | Key::H | Key::M | Key::Comma | Key::Tab | Key::Space))
            || (binding.command && binding.alt && binding.key == Key::Escape)
            || (binding.command && binding.shift && matches!(binding.key, Key::Num3 | Key::Num4 | Key::Num5))
    } else {
        // Alt+Tab(창 전환), Alt+F4(닫기), Ctrl+Shift+Esc(작업 관리자).
        (binding.alt && matches!(binding.key, Key::Tab | Key::F4))
            || (binding.command && binding.shift && binding.key == Key::Escape)
    }
}

/// 단축키가 겹친 이유.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conflict {
    /// 같은 영역에서 듣는 다른 기능이나, 목록에 없는 구조적인 키가 이미 쓰고 있다.
    App,
    /// OS가 먼저 가져간다.
    System,
    /// 이 기능의 단축키는 바꿀 수 없다(`Action::changeable`).
    Fixed,
}

impl Conflict {
    pub fn message(self) -> &'static str {
        match self {
            Conflict::App => "이 단축키는 앱에서 이미 사용 중입니다.",
            Conflict::System => "이 단축키는 시스템에서 이미 사용 중입니다.",
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
        if taken_by_system(binding) {
            return Some(Conflict::System);
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
    pub fn pressed(&self, ctx: &egui::Context, action: Action) -> bool {
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

    /// 포커스가 다르면 같은 키를 나눠 쓸 수 있다. 그것이 F2를 둘로 나눈 이유다.
    #[test]
    fn different_scopes_may_share_a_key() {
        let shortcuts = Shortcuts::default();
        let f2 = Action::RenameBookmark.default_binding();
        assert_eq!(Action::RenameFile.default_binding(), f2);
        assert_eq!(shortcuts.conflict(Action::RenameFile, f2), None);
        assert_eq!(shortcuts.conflict(Action::RenameBookmark, f2), None);

        // 어디서나 듣는 기능은 영역을 가리지 않고 겹친다 — 양쪽 다.
        assert_eq!(shortcuts.conflict(Action::Search, f2), Some(Conflict::App), "F2를 어디서나 듣게 두면 겹친다");
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

        // 다른 기능에 Tab을 주려 해도 막힌다 — 목록에 있으니 겹침 검사에 걸린다.
        let tab = Binding { command: false, shift: false, alt: false, key: Key::Tab };
        assert_eq!(shortcuts.conflict(Action::Search, tab), Some(Conflict::App));
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
        assert_eq!(shortcuts.conflict(Action::Search, save), Some(Conflict::App));
        assert_eq!(shortcuts.set(Action::Search, save), Err(Conflict::App));

        // 목록에 올리지 않은 구조적인 키.
        let esc = Binding { command: false, shift: false, alt: false, key: Key::Escape };
        assert_eq!(shortcuts.conflict(Action::Search, esc), Some(Conflict::App));

        // 자기 자신과는 겹치지 않는다.
        assert_eq!(shortcuts.conflict(Action::SaveBookmarks, save), None);
    }

    /// OS가 가져가는 조합은 받아 봐야 먹지 않는다.
    #[test]
    #[cfg(target_os = "macos")]
    fn the_system_keeps_some_combinations() {
        let shortcuts = Shortcuts::default();
        assert_eq!(shortcuts.conflict(Action::Search, mod_(Key::Q)), Some(Conflict::System));
        assert_eq!(shortcuts.conflict(Action::Search, mod_(Key::W)), Some(Conflict::System));
        // 기본값 중에는 그런 것이 없어야 한다.
        for action in Action::ALL {
            assert!(!super::taken_by_system(action.default_binding()), "{}", action.label());
        }
    }

    /// 기본값으로 되돌리면 저장할 것이 없다 — 나중에 기본값이 바뀌면 그대로 따라간다.
    #[test]
    fn only_changed_bindings_are_stored() {
        let mut shortcuts = Shortcuts::default();
        assert!(shortcuts.to_storage().is_empty());

        let f7 = Binding { command: false, shift: false, alt: false, key: Key::F7 };
        shortcuts.set(Action::Search, f7).unwrap();
        assert_eq!(shortcuts.to_storage().len(), 1);
        assert_eq!(shortcuts.get(Action::Search), f7);
        assert!(!shortcuts.is_default(Action::Search));

        // 되읽어도 같다.
        let restored = Shortcuts::from_storage(&shortcuts.to_storage());
        assert_eq!(restored, shortcuts);

        shortcuts.set(Action::Search, Action::Search.default_binding()).unwrap();
        assert!(shortcuts.to_storage().is_empty());
        assert!(shortcuts.is_default(Action::Search));
    }
}
