//! The hidden YouTube Music engine: a WebView2 in its own top-level window.
//!
//! Everything here runs on the UI thread. WebView2 creates the environment,
//! controller and handlers asynchronously, and their callbacks arrive through the
//! normal message loop, so nothing blocks. Messages from the page are queued and
//! announced with `Event::Music`; the app then calls [`Engine::poll`].

use crate::bridge::BRIDGE_JS;
use dj_core::music::MusicMsg;
use dj_win::window::{self, Event};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::Path;
use std::rc::Rc;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2Controller, ICoreWebView2Environment,
    ICoreWebView2EnvironmentOptions,
};
use webview2_com::{
    AddScriptToExecuteOnDocumentCreatedCompletedHandler, CoTaskMemPWSTR,
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    CoreWebView2EnvironmentOptions, ExecuteScriptCompletedHandler, NavigationCompletedEventHandler,
    WebMessageReceivedEventHandler,
};
use windows::core::{w, Result as WinResult, PCWSTR, PWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetSystemMetrics,
    RegisterClassExW, SetForegroundWindow, ShowWindow, SM_CXSCREEN, SM_CYSCREEN, SW_HIDE, SW_SHOW,
    WM_CLOSE, WM_SIZE, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
};

pub const MUSIC_URL: &str = "https://music.youtube.com";
const TITLE: PCWSTR = w!("Daijobu Island \u{b7} YouTube Music");
const CLASS: PCWSTR = w!("DaijobuMusicWindow");
const WIN_W: i32 = 1100;
const WIN_H: i32 = 760;
const BROWSER_ARGS: &str = "--autoplay-policy=no-user-gesture-required";

thread_local! {
    /// The controller, for the window procedure's resize handling. UI thread only.
    static CONTROLLER: RefCell<Option<ICoreWebView2Controller>> = const { RefCell::new(None) };
}

#[derive(Default)]
struct Shared {
    /// Kept alive for as long as the controller exists.
    env: Option<ICoreWebView2Environment>,
    web: Option<ICoreWebView2>,
    queue: VecDeque<MusicMsg>,
    /// Last value reported by the Google block-page check.
    blocked: bool,
}

/// A running engine. Dropping it closes the page and destroys its window.
pub struct Engine {
    hwnd: HWND,
    shared: Rc<RefCell<Shared>>,
}

