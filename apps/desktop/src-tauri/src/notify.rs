//! Desktop notifications: a download finished, a download failed, a source check queued
//! new items. The first event of a kind opens a 10 s window; the events of that kind
//! arriving inside it are shown together as one notification when it closes, and only if
//! that kind and notifications as a whole are on at that moment and the main window is not
//! in front. Clicking a notification brings the main window forward.
//!
//! Windows shows a WinRT toast under the app's own AppUserModelID; other platforms show
//! nothing yet.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};
use tokio::sync::{broadcast, mpsc};
use vids_core::queue::{JobFailure, JobOutcome, Queue, QueueEvent};
use vids_core::types::{ErrorCategory, JobState};

use crate::models::{Language, NotifySettings};
use crate::state::{AppState, ShellExecutor, lock_recover};

/// How long events of one kind are collected before they are shown.
pub const COALESCE_WINDOW: Duration = Duration::from_secs(10);
/// Longer notification bodies are cut, ending with an ellipsis.
const MAX_BODY_CHARS: usize = 120;
const ELLIPSIS: char = '\u{2026}';
/// The app's AppUserModelID: the bundle identifier, so toasts name the app.
#[cfg_attr(not(windows), allow(dead_code))]
pub const APP_USER_MODEL_ID: &str = "dev.terasaki8910.vidsdownloader";
#[cfg_attr(not(windows), allow(dead_code))]
const DISPLAY_NAME: &str = "vidsdownloader";
const MAIN_WINDOW: &str = "main";

/// What a notification is about; each kind has its own switch and its own window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyKind {
    Done,
    Failed,
    Found,
}

/// One thing worth a notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotifyEvent {
    /// A job finished and saved a file; its title, else its URL.
    Done { title: String },
    /// A job failed; its title, else its URL, and the kind of failure.
    Failed {
        title: String,
        category: ErrorCategory,
    },
    /// A source check queued `count` new items.
    Found {
        source_id: i64,
        name: String,
        count: usize,
    },
}

impl NotifyEvent {
    pub fn kind(&self) -> NotifyKind {
        match self {
            Self::Done { .. } => NotifyKind::Done,
            Self::Failed { .. } => NotifyKind::Failed,
            Self::Found { .. } => NotifyKind::Found,
        }
    }
}

/// The two lines a notification shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub title: String,
    pub body: String,
}

/// The events of one kind collected in one window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    pub kind: NotifyKind,
    pub events: Vec<NotifyEvent>,
}

#[derive(Debug)]
struct Window {
    opened_at: Instant,
    batch: Batch,
}

/// Collects events per kind into windows of [`COALESCE_WINDOW`]. The clock is passed in.
#[derive(Debug, Default)]
pub struct Coalescer {
    open: Vec<Window>,
    closed: Vec<Batch>,
}

impl Coalescer {
    /// Adds an event at `now`: to its kind's open window, else as the first event of a new
    /// one. A window that should have closed by `now` takes no more events.
    pub fn push(&mut self, event: NotifyEvent, now: Instant) {
        self.close_due(now);
        let kind = event.kind();
        match self
            .open
            .iter_mut()
            .find(|window| window.batch.kind == kind)
        {
            Some(window) => window.batch.events.push(event),
            None => self.open.push(Window {
                opened_at: now,
                batch: Batch {
                    kind,
                    events: vec![event],
                },
            }),
        }
    }

    /// When the next open window closes.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.open
            .iter()
            .map(|window| window.opened_at + COALESCE_WINDOW)
            .min()
    }

    /// The batches whose windows have closed by `now`, oldest first.
    pub fn take_due(&mut self, now: Instant) -> Vec<Batch> {
        self.close_due(now);
        std::mem::take(&mut self.closed)
    }

    fn close_due(&mut self, now: Instant) {
        let (due, open): (Vec<Window>, Vec<Window>) = std::mem::take(&mut self.open)
            .into_iter()
            .partition(|window| window.opened_at + COALESCE_WINDOW <= now);
        self.open = open;
        self.closed
            .extend(due.into_iter().map(|window| window.batch));
    }
}

/// Whether a closed window of `kind` is shown, given the switches and whether the main
/// window is in front (visible, not minimized and the foreground window) at that moment.
pub fn wanted(kind: NotifyKind, settings: &NotifySettings, window_in_front: bool) -> bool {
    let kind_on = match kind {
        NotifyKind::Done => settings.done,
        NotifyKind::Failed => settings.failed,
        NotifyKind::Found => settings.found,
    };
    settings.enabled && kind_on && !window_in_front
}

