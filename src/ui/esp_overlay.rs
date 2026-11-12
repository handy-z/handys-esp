use std::collections::HashMap;

use handy_sdk::ui::d2d::{Vector2, dwrite_factory};
use handy_sdk::ui::gpu_overlay::GpuOverlay;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D1_COLOR_F};
use windows::Win32::Graphics::Direct2D::D2D1_DRAW_TEXT_OPTIONS_NONE;
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_BOLD, DWRITE_PARAGRAPH_ALIGNMENT,
    DWRITE_PARAGRAPH_ALIGNMENT_FAR, DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_WORD_WRAPPING_NO_WRAP, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::core::w;

use crate::app::view::distance_text;
use crate::esp::marker::Frame;

const INK: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 80. / 255.,
    g: 220. / 255.,
    b: 120. / 255.,
    a: 1.,
};
const SHADE: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 0.,
    g: 0.,
    b: 0.,
    a: 160. / 255.,
};
const TEXT: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 1.,
    g: 1.,
    b: 1.,
    a: 1.,
};
// a label's layout box; far wider and taller than any one line
const LABEL_W: f32 = 800.;
const LABEL_H: f32 = 200.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Draw {
    pub r#box: bool,
    pub label: bool,
    pub distance: bool,
    pub tracer: bool,
}

impl Default for Draw {
    fn default() -> Self {
        Self {
            r#box: true,
            label: true,
            distance: true,
            tracer: true,
        }
    }
}

/// The markers drawn on the SDK's GpuOverlay (click-through, top-most, GPU-drawn over Roblox). Use from one thread:
/// the one that called create(), which must also call pump() for the window's messages.
pub struct EspOverlay {
    gpu: GpuOverlay,
    dpi: u32,
    dwrite: Option<IDWriteFactory>,
    above: Option<IDWriteTextFormat>,                 // centred, bottom-aligned
    below: Option<IDWriteTextFormat>,                 // centred, top-aligned
    layouts: [HashMap<String, IDWriteTextLayout>; 2], // above, below
}

impl Default for EspOverlay {
    fn default() -> Self {
        Self::new()
    }
}

// Cached: laying text out is the costly part.
fn layout(
    cache: &mut HashMap<String, IDWriteTextLayout>,
    dwrite: Option<&IDWriteFactory>,
    fmt: Option<&IDWriteTextFormat>,
    utf8: &str,
) -> Option<IDWriteTextLayout> {
    if let Some(l) = cache.get(utf8) {
        return Some(l.clone());
    }
    let (fmt, dwrite) = (fmt?, dwrite?);
    if cache.len() >= 2048 {
        cache.clear(); // names and distances repeat; this only bounds a long session
    }
    let t: Vec<u16> = utf8.encode_utf16().collect();
    let l = unsafe { dwrite.CreateTextLayout(&t, fmt, LABEL_W, LABEL_H) }.ok()?;
    cache.insert(utf8.to_owned(), l.clone());
    Some(l)
}

impl EspOverlay {
    pub fn new() -> Self {
        Self {
            gpu: GpuOverlay::new("handys-esp-overlay"),
            dpi: 0,
            dwrite: None,
            above: None,
            below: None,
            layouts: [HashMap::new(), HashMap::new()],
        }
    }

    /// Hidden until show().
    pub fn create(&mut self) {
        self.gpu.create();
        if self.dwrite.is_none() {
            self.dwrite = dwrite_factory(); // per thread: made on the thread that draws
        }
    }

    pub fn wait_for_frame(&self) {
        self.gpu.wait_for_frame();
    }

    pub fn hide(&self) {
        self.gpu.hide();
    }

    pub fn pump(&self) {
        self.gpu.pump();
    }

