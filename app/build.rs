//! Windows: give the .exe the Larrier icon and its version details (shown
//! by Explorer, the Start menu and Settings → Apps). The icon is drawn from
//! the same geometry as the window icon (`src/mark_raster.rs`).

#[allow(dead_code)]
mod mark {
    include!("src/mark_raster.rs");
}

fn main() {
    println!("cargo:rerun-if-changed=src/mark_raster.rs");
    println!("cargo:rerun-if-changed=src/mark_geometry.rs");
    let (commit, date) = build_info();
    println!("cargo:rustc-env=LARRIER_COMMIT={commit}");
    println!("cargo:rustc-env=LARRIER_COMMIT_DATE={date}");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // The browser engine's library loads when the browser is first used,
    // from where its runtime is (the program starts without it).
    println!("cargo:rustc-link-arg=/DELAYLOAD:libcef.dll");
    println!("cargo:rustc-link-lib=delayimp");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let ico = out.join("larrier.ico");
    std::fs::write(&ico, icon(&[16, 20, 24, 32, 40, 48, 64, 128, 256])).expect("write icon");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico.to_str().expect("utf-8 path"));
    res.set("ProductName", "Larrier");
    res.set("FileDescription", "Larrier");
    res.set("CompanyName", "Dev4YM");
    res.set("LegalCopyright", "© Dev4YM");
    res.set("OriginalFilename", "Larrier.exe");
    res.set("ProductVersion", &format!("{} ({commit})", env!("CARGO_PKG_VERSION")));
    // Windows 10/11 as what the program is made for: without it Windows
    // reports an older version, and the browser engine's GPU process fails.
    res.set_manifest(MANIFEST);
    // Without the Windows SDK's resource compiler the exe just has no icon.
    if let Err(e) = res.compile() {
        println!("cargo:warning=no exe icon: {e}");
    }
}

const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
</assembly>
"#;

/// The commit being built and its date (`LARRIER_COMMIT` /
/// `LARRIER_COMMIT_DATE` override them, e.g. for builds outside a checkout).
/// A checkout with uncommitted changes marks the commit `+dirty`.
fn build_info() -> (String, String) {
    for p in ["../.git/HEAD", "../.git/index", "../.git/refs/heads", "../.git/packed-refs"] {
        println!("cargo:rerun-if-changed={p}");
    }
    println!("cargo:rerun-if-env-changed=LARRIER_COMMIT");
    println!("cargo:rerun-if-env-changed=LARRIER_COMMIT_DATE");
    let git = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git").args(args).output().ok()?;
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (out.status.success() && !s.is_empty()).then_some(s)
    };
    let commit = std::env::var("LARRIER_COMMIT").ok().filter(|c| !c.is_empty()).unwrap_or_else(|| {
        let hash = git(&["rev-parse", "--short=9", "HEAD"]).unwrap_or_else(|| "unknown".into());
        let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some();
        if dirty { format!("{hash}+dirty") } else { hash }
    });
    let date = std::env::var("LARRIER_COMMIT_DATE").ok().filter(|c| !c.is_empty()).or_else(|| git(&["log", "-1", "--format=%cs"])).unwrap_or_default();
    (commit, date)
}

/// An .ico holding the mark at each size, as 32-bit bitmaps.
fn icon(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes.iter().map(|&n| bitmap(n)).collect();
    let mut out = Vec::new();
    out.extend(0u16.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend((sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (&n, img) in sizes.iter().zip(&images) {
        let side = if n >= 256 { 0 } else { n as u8 };
        out.extend([side, side, 0, 0]);
        out.extend(1u16.to_le_bytes());
        out.extend(32u16.to_le_bytes());
        out.extend((img.len() as u32).to_le_bytes());
        out.extend(offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in images {
        out.extend(img);
    }
    out
}

/// One icon image: a BITMAPINFOHEADER, BGRA rows bottom-up, then an empty
/// AND mask (the alpha channel does the masking).
fn bitmap(n: u32) -> Vec<u8> {
    let rgba = mark::mark_rgba(n);
    let mask_row = n.div_ceil(32) * 4;
    let mut out = Vec::with_capacity((40 + n * n * 4 + mask_row * n) as usize);
    out.extend(40u32.to_le_bytes());
    out.extend((n as i32).to_le_bytes());
    out.extend((2 * n as i32).to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(32u16.to_le_bytes());
    out.extend([0u8; 24]);
    for y in (0..n).rev() {
        for x in 0..n {
            let i = ((y * n + x) * 4) as usize;
            out.extend([rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
        }
    }
    out.extend(std::iter::repeat(0u8).take((mask_row * n) as usize));
    out
}
