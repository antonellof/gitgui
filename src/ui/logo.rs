//! The gitgui logo (`assets/logo.png`, compiled in): an image handle for the
//! UI and RGBA pixels for the desktop window icon. Decoded once with the
//! `png` crate, so no image codecs come along.

use std::sync::OnceLock;

use iced_core::image::Handle;

const BYTES: &[u8] = include_bytes!("../../assets/logo.png");

pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

fn decode() -> Option<Pixels> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(BYTES));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf.chunks(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf.chunks(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some(Pixels {
        width: info.width,
        height: info.height,
        rgba,
    })
}

pub fn pixels() -> Option<&'static Pixels> {
    static PIXELS: OnceLock<Option<Pixels>> = OnceLock::new();
    PIXELS.get_or_init(decode).as_ref()
}

/// The logo for an `image` widget, or `None` if the asset failed to decode.
pub fn handle() -> Option<Handle> {
    static HANDLE: OnceLock<Option<Handle>> = OnceLock::new();
    HANDLE
        .get_or_init(|| pixels().map(|p| Handle::from_rgba(p.width, p.height, p.rgba.clone())))
        .clone()
}

#[cfg(test)]
mod tests {
    #[test]
    fn logo_decodes_to_square_rgba() {
        let p = super::pixels().expect("assets/logo.png decodes");
        assert_eq!(p.width, p.height);
        assert_eq!(p.rgba.len(), (p.width * p.height * 4) as usize);
        assert!(super::handle().is_some());
    }
}
