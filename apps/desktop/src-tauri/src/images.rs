use anyhow::{Context, Result, anyhow, ensure};
use image::{
    AnimationDecoder, DynamicImage, Frames, ImageDecoder, ImageEncoder, ImageError, ImageFormat,
    ImageReader, Limits,
    codecs::{
        gif::GifDecoder,
        png::{PngDecoder, PngEncoder},
        webp::WebPDecoder,
    },
};
use std::io::Cursor;
use zibblu_core::{MAX_PASTED_IMAGE_BYTES, MAX_PROFILE_IMAGE_BYTES};

const MAX_EDGE: u32 = 8192;
const MAX_PIXELS: u64 = 16_000_000;
const MAX_ANIMATION_PIXELS: u64 = 64_000_000;
const MAX_FRAMES: u64 = 240;
const MAX_DECODER_BYTES: u64 = 128 * 1024 * 1024;
const DAMAGED: &str = "This image is damaged or could not be decoded.";
const TOO_MANY_FRAMES: &str = "This animation has too many frames for its image size.";

pub(crate) const FORMATS: &str = "PNG, JPEG, WebP, GIF or BMP";

fn limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_EDGE);
    limits.max_image_height = Some(MAX_EDGE);
    limits.max_alloc = Some(MAX_DECODER_BYTES);
    limits
}

fn check_dimensions(width: u32, height: u32) -> Result<u64> {
    let pixels = u64::from(width) * u64::from(height);
    ensure!(
        width > 0 && height > 0 && width <= MAX_EDGE && height <= MAX_EDGE && pixels <= MAX_PIXELS,
        "Images can be up to 8192 pixels per side and 16 megapixels in total."
    );
    Ok(pixels)
}

fn check_decoder(decoder: &mut impl ImageDecoder) -> Result<u64> {
    let (width, height) = decoder.dimensions();
    let pixels = check_dimensions(width, height)?;
    ensure!(
        decoder.total_bytes() <= MAX_DECODER_BYTES,
        "This image needs too much memory to decode."
    );
    decoder.set_limits(limits())?;
    Ok(pixels)
}

fn frame_limit(pixels_per_frame: u64) -> u64 {
    MAX_FRAMES.min(MAX_ANIMATION_PIXELS / pixels_per_frame)
}

fn animation(mut frames: Frames<'_>, pixels_per_frame: u64) -> Result<DynamicImage> {
    let max_frames = frame_limit(pixels_per_frame);
    let first = frames
        .next()
        .context("This animation has no image frames.")??
        .into_buffer();
    for (index, frame) in frames.enumerate() {
        ensure!(index as u64 + 2 <= max_frames, "{TOO_MANY_FRAMES}");
        frame?;
    }
    Ok(first.into())
}

fn check_gif(bytes: &[u8]) -> Result<()> {
    // The compositor's GIF decoder permits frame rectangles outside the logical canvas.
    let mut options = gif::DecodeOptions::new();
    options.check_frame_consistency(true);
    options.skip_frame_decoding(true);
    let mut decoder = options.read_info(Cursor::new(bytes)).context(DAMAGED)?;
    let pixels = check_dimensions(decoder.width().into(), decoder.height().into())?;
    let mut count = 0;
    while decoder.next_frame_info().context(DAMAGED)?.is_some() {
        count += 1;
        ensure!(count <= frame_limit(pixels), "{TOO_MANY_FRAMES}");
    }
    Ok(())
}

fn riff_chunk(bytes: &[u8]) -> Result<(&[u8; 4], &[u8], &[u8])> {
    let (header, body) = bytes.split_at_checked(8).context(DAMAGED)?;
    let size = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
    let (data, remaining) = body.split_at_checked(size).context(DAMAGED)?;
    let remaining = remaining.get(size % 2..).context(DAMAGED)?;
    Ok((header.first_chunk().unwrap(), data, remaining))
}

fn webp_dimensions(bytes: &[u8]) -> Result<(u32, u32)> {
    let [a, b, c, d, e, f] = *bytes.first_chunk::<6>().context(DAMAGED)?;
    Ok((
        u32::from_le_bytes([a, b, c, 0]) + 1,
        u32::from_le_bytes([d, e, f, 0]) + 1,
    ))
}

