use std::path::Path;
use std::process::Command;

fn main() {
    embed_version();

    #[cfg(windows)]
    {
        // rerun-if-changed를 하나라도 쓰면 cargo는 그 목록만 보고 다시 실행하므로 아이콘도 명시.
        println!("cargo:rerun-if-changed=../../assets/icon/icon.ico");
        let mut res = winres::WindowsResource::new();
        res.set_icon("../../assets/icon/icon.ico");
        // winres가 명시적으로 안 주면 크레이트 이름(Cargo.toml package.name = "ui")으로
        // ProductName/FileDescription을 채워서, Windows "연결 프로그램" 지정 시 앱 이름이
        // "ui"로 표시되는 버그가 있었음(2026-07-16 사용자 리포트) — 바이너리 이름
        // (PDF-Outliner)이나 macOS 번들 표시 이름(CFBundleName)과도 어긋났음.
        res.set("ProductName", "PDF Outliner");
        res.set("FileDescription", "PDF Outliner");
        res.set("InternalName", "PDF-Outliner");
        res.set("OriginalFilename", "PDF-Outliner.exe");
        res.compile().expect("failed to embed Windows icon resource");
    }
}

/// 창 제목에 표시할 앱 버전(`PDF_OUTLINER_VERSION`)을 컴파일 시점에 넣는다(2026-09-15 요청 —
/// "PDF Outliner v0.2.1 - 문서.pdf"). git 태그가 버전의 유일한 기준이라(pdf_viewer_spec.md §5,
/// Cargo.toml version은 릴리스마다 올리지 않음):
/// 1. 배포 빌드 — 패키징 스크립트가 넘기는 `PDF_OUTLINER_VERSION`(CI의 태그명, 예: v0.2.1)
/// 2. 로컬 빌드 — `git describe --tags --always`(태그 뒤 커밋이 있으면 예: v0.2.1-3-gabc1234)
/// 3. 둘 다 없으면 "dev"
fn embed_version() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=PDF_OUTLINER_VERSION");
    // 커밋·태그가 바뀌면 다시 계산한다. 없는 경로를 등록하면 cargo가 매번 다시 실행해 ui
    // 크레이트 전체를 재컴파일하므로 실제로 있는 것만 등록.
    for path in [
        "../../.git/HEAD",
        "../../.git/refs/heads",
        "../../.git/refs/tags",
        "../../.git/packed-refs",
    ] {
        if Path::new(path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }

    let version = std::env::var("PDF_OUTLINER_VERSION")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(git_describe)
        .unwrap_or_else(|| "dev".to_string());
    println!("cargo:rustc-env=PDF_OUTLINER_VERSION={version}");
}

fn git_describe() -> Option<String> {
    let output = Command::new("git")
        .args(["describe", "--tags", "--always"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!text.is_empty()).then_some(text)
}