    fn text_formats(&mut self, dpi: u32) {
        if dpi == self.dpi && self.above.is_some() {
            return;
        }
        self.dpi = dpi;
        self.above = None;
        self.below = None;
        let Some(dw) = &self.dwrite else { return };
        let make = |vertical: DWRITE_PARAGRAPH_ALIGNMENT| -> Option<IDWriteTextFormat> {
            unsafe {
                let f = dw
                    .CreateTextFormat(
                        w!("Segoe UI"),
                        None,
                        DWRITE_FONT_WEIGHT_BOLD,
                        DWRITE_FONT_STYLE_NORMAL,
                        DWRITE_FONT_STRETCH_NORMAL,
                        12.0 * dpi as f32 / 96.0,
                        w!(""),
                    )
                    .ok()?;
                let _ = f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
                let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
                let _ = f.SetParagraphAlignment(vertical);
                Some(f)
            }
        };
        self.above = make(DWRITE_PARAGRAPH_ALIGNMENT_FAR);
        self.below = make(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
        self.layouts[0].clear();
        self.layouts[1].clear();
    }

    /// Covers target's client area and draws the frame, scaled from frame.size to the client size. Hides when the
    /// target is null or minimised, or no Direct3D 11 device (hardware or WARP) can be made.
    pub fn show(&mut self, target: HWND, f: &Frame, d: Draw) {
        if target.is_invalid() || f.size.x <= 0.0 || f.size.y <= 0.0 {
            return self.hide();
        }
        let dpi = unsafe { GetDpiForWindow(target) };
        let s = dpi as f32 / 96.0;
        let (thin, thick) = (1.5 * s, 3.5 * s);
        self.text_formats(dpi);

        let Self {
            gpu,
            dwrite,
            above,
            below,
            layouts,
            ..
        } = self;
        let gpu: &GpuOverlay = gpu;
        gpu.show(target, |dc, client| {
            let (Some(ink), Some(shade), Some(text)) = (gpu.brush(INK), gpu.brush(SHADE), gpu.brush(TEXT)) else {
                return;
            };
            let (w, h) = (client.width, client.height);
            let (sx, sy) = (w / f.size.x, h / f.size.y);
            let mut label = |utf8: &str, cx: f32, y: f32, is_above: bool| {
                let (cache, fmt) = if is_above {
                    (&mut layouts[0], above.as_ref())
                } else {
                    (&mut layouts[1], below.as_ref())
                };
                let Some(l) = layout(cache, dwrite.as_ref(), fmt, utf8) else {
                    return;
                };
                // the format aligns the line
                let at = Vector2 {
                    X: cx - LABEL_W / 2.0,
                    Y: if is_above { y - LABEL_H } else { y },
                };
                unsafe {
                    dc.DrawTextLayout(
                        Vector2 {
                            X: at.X + 1.0,
                            Y: at.Y + 1.0,
                        },
                        &l,
                        &shade,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                    );
                    dc.DrawTextLayout(at, &l, &text, D2D1_DRAW_TEXT_OPTIONS_NONE);
                }
            };
            for m in &f.markers {
                let (l, t, r, b) = (
                    m.r#box.left * sx,
                    m.r#box.top * sy,
                    m.r#box.right * sx,
                    m.r#box.bottom * sy,
                );
                let cx = (l + r) / 2.0;
                if d.tracer {
                    let (from, to) = (Vector2 { X: w / 2.0, Y: h }, Vector2 { X: cx, Y: b });
                    unsafe {
                        dc.DrawLine(from, to, &shade, thick, None);
                        dc.DrawLine(from, to, &ink, thin, None);
                    }
                }
                if d.r#box {
                    let rc = D2D_RECT_F {
                        left: l,
                        top: t,
                        right: r,
                        bottom: b,
                    };
                    unsafe {
                        dc.DrawRectangle(&rc, &shade, thick, None);
                        dc.DrawRectangle(&rc, &ink, thin, None);
                    }
                }
                if d.label {
                    label(&m.name, cx, t - 2.0, true);
                }
                if d.distance
                    && let Some(studs) = m.studs
                {
                    label(&distance_text(studs), cx, b + 2.0, false);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::esp::marker::{Marker, Rect};
    use handy_sdk::roblox::screen::Vec2;
    use windows::Win32::Foundation::{COLORREF, POINT};
    use windows::Win32::Graphics::Dwm::DwmFlush;
    use windows::Win32::Graphics::Gdi::{
        BLACK_BRUSH, ClientToScreen, GetDC, GetPixel, GetStockObject, HBRUSH, ReleaseDC, UpdateWindow,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW,
        RegisterClassExW, WINDOW_EX_STYLE, WNDCLASSEXW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE, WindowFromPoint,
    };
    use windows::core::PCWSTR;

    unsafe extern "system" fn def_proc(
        h: HWND,
        msg: u32,
        wp: windows::Win32::Foundation::WPARAM,
        lp: windows::Win32::Foundation::LPARAM,
    ) -> windows::Win32::Foundation::LRESULT {
        unsafe { DefWindowProcW(h, msg, wp, lp) }
    }

    fn target_window() -> HWND {
        unsafe {
            let inst = GetModuleHandleW(None).unwrap_or_default();
            let class = w!("handys-esp-test-target");
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(def_proc),
                hInstance: inst.into(),
                hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassExW(&wc);
            let w = CreateWindowExW(
                WINDOW_EX_STYLE(WS_EX_TOPMOST.0),
                class,
                PCWSTR::null(),
                WS_POPUP | WS_VISIBLE,
                40,
                40,
                800,
                600,
                None,
                None,
                Some(inst.into()),
                None,
            )
            .expect("test window");
            let _ = UpdateWindow(w);
            w
        }
    }

    fn pump() {
        let mut m = MSG::default();
        while unsafe { PeekMessageW(&mut m, None, 0, 0, PM_REMOVE) }.as_bool() {
            unsafe { DispatchMessageW(&m) };
        }
    }

    fn green(c: COLORREF) -> bool {
        let (r, g, b) = (c.0 & 0xFF, (c.0 >> 8) & 0xFF, (c.0 >> 16) & 0xFF);
        g > r + 60 && g > b + 60
    }

    // Needs an interactive desktop, not Roblox: shows an 800x600 black window with the overlay on it for about a second.
    #[test]
    #[ignore]
    fn live_overlay_draws_clicks_through_and_is_fast() {
        let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }; // screen = client pixels
        let target = target_window();
        let mut overlay = EspOverlay::new();
        overlay.create();
        let marker = |l: f32, t: f32, r: f32, b: f32, name: String, studs: f32| Marker {
            r#box: Rect {
                left: l,
                top: t,
                right: r,
                bottom: b,
            },
            name,
            studs: Some(studs),
        };
        let mut f = Frame {
            size: Vec2 { x: 800., y: 600. },
            markers: vec![marker(100., 100., 300., 300., "Chest".into(), 12.)],
        };
        for i in 1..128 {
            // load: boxes, labels, distances and tracers on the right half
            let (x, y) = (420. + (i * 37 % 330) as f32, 40. + (i * 53 % 500) as f32);
            f.markers
                .push(marker(x, y, x + 30., y + 40., format!("Coin {i}"), i as f32));
        }
        pump();
        overlay.show(target, &f, Draw::default());
        unsafe {
            let _ = DwmFlush();
            let _ = DwmFlush(); // two compositions: the frame is on screen
        }
        let mut o = POINT::default();
        let _ = unsafe { ClientToScreen(target, &mut o) };
        let (edge, inside) = unsafe {
            let screen = GetDC(None);
            let px = (
                GetPixel(screen, o.x + 100, o.y + 200),
                GetPixel(screen, o.x + 200, o.y + 200),
            );
            ReleaseDC(None, screen);
            px
        };
        assert!(green(edge)); // the box's left edge
        assert_eq!(inside.0, 0); // see-through inside it
        let mid = POINT {
            x: o.x + 200,
            y: o.y + 200,
        };
        assert_eq!(unsafe { WindowFromPoint(mid) }, target); // clicks go through the overlay

        let frames = 300;
        let (mut total, mut worst) = (0f64, 0f64);
        for i in 0..frames {
            pump();
            overlay.pump();
            overlay.wait_for_frame(); // as the engine does: paced to the display, not timed here
            f.markers[0].r#box.left = 100. + (i % 2) as f32; // a different frame each time
            let t0 = std::time::Instant::now();
            overlay.show(target, &f, Draw::default());
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            total += ms;
            worst = worst.max(ms);
        }
        println!(
            "  128 markers on 800x600: {:.2} ms a frame on average, {worst:.2} ms worst",
            total / f64::from(frames)
        );
        overlay.hide();
        let _ = unsafe { DestroyWindow(target) };
    }
}