impl Engine {
    /// Creates the hidden window and starts WebView2 with the profile in `user_data`.
    /// `show` makes the window visible straight away (the sign-in case).
    pub fn start(user_data: &Path, show: bool) -> Result<Self, String> {
        // SAFETY: COM must be initialised on the UI thread before WebView2 is used.
        // S_FALSE (already initialised) and RPC_E_CHANGED_MODE are both harmless here.
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let hwnd = create_window(show).map_err(|e| format!("window: {e}"))?;
        let shared = Rc::new(RefCell::new(Shared::default()));

        let options = CoreWebView2EnvironmentOptions::default();
        // SAFETY: the options object is created above and not shared yet.
        unsafe { options.set_additional_browser_arguments(BROWSER_ARGS.to_string()) };
        let options: ICoreWebView2EnvironmentOptions = options.into();
        let data = wide(&user_data.to_string_lossy());

        let s = shared.clone();
        let env_done = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
            move |hr: WinResult<()>, env: Option<ICoreWebView2Environment>| {
                hr?;
                let Some(env) = env else { return Ok(()) };
                s.borrow_mut().env = Some(env.clone());
                let s2 = s.clone();
                let ctl_done = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
                    move |hr: WinResult<()>, ctl: Option<ICoreWebView2Controller>| {
                        hr?;
                        if let Some(ctl) = ctl {
                            on_controller(&s2, ctl, hwnd);
                        }
                        Ok(())
                    },
                ));
                // SAFETY: `hwnd` is our live window; the handler outlives the call.
                unsafe { env.CreateCoreWebView2Controller(hwnd, &ctl_done) }?;
                Ok(())
            },
        ));
        // SAFETY: the folder and option strings outlive the call; the handler is
        // owned by WebView2 once passed.
        unsafe {
            webview2_com::Microsoft::Web::WebView2::Win32::CreateCoreWebView2EnvironmentWithOptions(
                PCWSTR::null(),
                PCWSTR(data.as_ptr()),
                &options,
                &env_done,
            )
        }
        .map_err(|e| format!("WebView2 environment: {e}"))?;

        // The page loads later, when the async callbacks run on the message loop.
        Ok(Self { hwnd, shared })
    }

    /// Shows or hides the window. Showing also brings it to the front.
    pub fn show(&self, visible: bool) {
        // SAFETY: valid window handle owned by this thread.
        unsafe {
            let _ = ShowWindow(self.hwnd, if visible { SW_SHOW } else { SW_HIDE });
            if visible {
                let _ = SetForegroundWindow(self.hwnd);
            }
        }
    }

    pub fn hwnd_raw(&self) -> isize {
        self.hwnd.0 as isize
    }

    /// Takes every message queued since the last call.
    pub fn poll(&self) -> Vec<MusicMsg> {
        self.shared.borrow_mut().queue.drain(..).collect()
    }

    /// Runs a script in the page (fire and forget). Ignored until the page is ready.
    pub fn exec(&self, js: &str) {
        let Some(web) = self.shared.borrow().web.clone() else {
            return;
        };
        let js = wide(js);
        let done = ExecuteScriptCompletedHandler::create(Box::new(|_: WinResult<()>, _| Ok(())));
        // SAFETY: `js` outlives the call and the handler is owned by WebView2 afterwards.
        if let Err(e) = unsafe { web.ExecuteScript(PCWSTR(js.as_ptr()), &done) } {
            dj_win::system::log(&format!("music: ExecuteScript: {e}"));
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        CONTROLLER.with(|c| {
            if let Some(ctl) = c.borrow_mut().take() {
                // SAFETY: the controller is ours and still valid until closed here.
                let _ = unsafe { ctl.Close() };
            }
        });
        self.shared.borrow_mut().web = None;
        // SAFETY: the window is ours; destroying it tears down the page's host window.
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn on_controller(shared: &Rc<RefCell<Shared>>, ctl: ICoreWebView2Controller, hwnd: HWND) {
    if let Err(e) = setup_controller(shared, ctl, hwnd) {
        dj_win::system::log(&format!("music: controller setup: {e}"));
    }
}

fn setup_controller(
    shared: &Rc<RefCell<Shared>>,
    ctl: ICoreWebView2Controller,
    hwnd: HWND,
) -> WinResult<()> {
    // SAFETY: valid controller and window; the rectangle is ours.
    unsafe {
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        ctl.SetBounds(rc)?;
        ctl.SetIsVisible(true)?;
        let web = ctl.CoreWebView2()?;
        web.Settings()?.SetAreDevToolsEnabled(false)?;
        CONTROLLER.with(|c| *c.borrow_mut() = Some(ctl.clone()));

        // Runs before the page's own scripts, on every document.
        let js = wide(BRIDGE_JS);
        let added = AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(
            |_: WinResult<()>, _| Ok(()),
        ));
        web.AddScriptToExecuteOnDocumentCreated(PCWSTR(js.as_ptr()), &added)?;

        let s = shared.clone();
        let mut token = 0;
        web.add_WebMessageReceived(
            &WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    let mut raw = PWSTR::null();
                    if args.WebMessageAsJson(&mut raw).is_ok() {
                        let text = CoTaskMemPWSTR::from(raw).to_string();
                        if let Some(msg) = MusicMsg::parse(&text) {
                            s.borrow_mut().queue.push_back(msg);
                            window::push_event(Event::Music);
                        }
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;

        let s = shared.clone();
        let mut token = 0;
        web.add_NavigationCompleted(
            &NavigationCompletedEventHandler::create(Box::new(move |sender, _| {
                if let Some(web) = sender {
                    let blocked = is_google_block_page(&web);
                    let mut st = s.borrow_mut();
                    if st.blocked != blocked {
                        st.blocked = blocked;
                        st.queue.push_back(MusicMsg::Blocked(blocked));
                        window::push_event(Event::Music);
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;

        shared.borrow_mut().web = Some(web.clone());
        let url = wide(MUSIC_URL);
        web.Navigate(PCWSTR(url.as_ptr()))?;
        Ok(())
    }
}

/// Google's "this browser may not be secure" page. The URL is the reliable signal;
/// the title is checked too because Google keeps the host the same across variants.
/// The exact title text is not verified here, so this may miss a variant.
fn is_google_block_page(web: &ICoreWebView2) -> bool {
    // SAFETY: plain getters on a live WebView2; the returned strings are freed by
    // `CoTaskMemPWSTR`.
    unsafe {
        let mut src = PWSTR::null();
        let mut title = PWSTR::null();
        if web.Source(&mut src).is_err() || web.DocumentTitle(&mut title).is_err() {
            return false;
        }
        let url = CoTaskMemPWSTR::from(src).to_string();
        let title = CoTaskMemPWSTR::from(title).to_string().to_lowercase();
        url.starts_with("https://accounts.google.com/") && title.contains("couldn")
    }
}

fn create_window(show: bool) -> WinResult<HWND> {
    // SAFETY: standard class registration and window creation. Pointers are valid for
    // the duration of each call and the class/title strings are static.
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        // Fails harmlessly on the second engine start: the class already exists.
        RegisterClassExW(&wc);
        let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        let hwnd = CreateWindowExW(
            Default::default(),
            CLASS,
            TITLE,
            WS_OVERLAPPEDWINDOW,
            (sw - WIN_W) / 2,
            (sh - WIN_H) / 2,
            WIN_W,
            WIN_H,
            None,
            None,
            Some(hinst.into()),
            None,
        )?;
        let _ = ShowWindow(hwnd, if show { SW_SHOW } else { SW_HIDE });
        Ok(hwnd)
    }
}

/// The engine window: resizes the page with the window and hides on close instead of
/// destroying it, so the page keeps playing.
extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_SIZE => {
            CONTROLLER.with(|c| {
                if let Some(ctl) = c.borrow().as_ref() {
                    let mut rc = RECT::default();
                    // SAFETY: valid window owned by this thread.
                    if unsafe { GetClientRect(hwnd, &mut rc) }.is_ok() {
                        // SAFETY: valid controller on this thread.
                        let _ = unsafe { ctl.SetBounds(rc) };
                    }
                }
            });
            LRESULT(0)
        }
        WM_CLOSE => {
            // SAFETY: valid window handle.
            unsafe {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            LRESULT(0)
        }
        // SAFETY: forwards the unhandled message with its original arguments.
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
