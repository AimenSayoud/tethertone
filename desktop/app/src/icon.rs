//! The Tethertone mark — rising bars on a dark rounded square — drawn in code,
//! so the window and tray need no image files.

/// RGBA pixels, `size` × `size`.
pub fn rgba(size: u32) -> Vec<u8> {
    const BG: [u8; 4] = [11, 17, 32, 255];
    // (left edge, top edge) as fractions of the size, and colour — as icon.py.
    const BARS: [(f32, f32, [u8; 3]); 5] = [
        (0.235, 0.62, [0xBA, 0xE6, 0xFD]),
        (0.355, 0.50, [0x7D, 0xD3, 0xFC]),
        (0.475, 0.34, [0x38, 0xBD, 0xF8]),
        (0.595, 0.44, [0x0E, 0xA5, 0xE9]),
        (0.715, 0.62, [0xBA, 0xE6, 0xFD]),
    ];
    let s = size as f32;
    let radius = s * 0.22;
    let mut px = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // Rounded-square mask.
            let dx = (radius - fx).max(fx - (s - radius)).max(0.0);
            let dy = (radius - fy).max(fy - (s - radius)).max(0.0);
            if dx * dx + dy * dy > radius * radius {
                continue;
            }
            let mut colour = BG;
            for (left, top, rgb) in BARS {
                let (l, t, w, b) = (s * left, s * top, s * 0.075, s * 0.74);
                if fx >= l && fx < l + w && fy >= t && fy < b {
                    colour = [rgb[0], rgb[1], rgb[2], 255];
                }
            }
            let i = ((y * size + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&colour);
        }
    }
    px
}
