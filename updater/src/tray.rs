//! A tray owned by the application process; no supervisor process or installer UI.
use std::{mem::size_of, ptr::null_mut, sync::OnceLock};
use windows_sys::Win32::{
    Foundation::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{Shell::*, WindowsAndMessaging::*},
};
static PORT: OnceLock<u16> = OnceLock::new();
const CALLBACK: u32 = WM_APP + 1;

pub fn spawn(port: u16) {
    let _ = PORT.set(port);
    std::thread::spawn(move || unsafe {
        let name: Vec<u16> = "GamerTray\0".encode_utf16().collect();
        let instance = GetModuleHandleW(std::ptr::null());
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: name.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&class);
        let hwnd = CreateWindowExW(
            0,
            name.as_ptr(),
            name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return;
        }
        let mut icon = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: CALLBACK,
            hIcon: LoadIconW(null_mut(), IDI_APPLICATION),
            ..std::mem::zeroed()
        };
        for (slot, c) in icon.szTip.iter_mut().zip("Gamer".encode_utf16()) {
            *slot = c;
        }
        Shell_NotifyIconW(NIM_ADD, &icon);
        let mut msg = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        Shell_NotifyIconW(NIM_DELETE, &icon);
    });
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == CALLBACK {
        if l as u32 == WM_LBUTTONDBLCLK {
            crate::portable::open_browser(*PORT.get().unwrap_or(&8443));
        }
        if l as u32 == WM_RBUTTONUP {
            let menu = CreatePopupMenu();
            let open: Vec<_> = "打开 Gamer\0".encode_utf16().collect();
            let exit: Vec<_> = "退出 Gamer\0".encode_utf16().collect();
            AppendMenuW(menu, MF_STRING, 1, open.as_ptr());
            AppendMenuW(menu, MF_STRING, 2, exit.as_ptr());
            let mut point = POINT { x: 0, y: 0 };
            GetCursorPos(&mut point);
            SetForegroundWindow(hwnd);
            let selected = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_NONOTIFY,
                point.x,
                point.y,
                0,
                hwnd,
                std::ptr::null(),
            );
            DestroyMenu(menu);
            if selected == 1 {
                crate::portable::open_browser(*PORT.get().unwrap_or(&8443));
            }
            if selected == 2 {
                std::thread::spawn(|| {
                    if let Some(layout) = crate::portable::layout() {
                        if let Ok(options) = crate::portable::engine_options(&layout) {
                            let port = *PORT.get().unwrap_or(&8443);
                            let _ = crate::upgrade::httpc::http_request(
                                ([127, 0, 0, 1], port).into(),
                                "POST",
                                "/api/shutdown",
                                &[(
                                    "X-Admin-Token",
                                    options.admin_token.as_deref().unwrap_or(""),
                                )],
                                std::time::Duration::from_secs(95),
                            );
                        }
                    }
                });
            }
        }
        return 0;
    }
    DefWindowProcW(hwnd, msg, w, l)
}
