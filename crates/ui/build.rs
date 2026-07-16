fn main() {
    #[cfg(windows)]
    {
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
