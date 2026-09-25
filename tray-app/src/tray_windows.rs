//! Windows tray icon + context menu + global hotkey.
//!
//! Runs on a dedicated thread with a Win32 message pump. Menu selections and
//! hotkeys are forwarded to the UI thread as [`UiCommand`]s.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, MOD_ALT, MOD_CONTROL};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW,
    GetCursorPos, GetMessageW, LoadIconW, RegisterClassW, SetForegroundWindow, TrackPopupMenu,
    TranslateMessage, HMENU, MENU_ITEM_FLAGS, MF_CHECKED, MF_SEPARATOR, MF_STRING, MSG,
    TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WM_APP, WM_COMMAND, WM_HOTKEY, WM_LBUTTONUP, WM_NULL,
    WM_RBUTTONUP, WNDCLASSW, WS_OVERLAPPED,
};

use crate::state::{AppState, Page, UiCommand};

/// Tray callback message id.
const TRAY_CALLBACK: u32 = WM_APP + 1;
/// Global hotkey id (Ctrl+Alt+N).
const HOTKEY_ID: i32 = 1;
const HOTKEY_TOGGLE_VISIBLE: u32 = 100;
const HOTKEY_SHOW_HISTORY: u32 = 101;
const HOTKEY_SHOW_DIGEST: u32 = 102;
const HOTKEY_SHOW_SETTINGS: u32 = 103;
const HOTKEY_CLEAR_PROFILE: u32 = 104;
const HOTKEY_QUIT: u32 = 105;
const HOTKEY_PROFILE_BASE: u32 = 1000;

static SENDER: OnceLock<Sender<UiCommand>> = OnceLock::new();
static STATE: OnceLock<Arc<Mutex<AppState>>> = OnceLock::new();

/// Start the tray on its own thread.
pub fn spawn(handle: crate::state::UiHandle) {
    SENDER.get_or_init(|| handle.command_sender());
    STATE.get_or_init(|| handle.state().clone());

    std::thread::Builder::new()
        .name("tpt-focus-tray".into())
        .spawn(|| unsafe { run() })
        .expect("spawn tray thread");
}