/// The texts of one batch in the UI language; `None` for an empty batch.
pub fn notification(batch: &Batch, language: Language) -> Option<Notification> {
    let ja = language == Language::Ja;
    match batch.kind {
        NotifyKind::Done => {
            let titles: Vec<&str> = batch
                .events
                .iter()
                .filter_map(|event| match event {
                    NotifyEvent::Done { title } => Some(title.as_str()),
                    _ => None,
                })
                .collect();
            let first = titles.first()?;
            let title = if ja {
                "ダウンロードが終わりました"
            } else {
                "Download finished"
            };
            Some(Notification {
                title: title.to_owned(),
                body: fit_body(first, &more_suffix(titles.len() - 1, language)),
            })
        }
        NotifyKind::Failed => {
            let failures: Vec<(&str, ErrorCategory)> = batch
                .events
                .iter()
                .filter_map(|event| match event {
                    NotifyEvent::Failed { title, category } => Some((title.as_str(), *category)),
                    _ => None,
                })
                .collect();
            let (first, category) = failures.first()?;
            let title = if ja {
                "ダウンロードに失敗しました"
            } else {
                "Download failed"
            };
            let suffix = if failures.len() == 1 {
                format!(" \u{b7} {}", error_reason(*category, language))
            } else {
                more_suffix(failures.len() - 1, language)
            };
            Some(Notification {
                title: title.to_owned(),
                body: fit_body(first, &suffix),
            })
        }
        NotifyKind::Found => {
            let mut total = 0usize;
            let mut seen = HashSet::new();
            let mut names = Vec::new();
            for event in &batch.events {
                if let NotifyEvent::Found {
                    source_id,
                    name,
                    count,
                } = event
                {
                    total += count;
                    if seen.insert(*source_id) {
                        names.push(name.as_str());
                    }
                }
            }
            let first = names.first()?;
            let title = match (ja, total) {
                (true, _) => format!("新着 {total} 件"),
                (false, 1) => "1 new item".to_owned(),
                (false, _) => format!("{total} new items"),
            };
            let others = names.len() - 1;
            let suffix = match (ja, others) {
                (_, 0) => String::new(),
                (true, _) => format!(" ほか {others} 件のソース"),
                (false, 1) => " and 1 more source".to_owned(),
                (false, _) => format!(" and {others} more sources"),
            };
            Some(Notification {
                title,
                body: fit_body(first, &suffix),
            })
        }
    }
}

/// 「 ほか N 件」 / " and N more" after the first title; nothing for N = 0.
fn more_suffix(others: usize, language: Language) -> String {
    match (others, language) {
        (0, _) => String::new(),
        (_, Language::Ja) => format!(" ほか {others} 件"),
        (_, Language::En) => format!(" and {others} more"),
    }
}

/// `head` followed by `suffix`, at most [`MAX_BODY_CHARS`] characters: a longer `head` is
/// cut and ends with an ellipsis, so the count or the reason after it stays readable.
fn fit_body(head: &str, suffix: &str) -> String {
    let head_chars = head.chars().count();
    let suffix_chars = suffix.chars().count();
    if head_chars + suffix_chars <= MAX_BODY_CHARS {
        return format!("{head}{suffix}");
    }
    if suffix_chars + 1 >= MAX_BODY_CHARS {
        let mut cut: String = format!("{head}{suffix}")
            .chars()
            .take(MAX_BODY_CHARS - 1)
            .collect();
        cut.push(ELLIPSIS);
        return cut;
    }
    let kept: String = head
        .chars()
        .take(MAX_BODY_CHARS - 1 - suffix_chars)
        .collect();
    format!("{}{ELLIPSIS}{suffix}", kept.trim_end())
}

