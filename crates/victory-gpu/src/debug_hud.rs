//! Optional desktop HUD that tracks the game's client area without blocking presents.

use std::sync::{Mutex, Once, OnceLock};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

struct Sample {
    game: usize,
    text: String,
    received: Instant,
}

static PENDING: Mutex<Option<Sample>> = Mutex::new(None);

pub(crate) fn update(game: HWND, text: impl FnOnce() -> String) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var("VICTORY_DEBUG_HUD").as_deref() == Ok("1")) {
        return;
    }
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(Sample {
        game: game.0 as usize,
        text: text(),
        received: Instant::now(),
    });
    static START: Once = Once::new();
    START.call_once(|| {
        if let Err(error) = std::thread::Builder::new()
            .name("victory-hud".into())
            .spawn(|| {
                if let Err(error) = run() {
                    tracing::warn!(%error, "debug overlay could not start");
                }
            })
        {
            tracing::warn!(%error, "no debug overlay thread");
        }
    });
}

fn run() -> windows::core::Result<()> {
    const CLASS: PCWSTR = w!("VIctoryDebugHud");
    // SAFETY: the dedicated HUD thread owns this window and its message queue.
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return Err(windows::core::Error::from_thread());
        }
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_LAYERED,
            CLASS,
            w!("DexNom Runtime diagnostics"),
            WS_POPUP,
            0,
            0,
            400,
            102,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        SetLayeredWindowAttributes(hwnd, COLORREF(0), 235, LWA_ALPHA)?;
        let mut sample = None::<Sample>;
        let mut shown_at = None;
        let mut size = (0, 0);
        let mut previous = String::new();
        loop {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    return Ok(());
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if let Some(newest) = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take() {
                sample = Some(newest);
            }
            let position = sample.as_ref().and_then(position);
            if let (Some(sample), Some((x, y))) = (&sample, position) {
                let age = sample.received.elapsed().as_secs();
                let text = if age >= 3 {
                    format!("{}\nNo new frame sample for {age} s", sample.text)
                } else {
                    sample.text.clone()
                };
                if text != previous {
                    let mut words: Vec<u16> = text.encode_utf16().chain([0]).collect();
                    let dc = GetDC(Some(hwnd));
                    let prior_font = SelectObject(dc, GetStockObject(ANSI_FIXED_FONT));
                    let mut bounds = RECT::default();
                    let len = words.len() - 1;
                    DrawTextW(dc, &mut words[..len], &mut bounds, DT_NOPREFIX | DT_LEFT | DT_CALCRECT);
                    SelectObject(dc, prior_font);
                    ReleaseDC(Some(hwnd), dc);
                    size = (size.0.max(bounds.right + 24), size.1.max(bounds.bottom + 16));
                    SetWindowTextW(hwnd, PCWSTR(words.as_ptr()))?;
                    previous = text;
                }
                let placement = Some((x, y, size));
                if shown_at != placement {
                    SetWindowPos(
                        hwnd,
                        Some(HWND_TOPMOST),
                        x,
                        y,
                        size.0,
                        size.1,
                        SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    )?;
                    shown_at = placement;
                }
            } else if shown_at.take().is_some() {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn position(sample: &Sample) -> Option<(i32, i32)> {
    let game = HWND(sample.game as *mut _);
    // SAFETY: the window functions handle a destroyed or hidden game window.
    unsafe {
        if GetForegroundWindow() != game || IsIconic(game).as_bool() {
            return None;
        }
        let mut origin = POINT { x: 12, y: 12 };
        ClientToScreen(game, &mut origin)
            .as_bool()
            .then_some((origin.x, origin.y))
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: handles are supplied by Windows for this HUD window.
    unsafe {
        match msg {
            WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_SETTEXT => {
                let result = DefWindowProcW(hwnd, msg, wparam, lparam);
                let _ = InvalidateRect(Some(hwnd), None, false);
                result
            }
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut paint);
                let mut rect = RECT::default();
                let _ = GetClientRect(hwnd, &mut rect);
                let bg = CreateSolidBrush(COLORREF(0x241b13));
                FillRect(dc, &rect, bg);
                let _ = DeleteObject(bg.into());
                let mut words = [0u16; 384];
                let len = GetWindowTextW(hwnd, &mut words).max(0) as usize;
                let font = SelectObject(dc, GetStockObject(ANSI_FIXED_FONT));
                SetTextColor(dc, COLORREF(0xf4efe5));
                SetBkMode(dc, TRANSPARENT);
                rect.left += 12;
                rect.top += 8;
                DrawTextW(dc, &mut words[..len], &mut rect, DT_NOPREFIX | DT_LEFT);
                SelectObject(dc, font);
                let _ = EndPaint(hwnd, &paint);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
