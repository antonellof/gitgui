//! RGBA framebuffer with dirty detection against the last frame sent to the
//! terminal, plus PNG export for headless runs.

use std::path::Path;

pub struct Framebuffer {
    w: u32,
    h: u32,
    pixels: Vec<u8>,
    last_sent: Vec<u8>,
}

impl Framebuffer {
    pub fn new(w: u32, h: u32) -> Self {
        let n = (w as usize) * (h as usize) * 4;
        Self {
            w,
            h,
            pixels: vec![0; n],
            last_sent: Vec::new(),
        }
    }

    pub fn width(&self) -> u32 {
        self.w
    }

    pub fn height(&self) -> u32 {
        self.h
    }

    /// Reallocate for a new size and forget the last sent frame.
    pub fn resize(&mut self, w: u32, h: u32) {
        if (w, h) != (self.w, self.h) {
            *self = Self::new(w, h);
        }
    }

    pub fn clear(&mut self, rgba: [u8; 4]) {
        for px in self.pixels.as_chunks_mut::<4>().0 {
            *px = rgba;
        }
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn pixels_mut(&mut self) -> &mut [u8] {
        &mut self.pixels
    }

    /// True when the current pixels differ from the last frame marked sent.
    pub fn is_dirty(&self) -> bool {
        self.pixels != self.last_sent
    }

    pub fn mark_sent(&mut self) {
        self.last_sent.clone_from(&self.pixels);
    }

    #[cfg(test)]
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.w + x) * 4) as usize;
        [self.pixels[i], self.pixels[i + 1], self.pixels[i + 2], self.pixels[i + 3]]
    }

    pub fn save_png(&self, path: &Path) -> anyhow::Result<()> {
        let file = std::fs::File::create(path)?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), self.w, self.h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header()?;
        writer.write_image_data(&self.pixels)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_tracking() {
        let mut fb = Framebuffer::new(4, 2);
        assert!(fb.is_dirty(), "never sent is dirty");
        fb.clear([1, 2, 3, 255]);
        fb.mark_sent();
        assert!(!fb.is_dirty());
        fb.clear([1, 2, 3, 255]);
        assert!(!fb.is_dirty(), "identical repaint is not dirty");
        fb.pixels_mut()[0] = 9;
        assert!(fb.is_dirty());
        assert_eq!(fb.pixel(3, 1), [1, 2, 3, 255]);
    }

    #[test]
    fn png_roundtrip() {
        let dir = std::env::temp_dir().join(format!("gitgui-png-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.png");
        let mut fb = Framebuffer::new(3, 2);
        fb.clear([10, 20, 30, 255]);
        fb.pixels_mut()[4..8].copy_from_slice(&[255, 0, 0, 255]);
        fb.save_png(&path).unwrap();
        let dec = png::Decoder::new(std::fs::File::open(&path).unwrap());
        let mut reader = dec.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (3, 2));
        assert_eq!(&buf[..8], &[10, 20, 30, 255, 255, 0, 0, 255]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
