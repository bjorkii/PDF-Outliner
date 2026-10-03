//! 사용자가 고칠 수 있는 단축키.
//!
//! 전에는 `app.rs` 곳곳에서 `i.modifiers.command && i.key_pressed(Key::B)` 식으로 키를 직접 봤다.
//! 그러면 고칠 수 없고, 어떤 키가 이미 쓰이는지도 한눈에 알 수 없다. 여기 표 하나로 모아 두고
//! 설정 창의 `단축키` 탭이 이 표를 그린다.
//!
//! **구조적인 키는 넣지 않는다.** `Tab`(영역 전환), `Esc`(닫기), `Enter`(확정), 화살표(목록 이동)는
//! 뜻이 고정돼 있고, 바꾸게 두면 앱이 잠길 수 있다. 다만 **다른 기능에 그 키를 주려 할 때는 막아야**
//! 하므로 [`RESERVED`]에 적어 둔다.

use std::collections::BTreeMap;

/// 단축키를 붙일 수 있는 기능.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Action {
    AddBookmark,
    DeleteBookmark,
    Rename,
    SaveBookmarks,
    Undo,
    Redo,
    Search,
    HistoryBack,
    HistoryForward,
    ToggleScrollMode,
    OcrOverlay,
}

impl Action {
    pub const ALL: [Action; 11] = [
        Action::AddBookmark,
        Action::DeleteBookmark,
        Action::Rename,
        Action::SaveBookmarks,
        Action::Undo,
        Action::Redo,
        Action::Search,
        Action::HistoryBack,
        Action::HistoryForward,
        Action::ToggleScrollMode,
        Action::OcrOverlay,
    ];

    /// 저장 파일에 적는 이름. **화면에 보이는 이름과 따로 둔다** — 기능 이름을 다듬어도 사용자가
    /// 고쳐 둔 단축키가 날아가지 않는다.
    pub fn id(self) -> &'static str {
        match self {
            Action::AddBookmark => "add_bookmark",
            Action::DeleteBookmark => "delete_bookmark",
            Action::Rename => "rename",
            Action::SaveBookmarks => "save_bookmarks",
            Action::Undo => "undo",
            Action::Redo => "redo",
            Action::Search => "search",
            Action::HistoryBack => "history_back",
            Action::HistoryForward => "history_forward",
            Action::ToggleScrollMode => "toggle_scroll_mode",
            Action::OcrOverlay => "ocr_overlay",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Action::AddBookmark => "북마크 추가",
            Action::DeleteBookmark => "북마크 삭제",
            Action::Rename => "북마크 수정 / 파일명 변경",
            Action::SaveBookmarks => "북마크 저장",
            Action::Undo => "실행취소",
            Action::Redo => "다시 실행",
            Action::Search => "내용 검색",
            Action::HistoryBack => "이전 화면",
            Action::HistoryForward => "다음 화면",
            Action::ToggleScrollMode => "쪽 단위 / 연속 스크롤 전환",
            Action::OcrOverlay => "OCR 표시 모드",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Action::AddBookmark => "선택한 항목의 하위에, 선택이 없으면 최상위에 넣습니다.",
            Action::DeleteBookmark => "사이드바에서 고른 북마크를 지웁니다.",
            Action::Rename => "사이드바가 포커스면 북마크 제목을, 그 밖에서는 파일명을 바꿉니다.",
            Action::SaveBookmarks => "바뀐 북마크를 PDF에 씁니다.",
            Action::Undo => "북마크 편집을 되돌립니다.",
            Action::Redo => "되돌린 북마크 편집을 다시 합니다.",
            Action::Search => "문서 안의 글자를 찾습니다.",
            Action::HistoryBack => "직전에 보던 자리로 돌아갑니다.",
            Action::HistoryForward => "되돌아오기 전 자리로 다시 갑니다.",
            Action::ToggleScrollMode => "한 쪽씩 보기와 이어서 보기를 오갑니다.",
            Action::OcrOverlay => "보이지 않는 텍스트의 자리와 글자를 덮어 보여 줍니다.",
        }
    }

    pub fn default_binding(self) -> Binding {
        use egui::Key;
        let mod_ = |key| Binding { command: true, shift: false, alt: false, key };
        let plain = |key| Binding { command: false, shift: false, alt: false, key };
        match self {
            Action::AddBookmark => mod_(Key::B),
            Action::DeleteBookmark => plain(Key::Delete),
            Action::Rename => plain(Key::F2),
            Action::SaveBookmarks => mod_(Key::S),
            Action::Undo => mod_(Key::Z),
            Action::Redo => Binding { command: true, shift: true, alt: false, key: Key::Z },
            Action::Search => mod_(Key::F),
            Action::HistoryBack => mod_(Key::OpenBracket),
            Action::HistoryForward => mod_(Key::CloseBracket),
            Action::ToggleScrollMode => plain(Key::C),
            Action::OcrOverlay => plain(Key::F1),
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
            parts.push(if cfg!(target_os = "macos") { "Option" } else { "Alt" });
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

/// 앱이 구조적으로 쓰는 키 — 고칠 수 없고, 다른 기능에 줄 수도 없다.
const RESERVED: &[egui::Key] = &[
    egui::Key::Tab,
    egui::Key::Escape,
    egui::Key::Enter,
    egui::Key::ArrowUp,
    egui::Key::ArrowDown,
    egui::Key::ArrowLeft,
    egui::Key::ArrowRight,
];

/// OS가 먼저 가로채는 조합. 눌러도 앱에 오지 않으므로 받아 줘 봐야 "안 먹는 단축키"가 된다.
fn taken_by_system(binding: Binding) -> bool {
    use egui::Key;
    if cfg!(target_os = "macos") {
        binding.command
            && !binding.shift
            && !binding.alt
            && matches!(binding.key, Key::Q | Key::W | Key::H | Key::M | Key::Tab | Key::Space | Key::Comma)
    } else {
        // Windows·Linux에서 Alt+Tab, Alt+F4는 창 관리자가 가져간다.
        binding.alt && matches!(binding.key, Key::Tab | Key::F4)
    }
}

/// 단축키가 겹친 이유.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conflict {
    /// 앱의 다른 기능이나 구조적인 키가 이미 쓰고 있다.
    App,
    /// OS가 먼저 가져간다.
    System,
}

impl Conflict {
    pub fn message(self) -> &'static str {
        match self {
            Conflict::App => "이 단축키는 앱에서 이미 사용 중입니다.",
            Conflict::System => "이 단축키는 시스템에서 이미 사용 중입니다.",
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
    pub fn conflict(&self, action: Action, binding: Binding) -> Option<Conflict> {
        if taken_by_system(binding) {
            return Some(Conflict::System);
        }
        if RESERVED.contains(&binding.key) && !binding.command && !binding.alt {
            return Some(Conflict::App);
        }
        Action::ALL
            .iter()
            .any(|other| *other != action && self.get(*other) == binding)
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

    /// 기본값끼리 겹치면 둘 중 하나는 영영 먹지 않는다.
    #[test]
    fn no_two_defaults_collide() {
        for (index, a) in Action::ALL.iter().enumerate() {
            for b in &Action::ALL[index + 1..] {
                assert_ne!(
                    a.default_binding(),
                    b.default_binding(),
                    "{}와 {}의 기본 단축키가 같다",
                    a.label(),
                    b.label()
                );
            }
        }
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

        // 구조적인 키.
        let tab = Binding { command: false, shift: false, alt: false, key: Key::Tab };
        assert_eq!(shortcuts.conflict(Action::Search, tab), Some(Conflict::App));

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
