//! Turning a process id into something a person recognises: a name, and an icon.
//!
//! A Core Audio session knows a pid and, sometimes, a display name it set for
//! itself — which in practice almost nothing does. Everything the volume mixer
//! shows is derived from the executable behind that pid, exactly as Windows' own
//! mixer derives it: the file's `FileDescription` for the name ("Microsoft
//! Teams", not "ms-teams.exe"), and its shell icon for the glyph.
//!
//! Both are expensive relative to a 20Hz poll — a version-info block is a file
//! read, an icon is a GDI round trip — so the caller caches them by executable
//! path and does this once per process.

use std::mem::size_of;

use windows::core::PCWSTR;
use windows::Win32::Foundation::MAX_PATH;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, FILE_FLAGS_AND_ATTRIBUTES,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

use super::wstr::{from_wide, wide};

/// A decoded icon, ready to become an `egui` texture.
///
/// Straight RGBA8, premultiplied by nothing — `egui::ColorImage` wants
/// straight alpha, which is what an icon's AND-mask or 32bpp alpha channel
/// already gives.
pub struct IconImage {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
}

/// Its size, not its four thousand bytes of pixels — this is reachable from
/// `{:?}` on a whole [`Snapshot`](crate::audio::Snapshot).
impl std::fmt::Debug for IconImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "IconImage({}×{})", self.size[0], self.size[1])
    }
}

/// The full path of the executable behind a pid, if this process is allowed to
/// ask.
///
/// `PROCESS_QUERY_LIMITED_INFORMATION` is the least this needs and the most an
/// unelevated process can get for a service or another user's session — which is
/// why the audio device's own service sessions still resolve while, say, an
/// elevated game's may not.
pub fn executable_path(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; MAX_PATH as usize];
        let mut len = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &mut len,
        );
        let _ = windows::Win32::Foundation::CloseHandle(process);
        result.ok()?;
        Some(from_wide(&buffer[..len as usize]))
    }
}

/// The name to show for an executable.
///
/// `FileDescription` first, because that is the string the product calls itself
/// and the one Windows' mixer shows; the file stem when there is no version
/// info, which is the case for most drivers and helper services — Windows shows
/// those as bare file names too.
pub fn display_name(path: &str) -> String {
    file_description(path).unwrap_or_else(|| file_stem(path))
}

fn file_stem(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned())
}

/// `\StringFileInfo\<lang><codepage>\FileDescription` out of the file's version
/// resource.
///
/// The language is read from `\VarFileInfo\Translation` rather than assumed to
/// be `040904b0` (US English / Unicode): a localised build declares its own, and
/// guessing wrong returns nothing at all rather than the English string.
fn file_description(path: &str) -> Option<String> {
    unsafe {
        let path_w = wide(path);
        let path_p = PCWSTR(path_w.as_ptr());

        let size = GetFileVersionInfoSizeW(path_p, None);
        if size == 0 {
            return None;
        }
        let mut block = vec![0u8; size as usize];
        GetFileVersionInfoW(path_p, None, size, block.as_mut_ptr().cast()).ok()?;

        let (lang, codepage) = {
            let key = wide("\\VarFileInfo\\Translation");
            let mut data = std::ptr::null_mut();
            let mut len = 0u32;
            let ok = VerQueryValueW(
                block.as_ptr().cast(),
                PCWSTR(key.as_ptr()),
                &mut data,
                &mut len,
            );
            // Two u16s: language id then code page, in that order.
            if !ok.as_bool() || len < 4 || data.is_null() {
                return None;
            }
            let pair = data.cast::<u16>();
            (*pair, *pair.add(1))
        };

        let key = wide(&format!(
            "\\StringFileInfo\\{lang:04x}{codepage:04x}\\FileDescription"
        ));
        let mut data = std::ptr::null_mut();
        let mut len = 0u32;
        let ok = VerQueryValueW(
            block.as_ptr().cast(),
            PCWSTR(key.as_ptr()),
            &mut data,
            &mut len,
        );
        if !ok.as_bool() || len == 0 || data.is_null() {
            return None;
        }
        // `len` is a character count including the terminator.
        let text = from_wide(std::slice::from_raw_parts(data.cast::<u16>(), len as usize));
        let text = text.trim().to_owned();
        (!text.is_empty()).then_some(text)
    }
}

