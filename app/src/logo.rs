//! The Larrier mark: an "L" whose stem is a dry brush stroke and whose
//! foot is a crisp slab, split by a 45° cut at the corner. It comes in two tiles: ink (dark theme, app icon) and paper
//! (light theme).
//!
//! The SVGs in `assets/brand/` are the source of truth; [`mark_rgba`] draws
//! the same geometry for the window and notification icons, so no image
//! files ship with the app.

/// The mark on an ink tile (64 × 64 view box).
pub const MARK_SVG: &str = include_str!("../assets/brand/larrier-mark.svg");
/// The mark on a paper tile, for the light theme.
pub const MARK_LIGHT_SVG: &str = include_str!("../assets/brand/larrier-mark-light.svg");

include!("mark_raster.rs");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_the_mark() {
        let n = 64;
        let px = mark_rgba(n);
        let at = |x: u32, y: u32| {
            let i = ((y * n + x) * 4) as usize;
            [px[i], px[i + 1], px[i + 2], px[i + 3]]
        };
        assert_eq!(at(0, 0)[3], 0, "rounded corner is transparent");
        assert_eq!(at(32, 4)[3], 255, "tile is opaque");
        assert!(at(25, 25)[0] > 230, "stem is light");
        assert!(at(44, 45)[0] > 230, "foot is light");
        assert!(at(27, 44)[0] < 60, "the cut between them is ink");
        assert!(at(45, 25)[0] < 60, "the open corner is ink");
        assert!(MARK_SVG.contains("viewBox=\"0 0 64 64\"") && MARK_LIGHT_SVG.contains("#141416"));
    }
}