/// The short reason the UI shows for a failure (the `errors` texts in
/// `apps/desktop/src/i18n/ja.ts` and `en.ts`; change them together).
fn error_reason(category: ErrorCategory, language: Language) -> &'static str {
    match language {
        Language::Ja => match category {
            ErrorCategory::UnsupportedUrl => "対応していない URL です",
            ErrorCategory::PageMediaNotFound => "ページで動画が見つかりませんでした",
            ErrorCategory::PageGate => "確認のページで止まりました",
            ErrorCategory::PageMediaChoice => "動画がいくつか見つかりました",
            ErrorCategory::LoginRequired => "ログインが必要です",
            ErrorCategory::Unavailable => "動画が利用できません",
            ErrorCategory::GeoBlocked => "この地域では視聴できません",
            ErrorCategory::RateLimited => {
                "アクセスが制限されています（しばらく待って再試行してください）"
            }
            ErrorCategory::NoVideo => "動画が見つかりません",
            ErrorCategory::Drm => "DRM で保護されているため取得できません",
            ErrorCategory::ExtractorBroken => "サイトの仕様変更に対応できていません",
            ErrorCategory::Network => "通信エラーが発生しました",
            ErrorCategory::Postprocess => "後処理に失敗しました",
            ErrorCategory::Unknown => "不明なエラーが発生しました",
        },
        Language::En => match category {
            ErrorCategory::UnsupportedUrl => "This URL isn't supported",
            ErrorCategory::PageMediaNotFound => "No video was found on the page",
            ErrorCategory::PageGate => "Stopped at a confirmation page",
            ErrorCategory::PageMediaChoice => "Several videos were found",
            ErrorCategory::LoginRequired => "Login required",
            ErrorCategory::Unavailable => "This video isn't available",
            ErrorCategory::GeoBlocked => "Not available in this region",
            ErrorCategory::RateLimited => "Rate limited -- wait and retry",
            ErrorCategory::NoVideo => "No video found",
            ErrorCategory::Drm => "Protected by DRM; can't be fetched",
            ErrorCategory::ExtractorBroken => "The site's layout changed; not yet supported",
            ErrorCategory::Network => "A network error occurred",
            ErrorCategory::Postprocess => "Post-processing failed",
            ErrorCategory::Unknown => "An unknown error occurred",
        },
    }
}

/// A job's name in a notification: its title, else its URL.
pub fn job_label(title: Option<&str>, url: &str) -> String {
    title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or(url)
        .to_owned()
}

/// The notification a finished attempt is worth: `Done` only when it saved a file (not a
/// skip such as "already saved", not a list job that only queued its children), `Failed`
/// for a failure (the queue never retries on its own, so `Failed` is final). Canceled jobs
/// are worth none.
pub fn finished_event(
    state: JobState,
    result: Option<&Result<JobOutcome, JobFailure>>,
    label: String,
) -> Option<NotifyEvent> {
    match (state, result) {
        (JobState::Done, Some(Ok(outcome))) if outcome.output_path.is_some() => {
            Some(NotifyEvent::Done { title: label })
        }
        (JobState::Failed, Some(Err(failure))) => Some(NotifyEvent::Failed {
            title: label,
            category: failure.category,
        }),
        _ => None,
    }
}

/// The toast's XML: the generic template with the two lines. Characters XML 1.0 does not
/// allow are dropped.
#[cfg_attr(not(windows), allow(dead_code))]
fn toast_xml(title: &str, body: &str) -> String {
    format!(
        "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text>\
         </binding></visual></toast>",
        xml_text(title),
        xml_text(body)
    )
}

#[cfg_attr(not(windows), allow(dead_code))]
fn xml_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(ch),
            ch if ch < ' ' || matches!(ch, '\u{fffe}' | '\u{ffff}') => {}
            ch => out.push(ch),
        }
    }
    out
}

/// Sends events to the coalescing task. Sending never blocks; clones share the task.
#[derive(Debug, Clone)]
pub struct Notifier {
    sender: mpsc::UnboundedSender<NotifyEvent>,
}

impl Notifier {
    /// Starts the task that collects events and shows the notifications.
    pub fn start(app: AppHandle) -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        tauri::async_runtime::spawn(run(app, receiver));
        Self { sender }
    }

    pub fn send(&self, event: NotifyEvent) {
        if self.sender.send(event).is_err() {
            eprintln!("notifications: the notification task has stopped");
        }
    }
}

async fn run(app: AppHandle, mut events: mpsc::UnboundedReceiver<NotifyEvent>) {
    let shower = platform::Shower::start(app.clone());
    let mut coalescer = Coalescer::default();
    loop {
        let deadline = coalescer.next_deadline();
        tokio::select! {
            received = events.recv() => match received {
                Some(event) => coalescer.push(event, Instant::now()),
                None => break,
            },
            () = sleep_until(deadline) => {
                for batch in coalescer.take_due(Instant::now()) {
                    show_batch(&app, &shower, &batch);
                }
            }
        }
    }
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await,
        None => std::future::pending().await,
    }
}

fn show_batch(app: &AppHandle, shower: &platform::Shower, batch: &Batch) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let (language, switches) = {
        let settings = lock_recover(&state.settings);
        (settings.language, settings.notify)
    };
    // The window is asked only when the switches would show the batch.
    if !wanted(batch.kind, &switches, false) || main_window_in_front(app) {
        return;
    }
    if let Some(notification) = notification(batch, language) {
        shower.show(notification);
    }
}

