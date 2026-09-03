//! Kitty graphics protocol encoder.
//!
//! Every function appends raw bytes to an output buffer. Nothing here talks to
//! the terminal directly, so the exact sequences can be snapshot-tested. See
//! docs/PROTOCOLS.md section 4 for the byte layout.

use std::fmt::Write as _;
use std::io::Write;

use base64::Engine as _;

const APC: &[u8] = b"\x1b_G";
const ST: &[u8] = b"\x1b\\";

/// Maximum number of base64 characters per chunk on the direct transport.
pub const DIRECT_CHUNK: usize = 4096;

/// Image id used by the capability probe (1x1 RGB, action query).
pub const PROBE_IMAGE_ID: u32 = 31;
/// Image id used by the shared memory probe.
pub const SHM_PROBE_IMAGE_ID: u32 = 32;

fn b64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// `APC G i=31,s=1,v=1,a=q,t=d,f=24 ; AAAA ST`: graphics capability probe.
pub fn encode_query_probe(out: &mut Vec<u8>) {
    out.extend_from_slice(APC);
    out.extend_from_slice(b"i=31,s=1,v=1,a=q,t=d,f=24;AAAA");
    out.extend_from_slice(ST);
}

/// Shared memory capability probe: a 1x1 RGBA image transmitted via the named
/// shm object with `a=q`. The terminal answers `OK` and unlinks the object when
/// it supports the transport.
pub fn encode_shm_probe(out: &mut Vec<u8>, shm_name: &str) {
    out.extend_from_slice(APC);
    out.extend_from_slice(b"i=32,s=1,v=1,a=q,t=s,f=32;");
    out.extend_from_slice(b64(shm_name.as_bytes()).as_bytes());
    out.extend_from_slice(ST);
}

/// Transmit and display a frame stored in a POSIX shared memory object.
/// `cols` and `rows` bind the placement to the whole cell grid, so the
/// terminal scales the image to the grid regardless of how it maps image
/// pixels to screen pixels (Ghostty and cmux treat them as logical points
/// on HiDPI displays, which would otherwise show the frame at 2x).
pub fn encode_shm_place(
    out: &mut Vec<u8>,
    w: u32,
    h: u32,
    id: u32,
    shm_name: &str,
    cols: u32,
    rows: u32,
) {
    let mut ctl = String::new();
    let _ = write!(
        ctl,
        "a=T,t=s,f=32,s={w},v={h},c={cols},r={rows},i={id},p={id},C=1,q=2;"
    );
    out.extend_from_slice(APC);
    out.extend_from_slice(ctl.as_bytes());
    out.extend_from_slice(b64(shm_name.as_bytes()).as_bytes());
    out.extend_from_slice(ST);
}

/// Transmit and display a frame inline: zlib compressed, base64 encoded, split
/// into chunks of at most [`DIRECT_CHUNK`] characters.
pub fn encode_direct_place(
    out: &mut Vec<u8>,
    w: u32,
    h: u32,
    id: u32,
    rgba: &[u8],
    cols: u32,
    rows: u32,
) {
    debug_assert_eq!(rgba.len(), (w as usize) * (h as usize) * 4);
    let mut enc = flate2::write::ZlibEncoder::new(
        Vec::with_capacity(rgba.len() / 4),
        flate2::Compression::fast(),
    );
    enc.write_all(rgba).expect("write to Vec cannot fail");
    let z = enc.finish().expect("zlib finish cannot fail on Vec");
    let payload = b64(&z);
    let chunks: Vec<&[u8]> = payload.as_bytes().chunks(DIRECT_CHUNK).collect();
    let n = chunks.len().max(1);
    for (idx, chunk) in chunks.iter().enumerate() {
        let more = if idx + 1 < n { 1 } else { 0 };
        out.extend_from_slice(APC);
        if idx == 0 {
            let mut ctl = String::new();
            let _ = write!(
                ctl,
                "a=T,t=d,o=z,f=32,s={w},v={h},c={cols},r={rows},i={id},p={id},C=1,q=2,m={more};"
            );
            out.extend_from_slice(ctl.as_bytes());
        } else {
            let _ = write!(out, "m={more};");
        }
        out.extend_from_slice(chunk);
        out.extend_from_slice(ST);
    }
}

