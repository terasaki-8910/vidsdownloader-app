//! Takes the browser out of the webviews (Windows/WebView2): no browser shortcuts such as
//! Ctrl+J, Ctrl+P, Ctrl+F or F5 in any webview, and for the app's own webview also no
//! default context menu, status bar, pinch zoom or swipe navigation. The in-app browser's
//! page offers to save passwords and fills them in again, like Edge; a password stays in
//! the WebView2 data folder it was saved in (the account's own, or the folder for browsing
//! without an account).

/// Which webview `apply` configures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The app's own UI.
    AppUi,
    /// The in-app browser's page, which the user signs in on.
    BrowserPage,
    /// A hidden webview that only reads or writes an account's cookies.
    Helper,
}

#[cfg(windows)]
pub fn apply(webview: &tauri::Webview, role: Role) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2Settings3, ICoreWebView2Settings4, ICoreWebView2Settings5,
        ICoreWebView2Settings6,
    };
    use windows_core::Interface;

    let result = webview.with_webview(move |platform| {
        // SAFETY: the WebView2 COM objects are used on the webview's own thread, which
        // with_webview runs this closure on; the calls only change settings.
        let outcome = unsafe {
            (|| -> windows_core::Result<()> {
                let core = platform.controller().CoreWebView2()?;
                let settings = core.Settings()?;
                settings
                    .cast::<ICoreWebView2Settings3>()?
                    .SetAreBrowserAcceleratorKeysEnabled(false)?;
                match role {
                    Role::AppUi => {
                        settings.SetAreDefaultContextMenusEnabled(false)?;
                        settings.SetIsStatusBarEnabled(false)?;
                        settings
                            .cast::<ICoreWebView2Settings5>()?
                            .SetIsPinchZoomEnabled(false)?;
                        settings
                            .cast::<ICoreWebView2Settings6>()?
                            .SetIsSwipeNavigationEnabled(false)?;
                    }
                    Role::BrowserPage => {
                        let settings = settings.cast::<ICoreWebView2Settings4>()?;
                        settings.SetIsPasswordAutosaveEnabled(true)?;
                        settings.SetIsGeneralAutofillEnabled(true)?;
                    }
                    Role::Helper => {}
                }
                Ok(())
            })()
        };
        if let Err(error) = outcome {
            eprintln!("could not apply webview settings ({role:?}): {error}");
        }
    });
    if let Err(error) = result {
        eprintln!("could not reach a webview's settings: {error}");
    }
}

#[cfg(not(windows))]
pub fn apply(_webview: &tauri::Webview, _role: Role) {}
