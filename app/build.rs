//! Windows: give the .exe the Larrier icon and its version details (shown
//! by Explorer, the Start menu and Settings → Apps). The icon is drawn from
//! the same geometry as the window icon (`src/mark_raster.rs`).

#[allow(dead_code)]
mod mark {
    include!("src/mark_raster.rs");
}

fn main() {
    println!("cargo:rerun-if-changed=src/mark_raster.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
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
    // Without the Windows SDK's resource compiler the exe just has no icon.
    if let Err(e) = res.compile() {
        println!("cargo:warning=no exe icon: {e}");
    }
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