/// Feeds finished and failed jobs to `notifier`, next to the UI's own queue events.
pub async fn watch_queue(
    queue: Queue<ShellExecutor>,
    notifier: Notifier,
    mut events: broadcast::Receiver<QueueEvent>,
) {
    loop {
        match events.recv().await {
            Ok(QueueEvent::Finished {
                id,
                state,
                outcome_or_error,
            }) => {
                if !matches!(state, JobState::Done | JobState::Failed) {
                    continue;
                }
                let Some(job) = queue.snapshot().into_iter().find(|job| job.id == id) else {
                    continue;
                };
                let title = job
                    .details
                    .as_ref()
                    .and_then(|details| details.title.as_deref());
                let label = job_label(title, &job.url);
                if let Some(event) = finished_event(state, outcome_or_error.as_ref(), label) {
                    notifier.send(event);
                }
            }
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                eprintln!("notifications: {missed} queue events were missed");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

/// Whether the main window is in front: visible, not minimized, and the foreground window
/// (or the owner of the foreground dialog).
fn main_window_in_front(app: &AppHandle) -> bool {
    let Some(window) = app.get_window(MAIN_WINDOW) else {
        return false;
    };
    if !window.is_visible().unwrap_or(false) || window.is_minimized().unwrap_or(true) {
        return false;
    }
    platform::is_foreground(&window)
}

/// Shows, unminimizes and focuses the main window (a click on a notification).
#[cfg_attr(not(windows), allow(dead_code))]
fn bring_forward(app: &AppHandle) {
    let Some(window) = app.get_window(MAIN_WINDOW) else {
        return;
    };
    for (step, result) in [
        ("show", window.show()),
        ("unminimize", window.unminimize()),
        ("focus", window.set_focus()),
    ] {
        if let Err(error) = result {
            eprintln!("notifications: could not {step} the main window: {error}");
        }
    }
}

/// Gives the process the app's AppUserModelID, registered under the current user with the
/// app's name, so toasts from the unpackaged exe are shown and named. Call before any
/// window exists.
pub fn register_app_id() {
    platform::register_app_id();
}

#[cfg(windows)]
mod platform {
    use std::collections::VecDeque;
    use std::sync::mpsc::{self, Receiver, Sender};

    use tauri::AppHandle;
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::{
        ToastFailedEventArgs, ToastNotification, ToastNotificationManager, ToastNotifier,
    };
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
        RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegGetValueW, RegSetValueExW,
    };
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    use windows::Win32::UI::WindowsAndMessaging::{GA_ROOTOWNER, GetAncestor, GetForegroundWindow};
    use windows::core::{HSTRING, IInspectable, PCWSTR, w};

    use super::{APP_USER_MODEL_ID, DISPLAY_NAME, Notification, bring_forward, toast_xml};

    /// Toasts kept alive after showing, so a click on a recent one still reaches its
    /// handler.
    const KEPT_TOASTS: usize = 20;

    /// Shows toasts on a thread of its own, in the multithreaded COM apartment.
    pub struct Shower {
        sender: Option<Sender<Notification>>,
    }

    impl Shower {
        pub fn start(app: AppHandle) -> Self {
            let (sender, receiver) = mpsc::channel();
            let spawned = std::thread::Builder::new()
                .name("notifications".to_owned())
                .spawn(move || toast_thread(&app, &receiver));
            match spawned {
                Ok(_) => Self {
                    sender: Some(sender),
                },
                Err(error) => {
                    eprintln!("notifications: could not start the toast thread: {error}");
                    Self { sender: None }
                }
            }
        }

        pub fn show(&self, notification: Notification) {
            let sent = self
                .sender
                .as_ref()
                .is_some_and(|sender| sender.send(notification).is_ok());
            if !sent {
                eprintln!("notifications: the toast thread is not running");
            }
        }
    }

    fn toast_thread(app: &AppHandle, receiver: &Receiver<Notification>) {
        // SAFETY: the thread is new, so it joins the multithreaded apartment here and
        // leaves it only when it ends, after its last COM object has been released.
        let initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        {
            let mut notifier: Option<ToastNotifier> = None;
            let mut kept: VecDeque<ToastNotification> = VecDeque::new();
            for notification in receiver {
                match show_toast(app, &mut notifier, &notification) {
                    Ok(toast) => {
                        kept.push_back(toast);
                        if kept.len() > KEPT_TOASTS {
                            kept.pop_front();
                        }
                    }
                    Err(error) => eprintln!("notifications: could not show a toast: {error}"),
                }
            }
        }
        if initialized {
            // SAFETY: balances the successful CoInitializeEx above, on the same thread.
            unsafe { CoUninitialize() };
        }
    }

    fn show_toast(
        app: &AppHandle,
        notifier: &mut Option<ToastNotifier>,
        notification: &Notification,
    ) -> windows::core::Result<ToastNotification> {
        let xml = XmlDocument::new()?;
        xml.LoadXml(&HSTRING::from(toast_xml(
            &notification.title,
            &notification.body,
        )))?;
        let toast = ToastNotification::CreateToastNotification(&xml)?;
        let app = app.clone();
        toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(
            move |_, _| {
                bring_forward(&app);
                Ok(())
            },
        ))?;
        toast.Failed(
            &TypedEventHandler::<ToastNotification, ToastFailedEventArgs>::new(|_, args| {
                let code = args.ok().and_then(ToastFailedEventArgs::ErrorCode);
                eprintln!("notifications: a toast could not be shown: {code:?}");
                Ok(())
            }),
        )?;
        if notifier.is_none() {
            *notifier = Some(ToastNotificationManager::CreateToastNotifierWithId(
                &HSTRING::from(APP_USER_MODEL_ID),
            )?);
        }
        if let Some(notifier) = notifier {
            notifier.Show(&toast)?;
        }
        Ok(toast)
    }

    pub fn is_foreground(window: &tauri::Window) -> bool {
        let Ok(main) = window.hwnd() else {
            return false;
        };
        // SAFETY: both calls only read window handles; a stale or null handle makes them
        // return null, which never equals the main window.
        let root = unsafe {
            let foreground = GetForegroundWindow();
            if foreground.is_invalid() {
                return false;
            }
            GetAncestor(foreground, GA_ROOTOWNER)
        };
        !root.is_invalid() && root.0 as isize == main.0 as isize
    }

    pub fn register_app_id() {
        if let Err(error) = register_display_name() {
            eprintln!("notifications: could not register the app's name for toasts: {error}");
        }
        // SAFETY: the argument is a NUL-terminated string that outlives the call.
        let result =
            unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(APP_USER_MODEL_ID)) };
        if let Err(error) = result {
            eprintln!("notifications: could not set the AppUserModelID: {error}");
        }
    }

    /// `HKCU\Software\Classes\AppUserModelId\<id>`, `DisplayName` = the app's name; written
    /// only when it is missing or different.
    fn register_display_name() -> windows::core::Result<()> {
        let subkey = HSTRING::from(format!(
            "Software\\Classes\\AppUserModelId\\{APP_USER_MODEL_ID}"
        ));
        let mut key = HKEY::default();
        // SAFETY: `subkey` is NUL-terminated and outlives the call; `key` receives the
        // opened handle, closed below.
        unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                &subkey,
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_QUERY_VALUE | KEY_SET_VALUE,
                None,
                &mut key,
                None,
            )
        }
        .ok()?;
        let result = if read_display_name(key).as_deref() == Some(DISPLAY_NAME) {
            Ok(())
        } else {
            let data: Vec<u8> = DISPLAY_NAME
                .encode_utf16()
                .chain(std::iter::once(0))
                .flat_map(u16::to_le_bytes)
                .collect();
            // SAFETY: `key` is open with KEY_SET_VALUE; `data` is a NUL-terminated UTF-16
            // string, as REG_SZ requires.
            unsafe { RegSetValueExW(key, w!("DisplayName"), None, REG_SZ, Some(&data)) }.ok()
        };
        // SAFETY: `key` was opened above and is not used afterwards.
        let _ = unsafe { RegCloseKey(key) };
        result
    }

    fn read_display_name(key: HKEY) -> Option<String> {
        let mut buffer = [0u16; 256];
        let mut size = u32::try_from(std::mem::size_of_val(&buffer)).ok()?;
        // SAFETY: `buffer` is writable for `size` bytes, and RegGetValueW NUL-terminates
        // what it writes there; a longer value fails with ERROR_MORE_DATA instead.
        unsafe {
            RegGetValueW(
                key,
                PCWSTR::null(),
                w!("DisplayName"),
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut size),
            )
        }
        .ok()
        .ok()?;
        let len = buffer.iter().position(|&unit| unit == 0)?;
        String::from_utf16(&buffer[..len]).ok()
    }
}

