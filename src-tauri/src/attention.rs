//! Calling the user back to the app when something waits for them (an MCP
//! request set to Ask), when the window couldn't simply take the focus.
//!
//! On Wayland a window can't raise itself: the compositor only activates
//! a window that brings an activation token, which the desktop hands out
//! for a user action. So on Linux the app shows a desktop notification
//! with an "Open" action; clicking it gives the app a token (the
//! notification server's `ActivationToken` signal), and with it GTK
//! activates the window. Elsewhere the window takes the focus itself, or
//! its taskbar entry flashes.

use tauri::AppHandle;

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::Duration;

    use futures_util::StreamExt;
    use tauri::{AppHandle, Manager};
    use zbus::zvariant::Value;

    #[zbus::proxy(
        interface = "org.freedesktop.Notifications",
        default_service = "org.freedesktop.Notifications",
        default_path = "/org/freedesktop/Notifications"
    )]
    trait Notifications {
        #[allow(clippy::too_many_arguments)]
        fn notify(
            &self,
            app_name: &str,
            replaces_id: u32,
            app_icon: &str,
            summary: &str,
            body: &str,
            actions: &[&str],
            hints: HashMap<&str, Value<'_>>,
            expire_timeout: i32,
        ) -> zbus::Result<u32>;

        fn close_notification(&self, id: u32) -> zbus::Result<()>;

        #[zbus(signal)]
        fn action_invoked(&self, id: u32, action_key: String) -> zbus::Result<()>;

        #[zbus(signal)]
        fn activation_token(&self, id: u32, activation_token: String) -> zbus::Result<()>;

        #[zbus(signal)]
        fn notification_closed(&self, id: u32, reason: u32) -> zbus::Result<()>;
    }

    /// The notification on screen, so a second request replaces it and an
    /// answer closes it.
    static SHOWN: Mutex<Option<u32>> = Mutex::new(None);

    /// A request waits up to two minutes; the notification needn't outlive it.
    const LISTEN_FOR: Duration = Duration::from_secs(150);

    /// Activates the window with the token the click gave, on the main thread.
    fn activate(app: &AppHandle, token: Option<String>) {
        let app = app.clone();
        let _ = app.clone().run_on_main_thread(move || {
            use gtk::prelude::GtkWindowExt;
            let Some(window) = app.get_webview_window("main") else {
                return;
            };
            let _ = window.unminimize();
            let _ = window.show();
            if let Ok(gtk_window) = window.gtk_window() {
                if let Some(token) = &token {
                    // GTK sends it with xdg_activation's activate, which the
                    // compositor honours: the token came from a click
                    gtk_window.set_startup_id(token);
                }
                gtk_window.present();
            }
        });
    }

    pub async fn notify(
        app: AppHandle,
        title: String,
        body: String,
        sound: bool,
    ) -> zbus::Result<()> {
        let connection = zbus::Connection::session().await?;
        let proxy = NotificationsProxy::new(&connection).await?;
        // listen before showing it, so no signal is missed
        let mut actions = proxy.receive_action_invoked().await?;
        let mut tokens = proxy.receive_activation_token().await?;
        let mut closed = proxy.receive_notification_closed().await?;

        let replaces = SHOWN.lock().unwrap().unwrap_or(0);
        let mut hints = HashMap::new();
        hints.insert("desktop-entry", Value::from("mongo-studio"));
        // critical: it stays on screen until answered or dismissed
        hints.insert("urgency", Value::from(2u8));
        if sound {
            // the webview of a minimized window may not play the app's own
            hints.insert("sound-name", Value::from("message-new-instant"));
        } else {
            hints.insert("suppress-sound", Value::from(true));
        }
        let id = proxy
            .notify(
                "Mongo Studio",
                replaces,
                "mongo-studio",
                &title,
                &body,
                &["default", "Open", "open", "Open Mongo Studio"],
                hints,
                0,
            )
            .await?;
        *SHOWN.lock().unwrap() = Some(id);

        let mut token: Option<String> = None;
        let deadline = tokio::time::sleep(LISTEN_FOR);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                Some(signal) = tokens.next() => {
                    if let Ok(args) = signal.args() {
                        if args.id == id {
                            token = Some(args.activation_token);
                        }
                    }
                }
                Some(signal) = actions.next() => {
                    if let Ok(args) = signal.args() {
                        if args.id == id {
                            activate(&app, token.take());
                            break;
                        }
                    }
                }
                Some(signal) = closed.next() => {
                    if signal.args().is_ok_and(|args| args.id == id) {
                        break;
                    }
                }
                _ = &mut deadline => break,
            }
        }
        let mut shown = SHOWN.lock().unwrap();
        if *shown == Some(id) {
            *shown = None;
        }
        Ok(())
    }

    pub async fn dismiss() -> zbus::Result<()> {
        let Some(id) = SHOWN.lock().unwrap().take() else {
            return Ok(());
        };
        let connection = zbus::Connection::session().await?;
        NotificationsProxy::new(&connection)
            .await?
            .close_notification(id)
            .await
    }
}

/// How long the window manager gets to give the window the focus.
const FOCUS_SETTLE: std::time::Duration = std::time::Duration::from_millis(350);

/// Brings the main window forward; true when it ends up focused.
async fn bring_forward(app: &AppHandle) -> bool {
    use tauri::Manager;
    let Some(window) = app.get_webview_window("main") else {
        return false;
    };
    if window.is_focused().unwrap_or(false) {
        return true;
    }
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
    tokio::time::sleep(FOCUS_SETTLE).await;
    window.is_focused().unwrap_or(false)
}

/// Calls the user back to a request that waits: the window comes forward;
/// where it can't (Wayland, by design), a desktop notification with an
/// "Open" action does it on click, with its sound unless `sound` is off.
/// Elsewhere the taskbar entry flashes. Returns at once.
pub fn call_attention(app: &AppHandle, focus: bool, sound: bool, title: &str, body: &str) {
    if !focus {
        return;
    }
    let app = app.clone();
    let (title, body) = (title.to_string(), body.to_string());
    tauri::async_runtime::spawn(async move {
        if bring_forward(&app).await {
            return;
        }
        #[cfg(target_os = "linux")]
        if let Err(e) = linux::notify(app, title, body, sound).await {
            log::warn!("couldn't show the desktop notification: {e}");
        }
        #[cfg(not(target_os = "linux"))]
        {
            use tauri::Manager;
            let _ = (title, body, sound);
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.request_user_attention(Some(tauri::UserAttentionType::Critical));
            }
        }
    });
}

/// Takes the notification `call_attention` showed away, if it's there.
pub fn dismiss_attention() {
    #[cfg(target_os = "linux")]
    tauri::async_runtime::spawn(async {
        if let Err(e) = linux::dismiss().await {
            log::warn!("couldn't close the desktop notification: {e}");
        }
    });
}
