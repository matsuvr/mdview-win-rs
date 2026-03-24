use std::{collections::HashMap, path::PathBuf};

use gpui::{
    AppContext, Bounds, Context, Subscription, WindowBounds, WindowHandle, WindowOptions, px, size,
};

use crate::viewer::MarkdownWindow;

pub const APP_ID: &str = "io.github.mdview";

#[derive(Default)]
pub struct AppRegistry {
    windows_by_path: HashMap<PathBuf, WindowHandle<MarkdownWindow>>,
    subscriptions: Vec<Subscription>,
}

impl AppRegistry {
    pub fn remember_subscription(&mut self, subscription: Subscription) {
        self.subscriptions.push(subscription);
    }

    pub fn open_welcome_window(&mut self, cx: &mut Context<Self>) {
        let options = default_window_options(cx);
        let handle = match cx.open_window(options, |_window, cx| cx.new(MarkdownWindow::welcome)) {
            Ok(handle) => handle,
            Err(error) => {
                eprintln!("failed to open welcome window: {error}");
                return;
            }
        };

        let _ = handle.update(
            cx,
            |view: &mut MarkdownWindow, window, _cx: &mut Context<MarkdownWindow>| {
                view.sync_window(window);
                view.focus_window(window);
            },
        );
        cx.activate(true);
    }

    pub fn open_or_focus_path(&mut self, requested_path: PathBuf, cx: &mut Context<Self>) {
        self.prune_closed_windows(cx);

        let canonical_path = std::fs::canonicalize(&requested_path).ok();

        if let Some(key) = canonical_path.clone() {
            if let Some(handle) = self.windows_by_path.get(&key).copied() {
                match handle.update(
                    cx,
                    |view: &mut MarkdownWindow, window, cx: &mut Context<MarkdownWindow>| {
                        view.reload_from_request(requested_path.clone(), Some(key.clone()), cx);
                        view.sync_window(window);
                        view.focus_window(window);
                    },
                ) {
                    Ok(_) => {
                        cx.activate(true);
                        return;
                    }
                    Err(_) => {
                        self.windows_by_path.remove(&key);
                    }
                }
            }
        }

        let requested_clone = requested_path.clone();
        let canonical_clone = canonical_path.clone();
        let options = default_window_options(cx);
        let handle = match cx.open_window(options, move |_window, cx| {
            cx.new(|cx| {
                MarkdownWindow::from_path(requested_clone.clone(), canonical_clone.clone(), cx)
            })
        }) {
            Ok(handle) => handle,
            Err(error) => {
                eprintln!("failed to open {:?}: {error}", requested_path);
                return;
            }
        };

        let _ = handle.update(
            cx,
            |view: &mut MarkdownWindow, window, _cx: &mut Context<MarkdownWindow>| {
                view.sync_window(window);
                view.focus_window(window);
            },
        );

        if let Some(key) = canonical_path {
            self.windows_by_path.insert(key, handle);
        }

        cx.activate(true);
    }

    pub fn nudge_existing_window(&mut self, cx: &mut Context<Self>) {
        self.prune_closed_windows(cx);
        cx.activate(true);

        if let Some(handle) = self.windows_by_path.values().next().copied() {
            let _ = handle.update(
                cx,
                |view: &mut MarkdownWindow, window, _cx: &mut Context<MarkdownWindow>| {
                    view.focus_window(window);
                    view.sync_window(window);
                },
            );
        }
    }

    fn prune_closed_windows(&mut self, cx: &mut Context<Self>) {
        self.windows_by_path
            .retain(|_, handle| handle.read_with(cx, |_, _| ()).is_ok());
    }
}

fn default_window_options(cx: &mut Context<AppRegistry>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(920.0), px(760.0)),
            cx,
        ))),
        focus: true,
        show: true,
        is_movable: true,
        is_resizable: true,
        is_minimizable: true,
        window_min_size: Some(size(px(420.0), px(300.0))),
        app_id: Some(APP_ID.to_string()),
        ..Default::default()
    }
}
