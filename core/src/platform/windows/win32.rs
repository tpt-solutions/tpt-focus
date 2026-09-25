//! Raw Win32 calls behind the Windows context provider.
//!
//! Everything here is a thin, side-effecting wrapper; the decision logic
//! lives in pure functions so it can be unit tested without a desktop.

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId,
};

/// Tolerance (pixels) when comparing window and monitor bounds.
pub const FULLSCREEN_TOLERANCE: i32 = 2;

/// Initialise COM on this thread (multithreaded apartment).
///
/// WinRT activation and event subscription require an initialised apartment;
/// a plain worker thread has none. `S_FALSE` (already initialised) and
/// `RPC_E_CHANGED_MODE` ( initialised as STA, equally usable for WinRT) are
/// both fine, so every outcome is accepted. Idempotent.
pub fn ensure_com_initialized() {
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
}

/// Handle of the window currently in the foreground, if any.
pub fn foreground_window() -> Option<HWND> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_invalid() {
        None
    } else {
        Some(hwnd)
    }
}

/// Outer bounds of `hwnd` in screen coordinates.
pub fn window_rect(hwnd: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
    Some(rect)
}

/// Bounds of the monitor `hwnd` currently occupies.
pub fn monitor_rect(hwnd: HWND) -> Option<RECT> {
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_invalid() {
        return None;
    }

    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        rcMonitor: RECT::default(),
        rcWork: RECT::default(),
        dwFlags: Default::default(),
    };
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return None;
    }
    Some(info.rcMonitor)
}

/// Owning process id of `hwnd`, if the window is still alive.
pub fn process_id(hwnd: HWND) -> Option<u32> {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    (pid != 0).then_some(pid)
}

/// Executable path of `pid` (needs `PROCESS_QUERY_LIMITED_INFORMATION`).
pub fn process_image_path(pid: u32) -> Option<String> {
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buffer = [0u16; 1024];
    let mut size = buffer.len() as u32;

    let result = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_FORMAT(0),
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        )
    };
    let _ = unsafe { CloseHandle(process) };

    result.ok()?;
    String::from_utf16(&buffer[..size as usize]).ok()
}

/// Executable file stem (e.g. `"slack"`) of `pid`.
pub fn process_name(pid: u32) -> Option<String> {
    process_image_path(pid)
        .as_deref()
        .map(app_id_from_image_path)
}

/// True when `window` covers `monitor` edge-to-edge.
///
/// Pure so the fullscreen heuristic is unit-testable.
pub fn covers_monitor(window: &RECT, monitor: &RECT, tolerance: i32) -> bool {
    window.left <= monitor.left + tolerance
        && window.top <= monitor.top + tolerance
        && window.right >= monitor.right - tolerance
        && window.bottom >= monitor.bottom - tolerance
}

/// Reduce an absolute executable path to a stable app id.
///
/// `C:\Users\x\AppData\Roaming\slack\slack.exe` → `slack`
pub fn app_id_from_image_path(path: &str) -> String {
    let path = path.trim_end_matches(['\\', '/']);
    let file_name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let stem = file_name
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(file_name);
    if stem.is_empty() {
        file_name.to_string()
    } else {
        stem.to_lowercase()
    }
}

/// Centre-point helper used by tests and future click-through logic.
pub fn rect_center(rect: &RECT) -> POINT {
    POINT {
        x: (rect.left + rect.right) / 2,
        y: (rect.top + rect.bottom) / 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn fullscreen_requires_edge_to_edge_coverage() {
        let monitor = rect(0, 0, 1920, 1080);

        assert!(covers_monitor(&rect(0, 0, 1920, 1080), &monitor, 2));
        assert!(covers_monitor(&rect(-1, 0, 1920, 1081), &monitor, 2));

        // Maximized window: work area excludes the taskbar.
        assert!(!covers_monitor(&rect(0, 0, 1920, 1040), &monitor, 2));
        // Ordinary window.
        assert!(!covers_monitor(&rect(100, 100, 900, 700), &monitor, 2));
        // Off-by-one beyond tolerance.
        assert!(!covers_monitor(&rect(20, 20, 1920, 1080), &monitor, 2));
    }

    #[test]
    fn app_ids_come_from_image_paths() {
        assert_eq!(
            app_id_from_image_path(r"C:\Users\me\AppData\Roaming\slack\slack.exe"),
            "slack"
        );
        assert_eq!(
            app_id_from_image_path("/usr/bin/code-insiders"),
            "code-insiders"
        );
        assert_eq!(
            app_id_from_image_path("C:\\Program Files\\Discord\\Discord.exe"),
            "discord"
        );
        assert_eq!(app_id_from_image_path("noextension"), "noextension");
        assert_eq!(app_id_from_image_path("trailing\\"), "trailing");
    }

    #[test]
    fn rect_centre_is_midpoint() {
        let center = rect_center(&rect(0, 0, 100, 50));
        assert_eq!((center.x, center.y), (50, 25));
    }
}