fn check_vp8(bytes: &[u8], expected: Option<(u32, u32)>) -> Result<()> {
    let header = bytes.get(6..10).context(DAMAGED)?;
    let width = u32::from(u16::from_le_bytes([header[0], header[1]]) & 0x3fff);
    let height = u32::from(u16::from_le_bytes([header[2], header[3]]) & 0x3fff);
    check_dimensions(width, height)?;
    ensure!(
        expected.is_none_or(|size| size == (width, height)),
        "{DAMAGED}"
    );
    Ok(())
}

fn check_webp(bytes: &[u8]) -> Result<()> {
    // Bound RIFF chunks and VP8 dimensions before the codec allocates for metadata or pixels.
    let (riff, mut chunks) = bytes.split_at_checked(12).context(DAMAGED)?;
    let size = u64::from(u32::from_le_bytes(riff[4..8].try_into().unwrap()));
    ensure!(size + 8 == bytes.len() as u64, "{DAMAGED}");
    let mut canvas = None;
    let mut remaining_frames = 0;
    while !chunks.is_empty() {
        let (tag, data, rest) = riff_chunk(chunks)?;
        match tag {
            b"VP8X" => {
                ensure!(
                    chunks.len() == bytes.len() - 12 && data.len() == 10,
                    "{DAMAGED}"
                );
                let (width, height) = webp_dimensions(&data[4..])?;
                remaining_frames = frame_limit(check_dimensions(width, height)?);
                canvas = Some((width, height));
            }
            b"VP8 " => check_vp8(data, canvas)?,
            b"ANMF" => {
                ensure!(remaining_frames > 0, "{TOO_MANY_FRAMES}");
                remaining_frames -= 1;
                let (header, frame) = data.split_at_checked(16).context(DAMAGED)?;
                let size = webp_dimensions(&header[6..])?;
                check_dimensions(size.0, size.1)?;
                let (mut tag, mut data, mut tail) = riff_chunk(frame)?;
                if tag == b"ALPH" {
                    (tag, data, tail) = riff_chunk(tail)?;
                    ensure!(tag == b"VP8 ", "{DAMAGED}");
                }
                match tag {
                    b"VP8 " => check_vp8(data, Some(size))?,
                    b"VP8L" => {}
                    _ => anyhow::bail!("{DAMAGED}"),
                }
                while !tail.is_empty() {
                    (_, _, tail) = riff_chunk(tail)?;
                }
            }
            _ => {}
        }
        chunks = rest;
    }
    Ok(())
}

fn still(mut decoder: impl ImageDecoder) -> Result<DynamicImage> {
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    Ok(image)
}

fn decode(format: ImageFormat, bytes: &[u8]) -> Result<DynamicImage> {
    let source = Cursor::new(bytes);
    match format {
        ImageFormat::Png => {
            let mut decoder = PngDecoder::with_limits(source, limits())?;
            let pixels = check_decoder(&mut decoder)?;
            if decoder.is_apng()? {
                animation(decoder.apng()?.into_frames(), pixels)
            } else {
                still(decoder)
            }
        }
        ImageFormat::Gif => {
            check_gif(bytes)?;
            let mut decoder = GifDecoder::new(source)?;
            let pixels = check_decoder(&mut decoder)?;
            animation(decoder.into_frames(), pixels)
        }
        ImageFormat::WebP => {
            check_webp(bytes)?;
            let mut decoder = WebPDecoder::new(source)?;
            let pixels = check_decoder(&mut decoder)?;
            if decoder.has_animation() {
                animation(decoder.into_frames(), pixels)
            } else {
                still(decoder)
            }
        }
        _ => {
            let mut reader = ImageReader::with_format(source, format);
            reader.limits(limits());
            let mut decoder = reader.into_decoder()?;
            check_decoder(&mut decoder)?;
            still(decoder)
        }
    }
}

