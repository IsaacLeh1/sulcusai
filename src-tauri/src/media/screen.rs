// SPDX-License-Identifier: AGPL-3.0-only
//! Screenshots for "ask about the screen": the monitor the mouse is on.

use image::RgbaImage;

#[cfg(windows)]
pub fn capture() -> Result<RgbaImage, String> {
    use windows::Win32::Foundation::{HWND, POINT};
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, GetMonitorInfoW, MonitorFromPoint, ReleaseDC, SelectObject,
        BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, MONITORINFO, MONITOR_DEFAULTTONEAREST, SRCCOPY,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

    let fail = |what: &str| format!("Couldn't take a screenshot ({what}).");
    unsafe {
        let mut pt = POINT::default();
        GetCursorPos(&mut pt).map_err(|_| fail("cursor"))?;
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(mon, &mut mi).as_bool() {
            return Err(fail("monitor"));
        }
        let r = mi.rcMonitor;
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        if w <= 0 || h <= 0 {
            return Err(fail("size"));
        }
        let screen = GetDC(HWND::default());
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp);
        let copied = BitBlt(mem, 0, 0, w, h, screen, r.left, r.top, SRCCOPY | CAPTUREBLT);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down rows
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let lines = GetDIBits(mem, bmp, 0, h as u32, Some(buf.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS);
        SelectObject(mem, old);
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(mem);
        ReleaseDC(HWND::default(), screen);
        copied.map_err(|_| fail("copy"))?;
        if lines == 0 {
            return Err(fail("read"));
        }
        // BGRA to RGBA, fully opaque.
        for px in buf.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }
        RgbaImage::from_raw(w as u32, h as u32, buf).ok_or_else(|| fail("buffer"))
    }
}

#[cfg(not(windows))]
pub fn capture() -> Result<RgbaImage, String> {
    Err("Screenshots need Windows for now.".into())
}

/// Screens can be 4K or more; the vision models read about 1-2 megapixels.
pub fn for_model(img: RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    let max = 1920u32;
    if w.max(h) <= max {
        return img;
    }
    let s = max as f64 / w.max(h) as f64;
    image::imageops::resize(&img, (w as f64 * s) as u32, (h as f64 * s) as u32, image::imageops::FilterType::Triangle)
}