/// Delete the placement with the given id (image data is kept).
pub fn encode_delete_placement(out: &mut Vec<u8>, id: u32) {
    out.extend_from_slice(APC);
    let _ = write!(out, "a=d,d=i,i={id},p={id},q=2");
    out.extend_from_slice(ST);
}

/// Delete all placements and free all image data.
pub fn encode_delete_all(out: &mut Vec<u8>) {
    out.extend_from_slice(APC);
    out.extend_from_slice(b"a=d,d=A,q=2");
    out.extend_from_slice(ST);
}

/// Which byte path carries frame pixels to the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// POSIX shared memory, local terminals only.
    Shm,
    /// zlib + base64 inline, works over SSH.
    Direct,
}

/// Builds the escape sequences for one full-frame update following
/// docs/PROTOCOLS.md 4.4: synchronized output, home cursor, new placement on
/// top, old placement removed.
pub struct FrameEncoder {
    transport: Transport,
    frame_no: u64,
    shm_seq: u32,
    pid: u32,
    /// Placement id currently visible, if any.
    visible: Option<u32>,
}

impl FrameEncoder {
    pub fn new(transport: Transport, pid: u32) -> Self {
        Self {
            transport,
            frame_no: 0,
            shm_seq: 0,
            pid,
            visible: None,
        }
    }

    /// Next shm object name, at most 31 bytes including the leading slash.
    pub fn next_shm_name(&mut self) -> String {
        self.shm_seq = self.shm_seq.wrapping_add(1) % 1_000_000;
        let name = format!("/tg-{}-{}", self.pid, self.shm_seq);
        debug_assert!(name.len() <= 31);
        name
    }

    /// Image id for the next frame: alternates between 1 and 2.
    fn next_id(&self) -> u32 {
        1 + (self.frame_no % 2) as u32
    }

    /// Encode a frame. On the shm transport the pixels must already be in the
    /// object named `shm_name`; on the direct transport they are read from
    /// `rgba`.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_frame(
        &mut self,
        out: &mut Vec<u8>,
        w: u32,
        h: u32,
        cols: u32,
        rows: u32,
        rgba: &[u8],
        shm_name: Option<&str>,
    ) {
        let id = self.next_id();
        out.extend_from_slice(b"\x1b[?2026h");
        out.extend_from_slice(b"\x1b[1;1H");
        match self.transport {
            Transport::Shm => {
                let name = shm_name.expect("shm transport requires an shm name");
                encode_shm_place(out, w, h, id, name, cols, rows);
            }
            Transport::Direct => encode_direct_place(out, w, h, id, rgba, cols, rows),
        }
        if let Some(old) = self.visible {
            if old != id {
                encode_delete_placement(out, old);
            }
        }
        out.extend_from_slice(b"\x1b[?2026l");
        self.visible = Some(id);
        self.frame_no += 1;
    }

    /// Forget the visible placement, for example after a resize where all
    /// placements were deleted with [`encode_delete_all`].
    pub fn reset(&mut self) {
        self.visible = None;
    }
}

/// A POSIX shared memory object holding one RGBA frame.
///
/// The object is not unlinked on drop: the terminal unlinks it after reading.
/// Call [`Shm::unlink`] explicitly when the terminal never consumed it.
pub struct Shm {
    name: std::ffi::CString,
}

