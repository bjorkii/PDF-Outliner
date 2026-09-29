//! 파일·폴더 선택 창 — 제목과 확인 버튼 글자를 우리가 정한다.
//!
//! **왜 rfd를 그대로 쓰지 않는가**: rfd 0.15의 macOS 백엔드는 `set_title`을
//! `NSSavePanel.setMessage()`로 연결한다(`backend/macos/file_dialog/panel_ffi.rs:149`). 그래서
//! 우리가 준 문장은 창 안의 한 줄로 붙고, 창 제목은 macOS가 넣는 "Save"/"Open"으로 남는다. 반면
//! Windows 백엔드는 `IFileDialog::SetTitle`을 불러 진짜 창 제목을 바꾼다
//! (`backend/win_cid/file_dialog/dialog_ffi.rs:87`).
//!
//! 그래서 **macOS에서만** 패널을 직접 띄우고(NSSavePanel·NSOpenPanel), 나머지 플랫폼은 rfd에
//! 넘긴다. 두 경로가 같은 [`Dialog`] 인터페이스를 쓰므로 부르는 쪽은 플랫폼을 신경 쓰지 않는다.
//!
//! macOS에서 얻는 것이 하나 더 있다. 확인 버튼 글자(`prompt`)도 정할 수 있어서 "Save" 대신
//! "내보내기"처럼 그 작업의 말로 적을 수 있다. Windows는 OS가 버튼 글자를 정하므로 `prompt`는
//! 조용히 무시된다 — 부르는 쪽은 늘 넣어 두면 된다.
//!
//! 모두 **호출한 스레드를 막는 모달**이다(rfd의 동기 API와 같다). UI 스레드에서만 부른다.

use std::path::PathBuf;

/// 파일·폴더 선택 창 한 번. 만들어서 옵션을 얹고 `pick_*`/`save_file`로 띄운다.
#[derive(Debug, Clone, Default)]
pub struct Dialog {
    title: String,
    /// 확인 버튼에 적을 말(macOS에서만 쓰인다).
    prompt: Option<String>,
    /// (묶음 이름, 확장자들).
    filters: Vec<(String, Vec<String>)>,
    file_name: Option<String>,
}

impl Dialog {
    pub fn new(title: impl Into<String>) -> Self {
        Self { title: title.into(), ..Default::default() }
    }

    /// 확인 버튼 글자. macOS에서만 반영된다(Windows·Linux는 OS가 정한다).
    pub fn prompt(mut self, prompt: impl Into<String>) -> Self {
        self.prompt = Some(prompt.into());
        self
    }

    pub fn filter(mut self, name: &str, extensions: &[&str]) -> Self {
        self.filters.push((name.to_string(), extensions.iter().map(|e| e.to_string()).collect()));
        self
    }

    /// 저장 창에 미리 채워 둘 파일 이름.
    pub fn file_name(mut self, name: impl Into<String>) -> Self {
        self.file_name = Some(name.into());
        self
    }
}

// ─────────────────────────────────────────────────────────────── macOS 외(rfd)

#[cfg(not(target_os = "macos"))]
impl Dialog {
    fn to_rfd(&self) -> rfd::FileDialog {
        let mut dialog = rfd::FileDialog::new().set_title(&self.title);
        for (name, extensions) in &self.filters {
            let extensions: Vec<&str> = extensions.iter().map(String::as_str).collect();
            dialog = dialog.add_filter(name, &extensions);
        }
        if let Some(name) = &self.file_name {
            dialog = dialog.set_file_name(name);
        }
        dialog
    }

    pub fn pick_file(self) -> Option<PathBuf> {
        self.to_rfd().pick_file()
    }

    pub fn pick_files(self) -> Option<Vec<PathBuf>> {
        self.to_rfd().pick_files()
    }

    pub fn pick_folder(self) -> Option<PathBuf> {
        self.to_rfd().pick_folder()
    }

    pub fn save_file(self) -> Option<PathBuf> {
        self.to_rfd().save_file()
    }
}

// ─────────────────────────────────────────────────────────────── macOS(직접)