fn read(bytes: &[u8]) -> Result<(ImageFormat, DynamicImage)> {
    ensure!(
        bytes.len() <= MAX_PASTED_IMAGE_BYTES,
        "Images can be up to 16 MiB."
    );
    let format = image::guess_format(bytes)
        .ok()
        .filter(|format| {
            matches!(
                format,
                ImageFormat::Png
                    | ImageFormat::Jpeg
                    | ImageFormat::WebP
                    | ImageFormat::Gif
                    | ImageFormat::Bmp
            )
        })
        .with_context(|| format!("Choose a {FORMATS} image."))?;
    let decoded =
        decode(format, bytes).map_err(|error| match error.downcast_ref::<ImageError>() {
            Some(ImageError::Limits(_)) => anyhow!("This image exceeds the size or memory limit."),
            Some(_) => anyhow!("{DAMAGED}"),
            None => error,
        })?;
    Ok((format, decoded))
}

pub(crate) fn verified_mime(bytes: &[u8]) -> Result<&'static str> {
    let (format, _) = read(bytes)?;
    Ok(format.to_mime_type())
}

pub(crate) fn check_stored_profile(bytes: &[u8]) -> Result<()> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(1024);
    limits.max_image_height = Some(1024);
    limits.max_alloc = Some(8 * 1024 * 1024);
    let decoder = PngDecoder::with_limits(Cursor::new(bytes), limits)?;
    ensure!(
        !decoder.is_apng()?
            && decoder.color_type().bytes_per_pixel() == decoder.color_type().channel_count(),
        "Unsupported stored profile image"
    );
    Ok(())
}

pub(crate) fn profile_png(bytes: &[u8]) -> Result<Vec<u8>> {
    let (_, image) = read(bytes)?;
    for edge in [1024, 700] {
        let pixels = if image.width().max(image.height()) > edge {
            image.thumbnail(edge, edge).into_rgba8()
        } else {
            image.to_rgba8()
        };
        let mut png = Vec::new();
        PngEncoder::new(&mut png).write_image(
            &pixels,
            pixels.width(),
            pixels.height(),
            image::ExtendedColorType::Rgba8,
        )?;
        if png.len() <= MAX_PROFILE_IMAGE_BYTES {
            return Ok(png);
        }
    }
    anyhow::bail!("This profile image could not fit in 2 MiB.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Frame, RgbImage, RgbaImage, codecs::gif::GifEncoder};

    #[test]
    fn allowed_formats_decode_and_convert_to_png() -> Result<()> {
        for (format, mime) in [
            (ImageFormat::Png, "image/png"),
            (ImageFormat::Jpeg, "image/jpeg"),
            (ImageFormat::WebP, "image/webp"),
            (ImageFormat::Gif, "image/gif"),
            (ImageFormat::Bmp, "image/bmp"),
        ] {
            let mut input = Cursor::new(Vec::new());
            DynamicImage::from(RgbImage::new(32, 20)).write_to(&mut input, format)?;
            assert_eq!(verified_mime(input.get_ref())?, mime);
            let (stored, pixels) = read(&profile_png(input.get_ref())?)?;
            assert_eq!(stored, ImageFormat::Png);
            assert_eq!((pixels.width(), pixels.height()), (32, 20));
        }
        Ok(())
    }

    #[test]
    fn rejects_non_images_corruption_and_images_beyond_the_limits() -> Result<()> {
        assert!(verified_mime(b"plain text").is_err());
        assert!(verified_mime(b"GIF89a broken").is_err());
        let mut webp = Cursor::new(Vec::new());
        DynamicImage::from(RgbImage::new(32, 20)).write_to(&mut webp, ImageFormat::WebP)?;
        let mut webp = webp.into_inner();
        webp.truncate(webp.len() - 4);
        assert!(verified_mime(&webp).is_err());
        assert!(verified_mime(&vec![0; MAX_PASTED_IMAGE_BYTES + 1]).is_err());
        let mut png = Cursor::new(Vec::new());
        DynamicImage::from(RgbImage::new(MAX_EDGE + 1, 1)).write_to(&mut png, ImageFormat::Png)?;
        assert!(verified_mime(png.get_ref()).is_err());
        let mut gif = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut gif);
            for _ in 0..=MAX_FRAMES {
                encoder.encode_frame(Frame::new(RgbaImage::new(2, 2)))?;
            }
        }
        assert!(verified_mime(&gif).is_err());
        Ok(())
    }
}
