//! The pairing QR code, as a module matrix for the window and as text for a terminal.

use qrcode::{EcLevel, QrCode};

/// Square matrix, `true` = dark module, without the quiet zone.
pub fn matrix(data: &str) -> Option<(usize, Vec<bool>)> {
    let code = QrCode::with_error_correction_level(data.as_bytes(), EcLevel::M).ok()?;
    let width = code.width();
    let modules = code.to_colors().into_iter().map(|c| c == qrcode::Color::Dark).collect();
    Some((width, modules))
}

/// Two rows per text line with half blocks, plus a quiet zone; scannable from a terminal.
pub fn terminal(data: &str) -> String {
    let Some((width, modules)) = matrix(data) else { return String::new() };
    let dark = |x: isize, y: isize| {
        x >= 0 && y >= 0 && (x as usize) < width && (y as usize) < width && modules[y as usize * width + x as usize]
    };
    let border = 2isize;
    let mut out = String::new();
    let mut y = -border;
    while y < width as isize + border {
        for x in -border..width as isize + border {
            // Light-on-dark terminals: draw *light* modules, so the code reads correctly.
            out.push(match (!dark(x, y), !dark(x, y + 1)) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        out.push('\n');
        y += 2;
    }
    out
}
