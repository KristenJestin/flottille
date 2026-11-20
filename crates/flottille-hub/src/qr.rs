//! The pair code's QR, as text a console shows: two rows of modules a line, in half blocks. Light
//! modules are drawn and dark ones left blank, as a console's dark background shows them, with a
//! quiet zone of two modules around.

use qrcodegen::{QrCode, QrCodeEcc};

/// The modules of the quiet zone around the symbol.
const QUIET: i32 = 2;

/// The QR of `text`, a line of text for each two rows of modules; `None` when it does not fit one.
pub fn qr(text: &str) -> Option<String> {
    let code = QrCode::encode_text(text, QrCodeEcc::Medium).ok()?;
    let size = code.size();
    // A module outside the symbol is light, the quiet zone's.
    let light = |x: i32, y: i32| {
        !(0..size).contains(&x) || !(0..size).contains(&y) || !code.get_module(x, y)
    };
    let mut out = String::new();
    let mut y = -QUIET;
    while y < size + QUIET {
        for x in -QUIET..size + QUIET {
            out.push(match (light(x, y), light(x, y + 1)) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        out.push('\n');
        y += 2;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_qr_is_square_with_its_quiet_zone() {
        let drawn = qr("0123456789ABCDEFGHJKMNPQRSTVWXYZ").unwrap();
        let lines: Vec<&str> = drawn.lines().collect();
        let width = lines[0].chars().count();
        assert!(lines.iter().all(|line| line.chars().count() == width));
        // Two rows a line: as many lines as half the modules, rounded up.
        assert_eq!(lines.len(), width.div_ceil(2));
        // The quiet zone is light all around.
        assert!(lines[0].chars().all(|c| c == '█'));
    }
}
