//! First readable system UI font (TTF/OTF): fontconfig on Linux, OS
//! defaults elsewhere. Shared by the settings window and the Flow bar so
//! text matches the rest of the desktop.

pub fn bytes() -> Option<Vec<u8>> {
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(out) = std::process::Command::new("fc-match")
        .args(["-f", "%{file}", "sans-serif"])
        .output()
    {
        if out.status.success() {
            candidates.push(String::from_utf8_lossy(&out.stdout).trim().to_string());
        }
    }
    candidates.extend(
        [
            "/System/Library/Fonts/SFNS.ttf",
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/Library/Fonts/Arial.ttf",
            "C:\\Windows\\Fonts\\segoeui.ttf",
            "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ]
        .map(String::from),
    );
    candidates
        .into_iter()
        .filter(|p| {
            let l = p.to_lowercase();
            l.ends_with(".ttf") || l.ends_with(".otf")
        })
        .find_map(|p| std::fs::read(p).ok())
}