#[cfg(not(windows))]
mod platform {
    use tauri::AppHandle;

    use super::Notification;

    /// Nothing is shown outside Windows yet.
    pub struct Shower;

    impl Shower {
        pub fn start(_app: AppHandle) -> Self {
            Self
        }

        pub fn show(&self, _notification: Notification) {}
    }

    pub fn is_foreground(window: &tauri::Window) -> bool {
        window.is_focused().unwrap_or(false)
    }

    pub fn register_app_id() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(title: &str) -> NotifyEvent {
        NotifyEvent::Done {
            title: title.to_owned(),
        }
    }

    fn failed(title: &str, category: ErrorCategory) -> NotifyEvent {
        NotifyEvent::Failed {
            title: title.to_owned(),
            category,
        }
    }

    fn found(source_id: i64, name: &str, count: usize) -> NotifyEvent {
        NotifyEvent::Found {
            source_id,
            name: name.to_owned(),
            count,
        }
    }

    fn batch(events: Vec<NotifyEvent>) -> Batch {
        Batch {
            kind: events[0].kind(),
            events,
        }
    }

    fn texts(events: Vec<NotifyEvent>, language: Language) -> (String, String) {
        let notification = notification(&batch(events), language).unwrap();
        (notification.title, notification.body)
    }

    fn pair(title: &str, body: &str) -> (String, String) {
        (title.to_owned(), body.to_owned())
    }