unsafe fn run() {
    let hinstance = GetModuleHandleW(None).expect("module handle");
    let class_name = w!("TptFocusTray");

    let class = WNDCLASSW {
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinstance.into(),
        lpszClassName: class_name,
        hIcon: LoadIconW(None, w!("IDI_APPLICATION")).unwrap_or_default(),
        ..Default::default()
    };
    RegisterClassW(&class);

    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class_name,
        w!("tpt-focus"),
        WS_OVERLAPPED,
        0,
        0,
        0,
        0,
        None,
        None,
        Some(hinstance.into()),
        None,
    )
    .expect("create tray window");

    let mut icon = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: TRAY_CALLBACK,
        hIcon: LoadIconW(None, w!("IDI_APPLICATION")).unwrap_or_default(),
        ..Default::default()
    };
    for (index, slot) in icon
        .szTip
        .iter_mut()
        .take("tpt-focus".len())
        .zip("tpt-focus".chars())
    {
        *index = slot as u16;
    }
    Shell_NotifyIconW(NIM_ADD, &icon)
        .ok()
        .expect("add tray icon");

    // Ctrl+Alt+N toggles the popup.
    if let Err(error) = RegisterHotKey(Some(hwnd), HOTKEY_ID, MOD_CONTROL | MOD_ALT, 0x4E) {
        tracing::warn!(%error, "global hotkey Ctrl+Alt+N unavailable");
    }

    let mut message = MSG::default();
    while GetMessageW(&mut message, None, 0, 0).as_bool() {
        let _ = TranslateMessage(&message);
        DispatchMessageW(&message);
    }

    let _ = Shell_NotifyIconW(NIM_DELETE, &icon);
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        TRAY_CALLBACK => {
            let mouse = (lparam.0 & 0xFFFF) as u32;
            match mouse {
                WM_LBUTTONUP => send(UiCommand::ToggleVisible),
                WM_RBUTTONUP => show_menu(hwnd),
                _ => {}
            }
            LRESULT(0)
        }
        WM_HOTKEY => {
            send(UiCommand::ToggleVisible);
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = wparam.0 as u32;
            handle_menu_id(id);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

fn send(command: UiCommand) {
    if let Some(sender) = SENDER.get() {
        let _ = sender.send(command);
    }
    if let Some(state) = STATE.get() {
        // Wake the UI thread (no-op before the egui context exists).
        let _ = state;
    }
}

unsafe fn show_menu(hwnd: HWND) {
    let Ok(menu) = CreatePopupMenu() else {
        return;
    };

    let state = STATE
        .get()
        .and_then(|state| state.lock().ok())
        .map(|guard| guard.clone());
    let profiles = state
        .as_ref()
        .map(|state| state.profiles.clone())
        .unwrap_or_default();
    let active = state
        .as_ref()
        .and_then(|state| state.active_profile.clone());

    append(
        menu,
        HOTKEY_TOGGLE_VISIBLE,
        "Open / hide popup\tCtrl+Alt+N",
        false,
    );
    append(menu, HOTKEY_SHOW_HISTORY, "History", false);
    append(menu, HOTKEY_SHOW_DIGEST, "Digest", false);
    append(menu, HOTKEY_SHOW_SETTINGS, "Settings", false);
    append(menu, 0, "-", false);
    append(
        menu,
        HOTKEY_CLEAR_PROFILE,
        "normal (no profile)",
        active.is_none(),
    );
    for (index, profile) in profiles.iter().enumerate() {
        let id = HOTKEY_PROFILE_BASE + index as u32;
        let is_active = active
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case(&profile.name));
        append(menu, id, &profile.name, is_active);
    }
    append(menu, 0, "-", false);
    append(menu, HOTKEY_QUIT, "Quit tpt-focus", false);

    let mut cursor = POINT::default();
    let _ = GetCursorPos(&mut cursor);
    let _ = SetForegroundWindow(hwnd);

    let _ = TrackPopupMenu(
        menu,
        TPM_RIGHTBUTTON,
        cursor.x,
        cursor.y,
        Some(0),
        hwnd,
        None,
    );
    let _ = DestroyMenu(menu);
    // Restore the foreground focus quirk after TrackPopupMenu.
    let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
        Some(hwnd),
        WM_NULL,
        WPARAM(0),
        LPARAM(0),
    );
}

unsafe fn append(menu: HMENU, id: u32, label: &str, checked: bool) {
    if id == 0 {
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        return;
    }
    let mut wide: Vec<u16> = label.encode_utf16().collect();
    wide.push(0);
    let mut flags = MENU_ITEM_FLAGS(MF_STRING.0);
    if checked {
        flags |= MENU_ITEM_FLAGS(MF_CHECKED.0);
    }
    let _ = AppendMenuW(
        menu,
        flags,
        id as usize,
        windows::core::PCWSTR(wide.as_ptr()),
    );
}

fn handle_menu_id(id: u32) {
    match id {
        HOTKEY_TOGGLE_VISIBLE => send(UiCommand::ToggleVisible),
        HOTKEY_SHOW_HISTORY => send(UiCommand::Show(Page::History)),
        HOTKEY_SHOW_DIGEST => send(UiCommand::Show(Page::Digest)),
        HOTKEY_SHOW_SETTINGS => send(UiCommand::Show(Page::Settings)),
        HOTKEY_CLEAR_PROFILE => send(UiCommand::ActivateProfile(None)),
        HOTKEY_QUIT => send(UiCommand::Quit),
        base if base >= HOTKEY_PROFILE_BASE => {
            let index = (base - HOTKEY_PROFILE_BASE) as usize;
            let profiles = STATE
                .get()
                .and_then(|state| state.lock().ok())
                .map(|state| state.profiles.clone())
                .unwrap_or_default();
            if let Some(profile) = profiles.get(index) {
                send(UiCommand::ActivateProfile(Some(profile.name.clone())));
            }
        }
        _ => {}
    }
}
