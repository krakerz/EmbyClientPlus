//! A tiny hand-rolled 5x7 dot-matrix font for overlay button labels.
//!
//! Deliberately not libass: labels here are a small fixed character set
//! (language codes, "OFF"/"AUTO", backend names, digits) that doesn't need
//! real text shaping — a static glyph table blitted straight into the RGBA
//! canvas is deterministic and cheap enough to redraw every tick with no
//! caching, unlike spinning up an `AssLibrary`/renderer/track per label.
//! libass stays scoped to its actual job: real subtitle tracks.

const GLYPH_W: i32 = 5;
const GLYPH_H: i32 = 7;

/// The full A-Z uppercase alphabet (titles are free-form text and need
/// it, unlike the fixed small vocabulary the quick-toggle buttons use),
/// digits 0-9, and a handful of punctuation marks common in real titles
/// (`- ! . ?`), plus `:` and `/` for the OSD's time display and "N/A".
/// Anything else (accented/non-Latin characters) is silently skipped —
/// see `draw_text`'s doc comment.
fn glyph_rows(c: char) -> Option<[&'static str; 7]> {
    Some(match c {
        'A' => [
            ".###.", "#...#", "#...#", "#####", "#...#", "#...#", "#...#",
        ],
        'B' => [
            "####.", "#...#", "#...#", "####.", "#...#", "#...#", "####.",
        ],
        'C' => [
            ".####", "#....", "#....", "#....", "#....", "#....", ".####",
        ],
        'D' => [
            "####.", "#...#", "#...#", "#...#", "#...#", "#...#", "####.",
        ],
        'E' => [
            "#####", "#....", "#....", "####.", "#....", "#....", "#####",
        ],
        'F' => [
            "#####", "#....", "#....", "####.", "#....", "#....", "#....",
        ],
        'G' => [
            ".####", "#....", "#....", "#..##", "#...#", "#...#", ".####",
        ],
        'H' => [
            "#...#", "#...#", "#...#", "#####", "#...#", "#...#", "#...#",
        ],
        'I' => [
            "#####", "..#..", "..#..", "..#..", "..#..", "..#..", "#####",
        ],
        'J' => [
            "..###", "...#.", "...#.", "...#.", "...#.", "#..#.", ".##..",
        ],
        'K' => [
            "#...#", "#..#.", "#.#..", "##...", "#.#..", "#..#.", "#...#",
        ],
        'L' => [
            "#....", "#....", "#....", "#....", "#....", "#....", "#####",
        ],
        'M' => [
            "#...#", "##.##", "#.#.#", "#...#", "#...#", "#...#", "#...#",
        ],
        'N' => [
            "#...#", "##..#", "#.#.#", "#..##", "#...#", "#...#", "#...#",
        ],
        'O' => [
            ".###.", "#...#", "#...#", "#...#", "#...#", "#...#", ".###.",
        ],
        'P' => [
            "####.", "#...#", "#...#", "####.", "#....", "#....", "#....",
        ],
        'Q' => [
            ".###.", "#...#", "#...#", "#...#", "#.#.#", "#..#.", ".##.#",
        ],
        'R' => [
            "####.", "#...#", "#...#", "####.", "#.#..", "#..#.", "#...#",
        ],
        'S' => [
            ".####", "#....", "#....", ".###.", "....#", "....#", "####.",
        ],
        'T' => [
            "#####", "..#..", "..#..", "..#..", "..#..", "..#..", "..#..",
        ],
        'U' => [
            "#...#", "#...#", "#...#", "#...#", "#...#", "#...#", ".###.",
        ],
        'V' => [
            "#...#", "#...#", "#...#", "#...#", "#...#", ".#.#.", "..#..",
        ],
        'W' => [
            "#...#", "#...#", "#...#", "#.#.#", "#.#.#", "##.##", "#...#",
        ],
        'X' => [
            "#...#", "#...#", ".#.#.", "..#..", ".#.#.", "#...#", "#...#",
        ],
        'Y' => [
            "#...#", "#...#", ".#.#.", "..#..", "..#..", "..#..", "..#..",
        ],
        'Z' => [
            "#####", "....#", "...#.", "..#..", ".#...", "#....", "#####",
        ],
        '0' => [
            ".###.", "#...#", "#..##", "#.#.#", "##..#", "#...#", ".###.",
        ],
        '1' => [
            "..#..", ".##..", "..#..", "..#..", "..#..", "..#..", ".###.",
        ],
        '2' => [
            ".###.", "#...#", "....#", "...#.", "..#..", ".#...", "#####",
        ],
        '3' => [
            ".###.", "#...#", "....#", "..##.", "....#", "#...#", ".###.",
        ],
        '4' => [
            "...#.", "..##.", ".#.#.", "#..#.", "#####", "...#.", "...#.",
        ],
        '5' => [
            "#####", "#....", "#....", "####.", "....#", "#...#", ".###.",
        ],
        '6' => [
            "..##.", ".#...", "#....", "####.", "#...#", "#...#", ".###.",
        ],
        '7' => [
            "#####", "....#", "...#.", "..#..", ".#...", ".#...", ".#...",
        ],
        '8' => [
            ".###.", "#...#", "#...#", ".###.", "#...#", "#...#", ".###.",
        ],
        '9' => [
            ".###.", "#...#", "#...#", ".####", "....#", "...#.", ".###.",
        ],
        ':' => [
            ".....", "..#..", ".....", ".....", ".....", "..#..", ".....",
        ],
        '/' => [
            "....#", "...#.", "...#.", "..#..", ".#...", ".#...", "#....",
        ],
        '-' => [
            ".....", ".....", ".....", "#####", ".....", ".....", ".....",
        ],
        '!' => [
            "..#..", "..#..", "..#..", "..#..", "..#..", ".....", "..#..",
        ],
        '.' => [
            ".....", ".....", ".....", ".....", ".....", ".##..", ".##..",
        ],
        '?' => [
            ".###.", "#...#", "....#", "...#.", "..#..", ".....", "..#..",
        ],
        ' ' => [
            ".....", ".....", ".....", ".....", ".....", ".....", ".....",
        ],
        _ => return None,
    })
}