    #[test]
    fn a_finished_download_is_named_by_its_title() {
        assert_eq!(
            texts(vec![done("Big Buck Bunny")], Language::Ja),
            pair("ダウンロードが終わりました", "Big Buck Bunny")
        );
        assert_eq!(
            texts(vec![done("Big Buck Bunny")], Language::En),
            pair("Download finished", "Big Buck Bunny")
        );
    }

    #[test]
    fn several_finished_downloads_name_the_first_and_count_the_rest() {
        let events = vec![
            done("Sintel"),
            done("Tears of Steel"),
            done("Cosmos Laundromat"),
        ];
        assert_eq!(
            texts(events.clone(), Language::Ja),
            pair("ダウンロードが終わりました", "Sintel ほか 2 件")
        );
        assert_eq!(
            texts(events, Language::En),
            pair("Download finished", "Sintel and 2 more")
        );
    }

    #[test]
    fn a_failure_gives_the_title_and_the_reason_of_its_category() {
        assert_eq!(
            texts(
                vec![failed("Members only", ErrorCategory::LoginRequired)],
                Language::Ja
            ),
            pair(
                "ダウンロードに失敗しました",
                "Members only \u{b7} ログインが必要です"
            )
        );
        assert_eq!(
            texts(
                vec![failed("Members only", ErrorCategory::LoginRequired)],
                Language::En
            ),
            pair("Download failed", "Members only \u{b7} Login required")
        );
    }

    #[test]
    fn several_failures_name_the_first_and_count_the_rest() {
        let events = vec![
            failed("A", ErrorCategory::Network),
            failed("B", ErrorCategory::Drm),
        ];
        assert_eq!(
            texts(events.clone(), Language::Ja),
            pair("ダウンロードに失敗しました", "A ほか 1 件")
        );
        assert_eq!(
            texts(events, Language::En),
            pair("Download failed", "A and 1 more")
        );
    }

    #[test]
    fn new_items_count_every_item_and_name_the_first_source() {
        assert_eq!(
            texts(vec![found(1, "Blender Studio", 3)], Language::Ja),
            pair("新着 3 件", "Blender Studio")
        );
        assert_eq!(
            texts(vec![found(1, "Blender Studio", 3)], Language::En),
            pair("3 new items", "Blender Studio")
        );
        assert_eq!(
            texts(vec![found(1, "Blender Studio", 1)], Language::En),
            pair("1 new item", "Blender Studio")
        );

        let two = vec![found(1, "Blender Studio", 3), found(2, "NASA", 2)];
        assert_eq!(
            texts(two.clone(), Language::Ja),
            pair("新着 5 件", "Blender Studio ほか 1 件のソース")
        );
        assert_eq!(
            texts(two, Language::En),
            pair("5 new items", "Blender Studio and 1 more source")
        );

        // A source checked twice in one window counts once among the sources.
        let three = vec![
            found(1, "Blender Studio", 1),
            found(2, "NASA", 2),
            found(1, "Blender Studio", 1),
            found(3, "ESA", 4),
        ];
        assert_eq!(
            texts(three.clone(), Language::Ja),
            pair("新着 8 件", "Blender Studio ほか 2 件のソース")
        );
        assert_eq!(
            texts(three, Language::En),
            pair("8 new items", "Blender Studio and 2 more sources")
        );
    }