impl Shm {
    /// Create the object, write `data` into it, unmap and close. Errors map to
    /// `std::io::Error` from `errno`.
    pub fn create_and_fill(name: &str, data: &[u8]) -> std::io::Result<Self> {
        let cname =
            std::ffi::CString::new(name).map_err(|_| std::io::Error::other("nul in shm name"))?;
        // SAFETY: cname is a valid NUL terminated string; flags are plain ints.
        let fd = unsafe {
            libc::shm_open(
                cname.as_ptr(),
                libc::O_CREAT | libc::O_RDWR | libc::O_EXCL,
                0o600 as libc::c_uint,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let shm = Self { name: cname };
        let len = data.len();
        // SAFETY: fd is a valid descriptor we own.
        if unsafe { libc::ftruncate(fd, len as libc::off_t) } != 0 {
            let e = std::io::Error::last_os_error();
            unsafe { libc::close(fd) };
            shm.unlink();
            return Err(e);
        }
        if len > 0 {
            // SAFETY: mapping len bytes of a descriptor we just sized to len.
            let ptr = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    len,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fd,
                    0,
                )
            };
            if ptr == libc::MAP_FAILED {
                let e = std::io::Error::last_os_error();
                unsafe { libc::close(fd) };
                shm.unlink();
                return Err(e);
            }
            // SAFETY: ptr points to len writable bytes, data has len bytes.
            unsafe {
                std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, len);
                libc::munmap(ptr, len);
            }
        }
        // SAFETY: fd is valid and no longer needed.
        unsafe { libc::close(fd) };
        Ok(shm)
    }

    /// True while the object still exists (the terminal has not consumed it).
    pub fn exists(&self) -> bool {
        // SAFETY: name is a valid C string.
        let fd = unsafe { libc::shm_open(self.name.as_ptr(), libc::O_RDONLY, 0 as libc::c_uint) };
        if fd >= 0 {
            // SAFETY: fd is valid.
            unsafe { libc::close(fd) };
            true
        } else {
            false
        }
    }

    /// Remove the object. Safe to call when the terminal already removed it.
    pub fn unlink(&self) {
        // SAFETY: name is a valid C string.
        unsafe { libc::shm_unlink(self.name.as_ptr()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_probe_bytes() {
        let mut out = Vec::new();
        encode_query_probe(&mut out);
        assert_eq!(out, b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\");
    }

    #[test]
    fn shm_probe_bytes() {
        let mut out = Vec::new();
        encode_shm_probe(&mut out, "/tg-1-1");
        // base64("/tg-1-1") = "L3RnLTEtMQ=="
        assert_eq!(out, b"\x1b_Gi=32,s=1,v=1,a=q,t=s,f=32;L3RnLTEtMQ==\x1b\\");
    }

    #[test]
    fn shm_place_bytes() {
        let mut out = Vec::new();
        encode_shm_place(&mut out, 640, 480, 1, "/tg-1-1", 80, 24);
        assert_eq!(
            out,
            b"\x1b_Ga=T,t=s,f=32,s=640,v=480,c=80,r=24,i=1,p=1,C=1,q=2;L3RnLTEtMQ==\x1b\\"
        );
    }

    #[test]
    fn delete_bytes() {
        let mut out = Vec::new();
        encode_delete_placement(&mut out, 2);
        assert_eq!(out, b"\x1b_Ga=d,d=i,i=2,p=2,q=2\x1b\\");
        out.clear();
        encode_delete_all(&mut out);
        assert_eq!(out, b"\x1b_Ga=d,d=A,q=2\x1b\\");
    }

    fn split_apcs(bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut seqs = Vec::new();
        let mut i = 0;
        while i + 3 <= bytes.len() {
            if &bytes[i..i + 3] == b"\x1b_G" {
                let end = bytes[i..].windows(2).position(|w| w == b"\x1b\\").unwrap() + i;
                seqs.push(bytes[i + 3..end].to_vec());
                i = end + 2;
            } else {
                i += 1;
            }
        }
        seqs
    }

    #[test]
    fn direct_single_chunk() {
        // 2x2 solid image compresses to a handful of bytes: one chunk, m=0.
        let rgba = [200u8, 100, 50, 255].repeat(4);
        let mut out = Vec::new();
        encode_direct_place(&mut out, 2, 2, 1, &rgba, 1, 1);
        let seqs = split_apcs(&out);
        assert_eq!(seqs.len(), 1);
        let s = String::from_utf8(seqs[0].clone()).unwrap();
        let (ctl, payload) = s.split_once(';').unwrap();
        assert_eq!(ctl, "a=T,t=d,o=z,f=32,s=2,v=2,c=1,r=1,i=1,p=1,C=1,q=2,m=0");
        let z = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .unwrap();
        let mut dec = flate2::read::ZlibDecoder::new(&z[..]);
        let mut back = Vec::new();
        std::io::Read::read_to_end(&mut dec, &mut back).unwrap();
        assert_eq!(back, rgba);
    }

    #[test]
    fn direct_multi_chunk() {
        // Incompressible noise so the base64 payload exceeds one chunk.
        let mut x: u32 = 0x1234_5678;
        let n = 64 * 64;
        let mut rgba = Vec::with_capacity(n * 4);
        for _ in 0..n * 4 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            rgba.push((x & 0xff) as u8);
        }
        let mut out = Vec::new();
        encode_direct_place(&mut out, 64, 64, 2, &rgba, 8, 4);
        let seqs = split_apcs(&out);
        assert!(
            seqs.len() >= 2,
            "expected several chunks, got {}",
            seqs.len()
        );
        let first = String::from_utf8(seqs[0].clone()).unwrap();
        assert!(first.starts_with("a=T,t=d,o=z,f=32,s=64,v=64,c=8,r=4,i=2,p=2,C=1,q=2,m=1;"));
        let mut payload = String::new();
        for (i, s) in seqs.iter().enumerate() {
            let s = String::from_utf8(s.clone()).unwrap();
            let (ctl, chunk) = s.split_once(';').unwrap();
            assert!(chunk.len() <= DIRECT_CHUNK);
            if i > 0 {
                let want = if i + 1 == seqs.len() { "m=0" } else { "m=1" };
                assert_eq!(ctl, want);
            }
            payload.push_str(chunk);
        }
        let z = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .unwrap();
        let mut dec = flate2::read::ZlibDecoder::new(&z[..]);
        let mut back = Vec::new();
        std::io::Read::read_to_end(&mut dec, &mut back).unwrap();
        assert_eq!(back, rgba);
    }

    #[test]
    fn frame_sequence_shm() {
        let mut enc = FrameEncoder::new(Transport::Shm, 7);
        let mut out = Vec::new();
        enc.encode_frame(&mut out, 8, 4, 2, 1, &[], Some("/tg-7-1"));
        // base64("/tg-7-1") = "L3RnLTctMQ=="
        assert_eq!(
            out,
            b"\x1b[?2026h\x1b[1;1H\x1b_Ga=T,t=s,f=32,s=8,v=4,c=2,r=1,i=1,p=1,C=1,q=2;L3RnLTctMQ==\x1b\\\x1b[?2026l"
        );
        out.clear();
        enc.encode_frame(&mut out, 8, 4, 2, 1, &[], Some("/tg-7-2"));
        assert_eq!(
            out,
            b"\x1b[?2026h\x1b[1;1H\x1b_Ga=T,t=s,f=32,s=8,v=4,c=2,r=1,i=2,p=2,C=1,q=2;L3RnLTctMg==\x1b\\\x1b_Ga=d,d=i,i=1,p=1,q=2\x1b\\\x1b[?2026l"
        );
        out.clear();
        enc.encode_frame(&mut out, 8, 4, 2, 1, &[], Some("/tg-7-3"));
        assert!(out.windows(7).any(|w| w == b"i=1,p=1"));
        assert!(out.ends_with(b"\x1b_Ga=d,d=i,i=2,p=2,q=2\x1b\\\x1b[?2026l"));
    }

    #[test]
    fn frame_sequence_reset_skips_delete() {
        let mut enc = FrameEncoder::new(Transport::Shm, 7);
        let mut out = Vec::new();
        enc.encode_frame(&mut out, 1, 1, 1, 1, &[], Some("/a"));
        enc.reset();
        out.clear();
        enc.encode_frame(&mut out, 1, 1, 1, 1, &[], Some("/a"));
        assert!(!out.windows(5).any(|w| w == b"a=d,d"));
    }

    #[test]
    fn shm_name_length() {
        let mut enc = FrameEncoder::new(Transport::Shm, u32::MAX);
        for _ in 0..5 {
            assert!(enc.next_shm_name().len() <= 31);
        }
    }

    #[test]
    fn shm_roundtrip() {
        let name = format!("/tg-test-{}", std::process::id());
        let data = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
        let shm = Shm::create_and_fill(&name, &data).unwrap();
        assert!(shm.exists());
        // Read back through a fresh mapping.
        let cname = std::ffi::CString::new(name.clone()).unwrap();
        let fd = unsafe { libc::shm_open(cname.as_ptr(), libc::O_RDONLY, 0 as libc::c_uint) };
        assert!(fd >= 0);
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                8,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        assert_ne!(ptr, libc::MAP_FAILED);
        let back = unsafe { std::slice::from_raw_parts(ptr as *const u8, 8).to_vec() };
        unsafe {
            libc::munmap(ptr, 8);
            libc::close(fd);
        }
        assert_eq!(back, data);
        shm.unlink();
        assert!(!shm.exists());
        // Creating twice with O_EXCL after unlink works again.
        let shm2 = Shm::create_and_fill(&name, &data).unwrap();
        shm2.unlink();
    }
}
