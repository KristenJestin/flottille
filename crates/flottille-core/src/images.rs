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
    let Some(format) = scaled_format(mime) else {
        return bytes;
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

/// Whether `scaled_down` would decode and resize this picture. Only its header is read, so the
/// answer is cheap: a caller that must not wait for the scaling asks first.
pub fn needs_scaling(mime: &str, bytes: &[u8]) -> bool {
    let Some(format) = scaled_format(mime) else {
        return false;
    };
    ImageReader::with_format(Cursor::new(bytes), format)
        .into_dimensions()
        .is_ok_and(|(width, height)| width.max(height) > MAX_SIDE)
}

/// The size in pixels a picture (png, jpeg, gif or webp) declares, read from its header alone:
/// what the window needs to fit it large without decoding it.
pub fn dimensions(mime: &str, bytes: &[u8]) -> Option<(u32, u32)> {
    let format = match mime {
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::WebP,
        _ => return None,
    };
    ImageReader::with_format(Cursor::new(bytes), format)
        .into_dimensions()
        .ok()
}

/// Why a picture is not made a thumbnail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Its type is none a thumbnail is made of, or its bytes do not begin as that type.
    NotAPicture,
    /// It declares more pixels than a thumbnail is worth the memory of: said by its header,
    /// before anything is decoded.
    TooLarge,
    /// Its header or its data cannot be read.
    Unreadable,
}

/// The most memory decoding a picture for a thumbnail may take, in bytes.
const MAX_DECODE_BYTES: u64 = 64 * 1024 * 1024;

/// The thumbnail of a picture (png, jpeg, gif or webp), as PNG bytes of exactly `fit` (width,
/// height), its middle kept. The size it declares is read first, from its header alone, and a
/// picture over `MAX_SIDE` on a side is refused before a byte is decoded (a small file can
/// declare a picture of gigabytes); what is decoded is limited in memory too, and is the first
/// frame of a GIF, never the thousands it may hold.
pub fn thumbnail(mime: &str, bytes: &[u8], fit: (u32, u32)) -> Result<Vec<u8>, Refused> {
    let format = match mime {
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::WebP,
        _ => return Err(Refused::NotAPicture),
    };
    if !is_picture(mime, bytes) {
        return Err(Refused::NotAPicture);
    }
    let reader = || ImageReader::with_format(Cursor::new(bytes), format);
    let (width, height) = reader()
        .into_dimensions()
        .map_err(|_| Refused::Unreadable)?;
    if width.max(height) > MAX_SIDE {
        return Err(Refused::TooLarge);
    }
    let mut reader = reader();
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let picture = reader.decode().map_err(|_| Refused::Unreadable)?;
    let small = picture.resize_to_fill(fit.0, fit.1, imageops::FilterType::Triangle);
    let mut out = Cursor::new(Vec::new());
    small
        .write_to(&mut out, ImageFormat::Png)
        .map_err(|_| Refused::Unreadable)?;
    Ok(out.into_inner())
}

/// Whether these bytes begin as a picture of that type (png, jpeg, gif or webp): the signature
/// alone, since a file is named by an extension its writer chose.
pub fn is_picture(mime: &str, bytes: &[u8]) -> bool {
    match mime {
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => bytes.starts_with(&[0xFF, 0xD8, 0xFF]),
        "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "image/webp" => bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        _ => false,
    }
}