    #[test]
    fn bodies_longer_than_120_characters_are_cut_with_an_ellipsis() {
        let long = "長".repeat(200);
        let (_, body) = texts(vec![done(&long)], Language::Ja);
        assert_eq!(body.chars().count(), 120);
        assert!(body.ends_with('\u{2026}'));
        assert_eq!(body, format!("{}\u{2026}", "長".repeat(119)));

        // The count and the reason after a long title stay whole.
        let (_, body) = texts(vec![done(&long), done("x")], Language::Ja);
        assert_eq!(body.chars().count(), 120);
        assert!(body.ends_with("\u{2026} ほか 1 件"));
        let (_, body) = texts(
            vec![failed(&"a".repeat(130), ErrorCategory::Network)],
            Language::En,
        );
        assert_eq!(body.chars().count(), 120);
        assert!(body.ends_with("a\u{2026} \u{b7} A network error occurred"));

        // Exactly 120 characters are left alone.
        let exact = "b".repeat(120);
        assert_eq!(texts(vec![done(&exact)], Language::En).1, exact);
    }

    #[test]
    fn events_three_seconds_apart_make_one_notification() {
        let start = Instant::now();
        let mut coalescer = Coalescer::default();
        coalescer.push(done("A"), start);
        coalescer.push(done("B"), start + Duration::from_secs(3));
        assert_eq!(coalescer.next_deadline(), Some(start + COALESCE_WINDOW));
        assert!(
            coalescer
                .take_due(start + Duration::from_millis(9_999))
                .is_empty()
        );
        assert_eq!(
            coalescer.take_due(start + COALESCE_WINDOW),
            vec![batch(vec![done("A"), done("B")])]
        );
        assert_eq!(coalescer.next_deadline(), None);
        assert!(
            coalescer
                .take_due(start + Duration::from_secs(30))
                .is_empty()
        );
    }

    #[test]
    fn events_twelve_seconds_apart_make_two_notifications() {
        let start = Instant::now();
        let mut coalescer = Coalescer::default();
        coalescer.push(done("A"), start);
        assert_eq!(
            coalescer.take_due(start + COALESCE_WINDOW),
            vec![batch(vec![done("A")])]
        );
        coalescer.push(done("B"), start + Duration::from_secs(12));
        assert_eq!(
            coalescer.take_due(start + Duration::from_secs(22)),
            vec![batch(vec![done("B")])]
        );

        // Also when nothing took the first window before the second event came.
        let mut coalescer = Coalescer::default();
        coalescer.push(done("A"), start);
        coalescer.push(done("B"), start + Duration::from_secs(12));
        assert_eq!(
            coalescer.take_due(start + Duration::from_secs(12)),
            vec![batch(vec![done("A")])]
        );
        assert_eq!(
            coalescer.next_deadline(),
            Some(start + Duration::from_secs(22))
        );
        assert_eq!(
            coalescer.take_due(start + Duration::from_secs(22)),
            vec![batch(vec![done("B")])]
        );
    }

    #[test]
    fn each_kind_has_its_own_window() {
        let start = Instant::now();
        let mut coalescer = Coalescer::default();
        coalescer.push(done("A"), start);
        coalescer.push(
            failed("B", ErrorCategory::Network),
            start + Duration::from_secs(4),
        );
        coalescer.push(done("C"), start + Duration::from_secs(5));
        assert_eq!(
            coalescer.take_due(start + COALESCE_WINDOW),
            vec![batch(vec![done("A"), done("C")])]
        );
        assert_eq!(
            coalescer.next_deadline(),
            Some(start + Duration::from_secs(14))
        );
        assert_eq!(
            coalescer.take_due(start + Duration::from_secs(14)),
            vec![batch(vec![failed("B", ErrorCategory::Network)])]
        );
    }

    #[test]
    fn switches_and_a_window_in_front_decide_what_is_shown() {
        let all_on = NotifySettings::default();
        for kind in [NotifyKind::Done, NotifyKind::Failed, NotifyKind::Found] {
            assert!(wanted(kind, &all_on, false));
            // The main window in front: nothing.
            assert!(!wanted(kind, &all_on, true));
            // Everything off: nothing, whatever the kinds say.
            let off = NotifySettings {
                enabled: false,
                ..all_on
            };
            assert!(!wanted(kind, &off, false));
        }
        let no_done = NotifySettings {
            done: false,
            ..all_on
        };
        assert!(!wanted(NotifyKind::Done, &no_done, false));
        assert!(wanted(NotifyKind::Failed, &no_done, false));
        assert!(wanted(NotifyKind::Found, &no_done, false));
        let no_failed = NotifySettings {
            failed: false,
            ..all_on
        };
        assert!(!wanted(NotifyKind::Failed, &no_failed, false));
        let no_found = NotifySettings {
            found: false,
            ..all_on
        };
        assert!(!wanted(NotifyKind::Found, &no_found, false));
        assert!(wanted(NotifyKind::Done, &no_found, false));
    }

