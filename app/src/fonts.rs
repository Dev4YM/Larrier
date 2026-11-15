//! Inter, built into Larrier, so the interface reads the same on every
//! system and stays crisp at small sizes. It works offline. It's the variable
//! font (weight and optical size) for Latin, Latin Extended, Cyrillic and
//! Greek; other scripts (Arabic…) use the system's fonts.
//!
//! Inter is © The Inter Project Authors, under the SIL Open Font License 1.1
//! (`assets/fonts/inter/LICENSE.txt`); the files are Fontsource's
//! `@fontsource-variable/inter` 5.3.0 subsets.

use base64::Engine;

/// Each subset and the characters it covers (Fontsource's ranges).
const FACES: &[(&[u8], &str)] = &[
    (
        include_bytes!("../assets/fonts/inter/inter-latin.woff2"),
        "U+0000-00FF,U+0131,U+0152-0153,U+02BB-02BC,U+02C6,U+02DA,U+02DC,U+0304,U+0308,U+0329,U+2000-206F,U+20AC,U+2122,U+2191,U+2193,U+2212,U+2215,U+FEFF,U+FFFD",
    ),
    (
        include_bytes!("../assets/fonts/inter/inter-latin-ext.woff2"),
        "U+0100-02BA,U+02BD-02C5,U+02C7-02CC,U+02CE-02D7,U+02DD-02FF,U+0304,U+0308,U+0329,U+1D00-1DBF,U+1E00-1E9F,U+1EF2-1EFF,U+2020,U+20A0-20AB,U+20AD-20C0,U+2113,U+2C60-2C7F,U+A720-A7FF",
    ),
    (include_bytes!("../assets/fonts/inter/inter-cyrillic.woff2"), "U+0301,U+0400-045F,U+0490-0491,U+04B0-04B1,U+2116"),
    (include_bytes!("../assets/fonts/inter/inter-greek.woff2"), "U+0370-0377,U+037A-037F,U+0384-038A,U+038C,U+038E-03A1,U+03A3-03FF"),
];

/// `@font-face` rules for the built-in Inter. They're data URLs: there's no
/// file to fetch, so the first frame is already drawn in Inter.
pub fn faces_css() -> String {
    let b64 = base64::engine::general_purpose::STANDARD;
    FACES
        .iter()
        .map(|(bytes, range)| {
            format!(
                "@font-face{{font-family:'Inter';font-style:normal;font-weight:100 900;font-display:block;src:url(data:font/woff2;base64,{}) format('woff2');unicode-range:{range};}}",
                b64.encode(bytes)
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_subset_is_a_woff2() {
        for (bytes, range) in super::FACES {
            assert_eq!(&bytes[..4], b"wOF2", "{range}");
        }
        assert_eq!(super::faces_css().matches("@font-face").count(), super::FACES.len());
    }
}