fn scaled_format(mime: &str) -> Option<ImageFormat> {
    match mime {
        "image/png" => Some(ImageFormat::Png),
        "image/jpeg" => Some(ImageFormat::Jpeg),
        _ => None,
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

    #[test]
    fn a_picture_is_told_by_its_signature() {
        let png = picture(4, 4, ImageFormat::Png);
        let jpeg = picture(4, 4, ImageFormat::Jpeg);
        assert!(is_picture("image/png", &png));
        assert!(is_picture("image/jpeg", &jpeg));
        assert!(is_picture("image/gif", b"GIF89a\x01\x00"));
        assert!(is_picture("image/webp", b"RIFF\x00\x00\x00\x00WEBPVP8 "));
        assert!(!is_picture("image/png", &jpeg));
        assert!(!is_picture("image/png", b"<p>not a picture</p>"));
        assert!(!is_picture("image/svg+xml", b"<svg/>"));
    }

    /// A PNG of `width` by `height` that is only its header and a stub of data: the data a decoder would need is not
    /// there, so a refusal that comes from anything but the header would be another error.
    fn png_declaring(width: u32, height: u32) -> Vec<u8> {
        let chunk = |kind: &[u8; 4], data: &[u8]| {
            let mut out = (data.len() as u32).to_be_bytes().to_vec();
            let mut body = kind.to_vec();
            body.extend_from_slice(data);
            let mut crc = 0xFFFF_FFFFu32;
            for byte in &body {
                crc ^= u32::from(*byte);
                for _ in 0..8 {
                    crc = if crc & 1 == 1 {
                        (crc >> 1) ^ 0xEDB8_8320
                    } else {
                        crc >> 1
                    };
                }
            }
            out.extend_from_slice(&body);
            out.extend_from_slice(&(!crc).to_be_bytes());
            out
        };
        let mut header = width.to_be_bytes().to_vec();
        header.extend_from_slice(&height.to_be_bytes());
        header.extend_from_slice(&[8, 6, 0, 0, 0]);
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend(chunk(b"IHDR", &header));
        png.extend(chunk(b"IDAT", &[0]));
        png.extend(chunk(b"IEND", &[]));
        png
    }

    #[test]
    fn a_small_file_declaring_a_huge_picture_is_refused_before_any_decoding() {
        // 11000 x 11000 pixels is 484 MB decoded; the bytes are a header and nothing else, so
        // a decoder would fail on its data: only the header's size can refuse it as too large.
        assert_eq!(
            thumbnail("image/png", &png_declaring(11_000, 11_000), (128, 88)),
            Err(Refused::TooLarge)
        );
        assert_eq!(
            thumbnail("image/png", &png_declaring(1, MAX_SIDE + 1), (128, 88)),
            Err(Refused::TooLarge)
        );
        // At the limit, it is the data that is missing.
        assert_eq!(
            thumbnail("image/png", &png_declaring(MAX_SIDE, MAX_SIDE), (128, 88)),
            Err(Refused::Unreadable)
        );
    }

    #[test]
    fn a_thumbnail_is_the_size_asked_and_a_gif_gives_its_first_frame() {
        let png = thumbnail("image/png", &picture(300, 100, ImageFormat::Png), (128, 88)).unwrap();
        assert_eq!(size_of(&png).0, 128);
        let mut gif = Cursor::new(Vec::new());
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut gif);
            for shade in [10u8, 200] {
                let frame = image::Frame::new(image::RgbaImage::from_pixel(
                    40,
                    30,
                    image::Rgba([shade, 0, 0, 255]),
                ));
                encoder.encode_frame(frame).unwrap();
            }
        }
        let first = thumbnail("image/gif", &gif.into_inner(), (16, 12)).unwrap();
        let decoded = image::load_from_memory(&first).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (16, 12));
        assert!(
            decoded.get_pixel(8, 6)[0] < 100,
            "the first frame, not the last"
        );
    }

    #[test]
    fn what_is_no_picture_is_refused() {
        assert_eq!(
            thumbnail("image/png", b"<p>text</p>", (8, 8)),
            Err(Refused::NotAPicture)
        );
        assert_eq!(
            thumbnail("image/svg+xml", b"<svg/>", (8, 8)),
            Err(Refused::NotAPicture)
        );
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
    fn only_a_picture_that_is_scaled_is_said_to_need_it() {
        assert!(needs_scaling(
            "image/png",
            &picture(3000, 1500, ImageFormat::Png)
        ));
        assert!(!needs_scaling(
            "image/png",
            &picture(2000, 1200, ImageFormat::Png)
        ));
        assert!(!needs_scaling("image/gif", b"GIF89a"));
        assert!(!needs_scaling("image/png", b"not a picture"));
    }

    #[test]
    fn what_is_not_a_picture_or_not_scaled_is_left_as_it_is() {
        let junk = b"\x89PNG\r\n\x1a\nnot a real picture".to_vec();
        assert_eq!(scaled_down("image/png", junk.clone()), junk);
        assert_eq!(scaled_down("image/gif", junk.clone()), junk);
    }
}