    #[test]
    fn only_saved_files_and_failures_are_worth_a_notification() {
        let label = || "Title".to_owned();
        let saved = JobOutcome {
            output_path: Some("C:\\Videos\\a.mp4".to_owned()),
            ..JobOutcome::default()
        };
        assert_eq!(
            finished_event(JobState::Done, Some(&Ok(saved)), label()),
            Some(done("Title"))
        );
        // Page images that saved some of the files still saved files.
        let partly = JobOutcome {
            output_path: Some("C:\\Videos\\1.jpg".to_owned()),
            skipped: Some("1 of 3 images could not be saved".to_owned()),
            ..JobOutcome::default()
        };
        assert_eq!(
            finished_event(JobState::Done, Some(&Ok(partly)), label()),
            Some(done("Title"))
        );
        let skipped = JobOutcome {
            skipped: Some("already saved".to_owned()),
            ..JobOutcome::default()
        };
        assert_eq!(
            finished_event(JobState::Done, Some(&Ok(skipped)), label()),
            None
        );
        let list = JobOutcome {
            entries: vec![vids_core::queue::ListEntry {
                url: "https://example.com/child".to_owned(),
                playlist_item: Some(1),
                title: None,
            }],
            list_total: Some(1),
            ..JobOutcome::default()
        };
        assert_eq!(
            finished_event(JobState::Done, Some(&Ok(list)), label()),
            None
        );
        assert_eq!(finished_event(JobState::Canceled, None, label()), None);
        let failure = JobFailure {
            category: ErrorCategory::GeoBlocked,
            message: "not in your country".to_owned(),
        };
        assert_eq!(
            finished_event(JobState::Failed, Some(&Err(failure)), label()),
            Some(failed("Title", ErrorCategory::GeoBlocked))
        );
    }

    #[test]
    fn a_job_without_a_title_is_named_by_its_url() {
        let url = "https://example.com/watch?v=1";
        assert_eq!(job_label(Some(" Sintel "), url), "Sintel");
        assert_eq!(job_label(Some("  "), url), url);
        assert_eq!(job_label(None, url), url);
    }

    #[test]
    fn reasons_are_the_texts_the_ui_shows() {
        let ja = include_str!("../../src/i18n/ja.ts");
        let en = include_str!("../../src/i18n/en.ts");
        for (category, name) in [
            (ErrorCategory::UnsupportedUrl, "UnsupportedUrl"),
            (ErrorCategory::PageMediaNotFound, "PageMediaNotFound"),
            (ErrorCategory::PageGate, "PageGate"),
            (ErrorCategory::PageMediaChoice, "PageMediaChoice"),
            (ErrorCategory::LoginRequired, "LoginRequired"),
            (ErrorCategory::Unavailable, "Unavailable"),
            (ErrorCategory::GeoBlocked, "GeoBlocked"),
            (ErrorCategory::RateLimited, "RateLimited"),
            (ErrorCategory::NoVideo, "NoVideo"),
            (ErrorCategory::Drm, "Drm"),
            (ErrorCategory::ExtractorBroken, "ExtractorBroken"),
            (ErrorCategory::Network, "Network"),
            (ErrorCategory::Postprocess, "Postprocess"),
            (ErrorCategory::Unknown, "Unknown"),
        ] {
            let ja_line = format!("{name}: \"{}\",", error_reason(category, Language::Ja));
            let en_line = format!("{name}: \"{}\",", error_reason(category, Language::En));
            assert!(ja.contains(&ja_line), "ja.ts lacks {ja_line}");
            assert!(en.contains(&en_line), "en.ts lacks {en_line}");
        }
    }

    #[test]
    fn toast_text_is_escaped_for_xml() {
        assert_eq!(
            toast_xml("Download finished", "Tom & Jerry <1> \"x\" 'y'\u{1}"),
            "<toast><visual><binding template=\"ToastGeneric\"><text>Download finished</text>\
             <text>Tom &amp; Jerry &lt;1&gt; &quot;x&quot; &apos;y&apos;</text></binding>\
             </visual></toast>"
        );
    }
}