/// The executable's shell icon, decoded to RGBA.
///
/// `SHGFI_LARGEICON` is 32×32, which is two device pixels per logical pixel at
/// the 16px the rows draw it at — so it stays sharp on a 200% display without
/// asking the shell for a jumbo icon and scaling that down.
pub fn icon(path: &str) -> Option<IconImage> {
    unsafe {
        let path_w = wide(path);
        let mut info = SHFILEINFOW::default();
        let ok = SHGetFileInfoW(
            PCWSTR(path_w.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut info),
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        );
        if ok == 0 || info.hIcon.is_invalid() {
            return None;
        }
        let image = decode_icon(info.hIcon);
        let _ = DestroyIcon(info.hIcon);
        image
    }
}

/// Pull the pixels out of an `HICON`.
///
/// An icon is two bitmaps: colour, and a 1bpp AND-mask. A modern icon's colour
/// bitmap is 32bpp with a real alpha channel and the mask is redundant; a legacy
/// one is 24bpp-in-32 with the mask carrying the transparency. Reading the alpha
/// and falling back to the mask when it is uniformly zero handles both without
/// having to ask which kind this is.
unsafe fn decode_icon(icon: HICON) -> Option<IconImage> {
    let mut info = ICONINFO::default();
    GetIconInfo(icon, &mut info).ok()?;
    // Both bitmaps are ours to free from here on, on every path out.
    let colour = info.hbmColor;
    let mask = info.hbmMask;
    let result = decode_bitmaps(colour, mask);
    if !colour.is_invalid() {
        let _ = DeleteObject(colour.into());
    }
    if !mask.is_invalid() {
        let _ = DeleteObject(mask.into());
    }
    result
}

unsafe fn decode_bitmaps(colour: HBITMAP, mask: HBITMAP) -> Option<IconImage> {
    if colour.is_invalid() {
        return None;
    }
    let mut header = BITMAP::default();
    if GetObjectW(
        colour.into(),
        size_of::<BITMAP>() as i32,
        Some((&raw mut header).cast()),
    ) == 0
    {
        return None;
    }
    let (w, h) = (header.bmWidth as usize, header.bmHeight as usize);
    if w == 0 || h == 0 || w > 512 || h > 512 {
        return None;
    }

    let mut pixels = read_dib(colour, w, h)?;

    // No alpha anywhere: a legacy icon, whose transparency is in the mask. A
    // mask bit of 1 means "leave the background alone", i.e. transparent.
    if pixels.chunks_exact(4).all(|p| p[3] == 0) {
        let mask_pixels = (!mask.is_invalid()).then(|| read_dib(mask, w, h)).flatten();
        match mask_pixels {
            Some(m) => {
                for (pixel, mask) in pixels.chunks_exact_mut(4).zip(m.chunks_exact(4)) {
                    pixel[3] = if mask[0] == 0 { 255 } else { 0 };
                }
            }
            // No usable mask either — better a square icon than an invisible one.
            None => pixels.chunks_exact_mut(4).for_each(|p| p[3] = 255),
        }
    }

    // BGRA (what a DIB gives) to RGBA (what egui wants).
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Some(IconImage {
        size: [w, h],
        rgba: pixels,
    })
}

/// Read a bitmap as top-down 32bpp BGRA.
///
/// The negative `biHeight` is what asks for top-down; a DIB is bottom-up by
/// default and every image consumer in this program counts rows the other way.
unsafe fn read_dib(bitmap: HBITMAP, w: usize, h: usize) -> Option<Vec<u8>> {
    let dc = CreateCompatibleDC(None);
    if dc.is_invalid() {
        return None;
    }
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut buffer = vec![0u8; w * h * 4];
    let lines = GetDIBits(
        dc,
        bitmap,
        0,
        h as u32,
        Some(buffer.as_mut_ptr().cast()),
        &mut info,
        DIB_RGB_COLORS,
    );
    let _ = DeleteDC(dc);
    (lines != 0).then_some(buffer)
}
