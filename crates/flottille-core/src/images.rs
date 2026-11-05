//! The pictures a message carries: how many, how big, and the scaling of those that are larger
//! than any provider looks at. The daemon and the window both read it, so there is one answer.

use std::io::Cursor;

use image::{DynamicImage, ImageFormat, ImageReader, codecs::jpeg::JpegEncoder, imageops};

/// Most pictures one message carries. The providers take far more per request (the Claude API
/// 100 or more, the OpenAI API 1500, OpenCode defers to its provider); 20 is also where the
/// Claude API starts to ask every picture to be at most `MAX_SIDE` px on its long side.
pub const MAX_IMAGES: usize = 20;
/// The biggest one picture is, in bytes, once stored.
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
/// The longest side of a picture kept, in pixels: what the Claude API asks of every picture of a
/// request of more than 20, and what OpenCode's own normalisation uses.
pub const MAX_SIDE: u32 = 2000;

/// The picture, scaled down to `MAX_SIDE` px on its long side when it is larger, in the same
/// format (a PNG stays a PNG, a JPEG a JPEG), keeping its proportions. Left as it is when it is
/// small enough, when it is a GIF or a WebP (they may be animated, and the encoder for WebP only
/// writes lossless files, which would weigh more), and when it cannot be read: this is a favour
/// to the byte caps, never a check.
pub fn scaled_down(mime: &str, bytes: Vec<u8>) -> Vec<u8> {
    let format = match mime {
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        _ => return bytes,
    };
    let reader = || ImageReader::with_format(Cursor::new(&bytes), format);
    // The header alone says the size: a picture that fits is never decoded.
    let Ok((width, height)) = reader().into_dimensions() else {
        return bytes;
    };
    if width.max(height) <= MAX_SIDE {
        return bytes;
    }
    let Ok(picture) = reader().decode() else {
        return bytes;
    };
    let small = picture.resize(MAX_SIDE, MAX_SIDE, imageops::FilterType::Lanczos3);
    match encode(&small, format) {
        Some(scaled) => scaled,
        None => bytes,
    }
}

fn encode(picture: &DynamicImage, format: ImageFormat) -> Option<Vec<u8>> {
    let mut out = Cursor::new(Vec::new());
    match format {
        ImageFormat::Jpeg => picture
            .write_with_encoder(JpegEncoder::new_with_quality(&mut out, 90))
            .ok()?,
        _ => picture.write_to(&mut out, format).ok()?,
    }
    Some(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        let rgb = DynamicImage::ImageRgb8(image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x + y) % 239) as u8])
        }));
        rgb.write_to(&mut out, format).unwrap();
        out.into_inner()
    }

    fn size_of(bytes: &[u8]) -> (u32, u32, Option<ImageFormat>) {
        let reader = ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .unwrap();
        let format = reader.format();
        let (w, h) = reader.into_dimensions().unwrap();
        (w, h, format)
    }

    #[test]
    fn a_big_png_is_scaled_to_its_long_side_and_stays_a_png() {
        let scaled = scaled_down("image/png", picture(3000, 1500, ImageFormat::Png));
        assert_eq!(size_of(&scaled), (2000, 1000, Some(ImageFormat::Png)));
    }

    #[test]
    fn a_tall_jpeg_is_scaled_and_stays_a_jpeg() {
        let scaled = scaled_down("image/jpeg", picture(1000, 4000, ImageFormat::Jpeg));
        assert_eq!(size_of(&scaled), (500, 2000, Some(ImageFormat::Jpeg)));
    }

    #[test]
    fn a_picture_that_fits_is_left_byte_for_byte() {
        let png = picture(2000, 1200, ImageFormat::Png);
        assert_eq!(scaled_down("image/png", png.clone()), png);
    }

    #[test]
    fn what_is_not_a_picture_or_not_scaled_is_left_as_it_is() {
        let junk = b"\x89PNG\r\n\x1a\nnot a real picture".to_vec();
        assert_eq!(scaled_down("image/png", junk.clone()), junk);
        assert_eq!(scaled_down("image/gif", junk.clone()), junk);
    }
}