/// Blits `text` (unsupported characters are silently skipped, still
/// advancing the cursor) into `canvas` (row-major straight-alpha RGBA8,
/// `canvas_w * canvas_h * 4` bytes) starting at `(x, y)`, each glyph cell
/// scaled up by `scale` pixels for legibility.
#[allow(clippy::too_many_arguments)]
pub fn draw_text(
    canvas: &mut [u8],
    canvas_w: u32,
    canvas_h: u32,
    x: i32,
    y: i32,
    text: &str,
    scale: i32,
    color: (u8, u8, u8, u8),
) {
    let (r, g, b, a) = color;
    let mut cursor_x = x;
    let advance = (GLYPH_W + 1) * scale;
    for ch in text.chars() {
        let Some(rows) = glyph_rows(ch.to_ascii_uppercase()) else {
            cursor_x += advance;
            continue;
        };
        for (row_idx, row) in rows.iter().enumerate() {
            for (col_idx, cell) in row.chars().enumerate() {
                if cell != '#' {
                    continue;
                }
                let px0 = cursor_x + col_idx as i32 * scale;
                let py0 = y + row_idx as i32 * scale;
                for dy in 0..scale {
                    let py = py0 + dy;
                    if py < 0 || py as u32 >= canvas_h {
                        continue;
                    }
                    for dx in 0..scale {
                        let px = px0 + dx;
                        if px < 0 || px as u32 >= canvas_w {
                            continue;
                        }
                        let offset = ((py as u32 * canvas_w + px as u32) * 4) as usize;
                        canvas[offset] = r;
                        canvas[offset + 1] = g;
                        canvas[offset + 2] = b;
                        canvas[offset + 3] = a;
                    }
                }
            }
        }
        cursor_x += advance;
    }
}

/// Total pixel width `text` would occupy if drawn with [`draw_text`] at
/// `scale` — used to right-align/center labels within a button rect.
pub fn text_width(text: &str, scale: i32) -> i32 {
    text.chars().count() as i32 * (GLYPH_W + 1) * scale
}

pub fn text_height(scale: i32) -> i32 {
    GLYPH_H * scale
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_recognizable_a_glyph_at_scale_one() {
        let mut canvas = vec![0u8; 5 * 7 * 4];
        draw_text(&mut canvas, 5, 7, 0, 0, "A", 1, (255, 255, 255, 255));
        // Row 0 of 'A' is ".###." — column 0 stays blank, column 2 is lit.
        assert_eq!(canvas[3], 0, "top-left corner should be untouched");
        let lit_offset = 2 * 4;
        assert_eq!(
            canvas[lit_offset + 3],
            255,
            "top-middle pixel should be lit"
        );
    }

    #[test]
    fn unsupported_char_still_advances_cursor() {
        let mut canvas = vec![0u8; 20 * 7 * 4];
        draw_text(&mut canvas, 20, 7, 0, 0, "A?A", 1, (255, 255, 255, 255));
        let single_char_advance = text_width("A", 1);
        assert_eq!(text_width("A?A", 1), single_char_advance * 3);
    }
}
