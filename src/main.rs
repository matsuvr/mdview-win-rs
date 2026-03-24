#![forbid(unsafe_code)]
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("mdview targets Windows x64 only.");

mod assets;
mod ipc;
mod markdown;
mod mermaid;
mod registry;
mod theme;
mod viewer;

use std::path::PathBuf;
use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

use gpui::{AppContext, Application, AsyncApp, Context, Entity};

use crate::{
    assets::AppAssets,
    ipc::{IpcMode, forward_to_primary, spawn_listener_thread, try_establish_endpoint},
    registry::AppRegistry,
};

fn main() {
    let launch_paths: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();

    let primary_endpoint = match try_establish_endpoint() {
        Ok(IpcMode::Secondary(mut stream)) => {
            if let Err(error) = forward_to_primary(&mut stream, &launch_paths) {
                eprintln!("failed to contact existing instance: {error}");
            }
            return;
        }
        Ok(IpcMode::Primary(primary)) => Some(primary),
        Err(error) => {
            eprintln!("failed to initialize single-instance endpoint: {error}");
            None
        }
    };

    let app_assets = AppAssets::default();

    Application::new()
        .with_assets(app_assets.clone())
        .run(move |cx| {
            cx.set_global(app_assets.clone());

            let registry: Entity<AppRegistry> = cx.new(|_| AppRegistry::default());
            let mut primary_listener = None;
            let mut primary_instance_lock = None;

            if let Some(primary) = primary_endpoint {
                let (listener, instance_lock) = primary.into_parts();
                primary_listener = Some(listener);
                primary_instance_lock = Some(instance_lock);
            }

            let quit_subscription = cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            });
            let app_quit_subscription = cx.on_app_quit({
                let mut primary_instance_lock = primary_instance_lock;
                move |_cx| {
                    drop(primary_instance_lock.take());
                    async {}
                }
            });

            let _ = registry.update(
                cx,
                move |registry: &mut AppRegistry, _cx: &mut Context<AppRegistry>| {
                    registry.remember_subscription(quit_subscription);
                    registry.remember_subscription(app_quit_subscription);
                },
            );

            if let Some(listener) = primary_listener {
                let (ipc_tx, ipc_rx) = mpsc::channel::<Vec<PathBuf>>();
                let _listener_thread = spawn_listener_thread(listener, ipc_tx);
                let registry = registry.clone();

                cx.spawn(move |app: &mut AsyncApp| {
                    let app = app.clone();

                    async move {
                        loop {
                            loop {
                                match ipc_rx.try_recv() {
                                    Ok(paths) => {
                                        let update_result = app.update({
                                            let registry = registry.clone();
                                            move |app_cx| {
                                                if paths.is_empty() {
                                                    let _ = registry.update(
                                                        app_cx,
                                                        |registry: &mut AppRegistry,
                                                         cx: &mut Context<AppRegistry>| {
                                                            registry.nudge_existing_window(cx);
                                                        },
                                                    );
                                                } else {
                                                    for path in paths {
                                                        let _ = registry.update(
                                                            app_cx,
                                                            move |registry: &mut AppRegistry,
                                                                  cx: &mut Context<AppRegistry>| {
                                                                registry.open_or_focus_path(path, cx);
                                                            },
                                                        );
                                                    }
                                                }
                                            }
                                        });

                                        if let Err(error) = update_result {
                                            eprintln!("IPC UI dispatch failed: {error}");
                                            return;
                                        }
                                    }
                                    Err(TryRecvError::Empty) => break,
                                    Err(TryRecvError::Disconnected) => return,
                                }
                            }

                            app.background_executor()
                                .timer(Duration::from_millis(25))
                                .await;
                        }
                    }
                })
                .detach();
            }

            if launch_paths.is_empty() {
                let _ = registry.update(cx, |registry: &mut AppRegistry, cx: &mut Context<AppRegistry>| {
                    registry.open_welcome_window(cx)
                });
            } else {
                for path in &launch_paths {
                    let path = path.clone();
                    let _ = registry.update(
                        cx,
                        move |registry: &mut AppRegistry, cx: &mut Context<AppRegistry>| {
                            registry.open_or_focus_path(path, cx)
                        },
                    );
                }
            }

            cx.activate(true);
        });
}