#[cfg(target_os = "macos")]
mod mac {
    use objc2::rc::Retained;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSModalResponse, NSModalResponseOK, NSOpenPanel, NSSavePanel, NSWindowLevel};
    use objc2_foundation::{NSArray, NSString};
    use std::path::PathBuf;

    /// 다른 창 위에 뜨게 한다. `NSModalPanelWindowLevel`이 이 용도의 값이다(AppKit 문서) —
    /// 이것을 주지 않으면 "항상 위" 창(분리된 검색 결과 창) 아래로 깔릴 수 있다.
    const MODAL_PANEL_LEVEL: NSWindowLevel = 8;

    /// 확장자 묶음을 패널에 건다. `setAllowedFileTypes`는 deprecated지만 대체(UTType) 쪽은
    /// 확장자 문자열을 그대로 받지 못한다 — rfd도 같은 이유로 이것을 쓴다.
    pub fn set_filters(panel: &NSSavePanel, filters: &[(String, Vec<String>)]) {
        let extensions: Vec<Retained<NSString>> = filters
            .iter()
            .flat_map(|(_, exts)| exts.iter())
            .map(|ext| NSString::from_str(ext))
            .collect();
        if extensions.is_empty() {
            return;
        }
        let array = NSArray::from_retained_slice(&extensions);
        #[allow(deprecated)]
        panel.setAllowedFileTypes(Some(&array));
    }

    /// 제목·확인 버튼 글자·기본 파일 이름 — rfd가 못 하는 자리가 여기다.
    pub fn set_labels(panel: &NSSavePanel, title: &str, prompt: Option<&str>, file_name: Option<&str>) {
        // 창 제목(NSSavePanel은 NSWindow를 물려받는다). rfd는 이것을 건드리지 않아 "Save"가
        // 그대로 남았다.
        //
        // `setMessage`는 쓰지 않는다. rfd가 제목 대신 쓰던 자리인데, 제목과 같은 말을 넣었더니
        // 창에 같은 문장이 두 번 나왔다(2026-09-29 화면 확인). 앱 모달(`runModal`)로 띄우는 한
        // 제목 줄이 늘 있으므로 제목 하나로 충분하다.
        panel.setTitle(Some(&NSString::from_str(title)));
        if let Some(prompt) = prompt {
            panel.setPrompt(Some(&NSString::from_str(prompt)));
        }
        if let Some(name) = file_name {
            panel.setNameFieldStringValue(&NSString::from_str(name));
        }
        panel.setLevel(MODAL_PANEL_LEVEL);
    }

    pub fn open_panel(mtm: MainThreadMarker) -> Retained<NSOpenPanel> {
        NSOpenPanel::openPanel(mtm)
    }

    pub fn save_panel(mtm: MainThreadMarker) -> Retained<NSSavePanel> {
        NSSavePanel::savePanel(mtm)
    }

    pub fn ran_ok(response: NSModalResponse) -> bool {
        response == NSModalResponseOK
    }

    /// 고른 경로 하나.
    pub fn url(panel: &NSSavePanel) -> Option<PathBuf> {
        panel.URL().and_then(|url| url.path()).map(|p| PathBuf::from(p.to_string()))
    }

    /// 고른 경로 여럿(열기 패널만).
    pub fn urls(panel: &NSOpenPanel) -> Vec<PathBuf> {
        panel.URLs().iter().filter_map(|url| url.path()).map(|p| PathBuf::from(p.to_string())).collect()
    }
}

#[cfg(target_os = "macos")]
impl Dialog {
    /// 열기 패널 하나를 띄운다. `directories`면 폴더를, 아니면 파일을 고른다.
    fn open(self, directories: bool, multiple: bool) -> Option<Vec<PathBuf>> {
        // UI 스레드가 아니면 패널을 만들 수 없다 — 그럴 일은 없지만 패닉 대신 조용히 포기한다.
        let mtm = objc2::MainThreadMarker::new()?;
        let panel = mac::open_panel(mtm);
        panel.setCanChooseDirectories(directories);
        panel.setCanChooseFiles(!directories);
        panel.setAllowsMultipleSelection(multiple);
        // 폴더를 고르는 창에서는 거기서 바로 새 폴더를 만들 수 있게 둔다(rfd와 같다).
        panel.setCanCreateDirectories(directories);
        if !directories {
            mac::set_filters(&panel, &self.filters);
        }
        mac::set_labels(&panel, &self.title, self.prompt.as_deref(), None);
        if !mac::ran_ok(panel.runModal()) {
            return None;
        }
        let paths = mac::urls(&panel);
        (!paths.is_empty()).then_some(paths)
    }

    pub fn pick_file(self) -> Option<PathBuf> {
        self.open(false, false)?.into_iter().next()
    }

    pub fn pick_files(self) -> Option<Vec<PathBuf>> {
        self.open(false, true)
    }

    pub fn pick_folder(self) -> Option<PathBuf> {
        self.open(true, false)?.into_iter().next()
    }

    pub fn save_file(self) -> Option<PathBuf> {
        let mtm = objc2::MainThreadMarker::new()?;
        let panel = mac::save_panel(mtm);
        panel.setCanCreateDirectories(true);
        mac::set_filters(&panel, &self.filters);
        mac::set_labels(&panel, &self.title, self.prompt.as_deref(), self.file_name.as_deref());
        if !mac::ran_ok(panel.runModal()) {
            return None;
        }
        mac::url(&panel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 옵션이 그대로 담기는지만 확인한다 — 패널을 실제로 띄우는 것은 UI 스레드와 OS가 필요해
    /// 자동 테스트로 확인할 수 없다.
    #[test]
    fn options_are_collected() {
        let dialog = Dialog::new("북마크를 저장할 CSV 파일 지정")
            .prompt("내보내기")
            .filter("CSV", &["csv"])
            .file_name("bookmark-문서.csv");
        assert_eq!(dialog.title, "북마크를 저장할 CSV 파일 지정");
        assert_eq!(dialog.prompt.as_deref(), Some("내보내기"));
        assert_eq!(dialog.filters, vec![("CSV".to_string(), vec!["csv".to_string()])]);
        assert_eq!(dialog.file_name.as_deref(), Some("bookmark-문서.csv"));
    }
}
