//! Keeps the screen from showing the text console between processes.
//!
//! The UI, mpv and RetroArch each own the display (DRM master) while they run. When one exits
//! and before the next draws, the kernel falls back to the fbdev buffer, which is the text console
//! unless something else was written there. So the session paints a splash into `/dev/fb0` before
//! each handoff, and switches tty1 to graphics mode so the console never draws over it.
//!
//!   fbsplash --vt-graphics    clear tty1, hide its cursor, stop the kernel drawing text on it
//!   fbsplash [MASK]           paint the logo (an 8-bit alpha mask named `*-<w>x<h>.gray`)
//!
//! Every failure is logged to stderr and is non-fatal: no splash is better than no session.

use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;

unsafe extern "C" {
    fn ioctl(fd: i32, request: u64, ...) -> i32;
}

const FBIOGET_VSCREENINFO: u64 = 0x4600;
const FBIOGET_FSCREENINFO: u64 = 0x4602;
const KDSETMODE: u64 = 0x4B3A;
const KD_GRAPHICS: u64 = 1;

/// The UI's background (#0e1318) and the logo colour, as (r, g, b).
const BACKGROUND: [u8; 3] = [0x0e, 0x13, 0x18];
const FOREGROUND: [u8; 3] = [0xe5, 0x09, 0x14];

fn vt_graphics() -> Result<(), String> {
    let mut tty = OpenOptions::new()
        .write(true)
        .open("/dev/tty1")
        .map_err(|e| format!("cannot open /dev/tty1: {e}"))?;
    // Clear, home the cursor and hide it, then stop the kernel painting text on this console.
    let _ = tty.write_all(b"\x1b[2J\x1b[H\x1b[?25l");
    // SAFETY: KDSETMODE takes the mode by value on a tty fd.
    if unsafe { ioctl(tty.as_raw_fd(), KDSETMODE, KD_GRAPHICS) } != 0 {
        return Err(format!("KDSETMODE: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

/// The mask's size from its file name (`logo-1400x300.gray`).
fn mask_size(path: &str) -> Option<(usize, usize)> {
    let stem = path.rsplit('/').next()?.strip_suffix(".gray")?;
    let (w, h) = stem.rsplit('-').next()?.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

fn paint(mask_path: Option<&str>) -> Result<(), String> {
    let mut fb = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/fb0")
        .map_err(|e| format!("cannot open /dev/fb0: {e}"))?;
    let fd = fb.as_raw_fd();
    let mut var = [0u32; 40]; // struct fb_var_screeninfo, 160 bytes
    let mut fix = [0u32; 17]; // struct fb_fix_screeninfo, 68 bytes (line_length is word 12)
    // SAFETY: the buffers are at least as large as the structs the kernel fills in.
    unsafe {
        if ioctl(fd, FBIOGET_VSCREENINFO, var.as_mut_ptr()) != 0 {
            return Err(format!("FBIOGET_VSCREENINFO: {}", std::io::Error::last_os_error()));
        }
        if ioctl(fd, FBIOGET_FSCREENINFO, fix.as_mut_ptr()) != 0 {
            return Err(format!("FBIOGET_FSCREENINFO: {}", std::io::Error::last_os_error()));
        }
    }
    let (width, height, bpp, stride) = (var[0] as usize, var[1] as usize, var[6], fix[12] as usize);
    if bpp != 32 || width == 0 || height == 0 || stride < width * 4 {
        return Err(format!("unsupported framebuffer {width}x{height} {bpp}bpp stride {stride}"));
    }

    let mask = match mask_path {
        Some(p) => {
            let (w, h) = mask_size(p).ok_or_else(|| format!("{p}: name must end in -<w>x<h>.gray"))?;
            let bytes = std::fs::read(p).map_err(|e| format!("cannot read {p}: {e}"))?;
            if bytes.len() != w * h {
                return Err(format!("{p}: {} bytes, expected {}", bytes.len(), w * h));
            }
            Some((w, h, bytes))
        }
        None => None,
    };

    // BGRX, the layout of the kernel's XRGB8888 console framebuffer.
    let px = |c: [u8; 3]| [c[2], c[1], c[0], 0xff];
    let mut frame = Vec::with_capacity(stride * height);
    let (left, top) = mask.as_ref().map_or((0, 0), |(w, h, _)| (width.saturating_sub(*w) / 2, height.saturating_sub(*h) / 2));
    for y in 0..height {
        let mut row = vec![0u8; stride];
        for x in 0..width {
            let mut c = BACKGROUND;
            if let Some((w, h, a)) = &mask {
                if x >= left && x < left + w && y >= top && y < top + h {
                    let alpha = a[(y - top) * w + (x - left)] as u32;
                    for i in 0..3 {
                        c[i] = ((BACKGROUND[i] as u32 * (255 - alpha) + FOREGROUND[i] as u32 * alpha) / 255) as u8;
                    }
                }
            }
            row[x * 4..x * 4 + 4].copy_from_slice(&px(c));
        }
        frame.extend_from_slice(&row);
    }
    fb.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    fb.write_all(&frame).map_err(|e| format!("cannot write the framebuffer: {e}"))
}

fn main() {
    let arg = std::env::args().nth(1);
    let result = match arg.as_deref() {
        Some("--vt-graphics") => vt_graphics(),
        other => paint(other),
    };
    if let Err(e) = result {
        eprintln!("fbsplash: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::mask_size;

    #[test]
    fn mask_size_comes_from_the_file_name() {
        assert_eq!(mask_size("/nix/store/x/logo-1400x300.gray"), Some((1400, 300)));
        assert_eq!(mask_size("logo.gray"), None);
        assert_eq!(mask_size("logo-1400x300.png"), None);
    }
}
