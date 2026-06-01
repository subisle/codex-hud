mod cc_switch;
mod mcp;

use std::io;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, RecvTimeoutError, Sender},
    Arc, Mutex,
};
use std::thread;
use std::time::Duration;

use crate::hud::{apply_app_server_message, HudSnapshot};
use crate::protocol::{AppServerClient, ClientInfo};

pub struct HudCollectors {
    handles: Vec<thread::JoinHandle<()>>,
}

impl HudCollectors {
    pub fn spawn(
        real_codex: PathBuf,
        socket_path: PathBuf,
        hud_events: Option<Receiver<serde_json::Value>>,
        snapshot: Arc<Mutex<HudSnapshot>>,
        stop: Arc<AtomicBool>,
        hud_dirty: Sender<()>,
    ) -> Self {
        let mut handles = Vec::new();
        let consume_events = hud_events.is_none();

        if let Some(hud_events) = hud_events {
            let snapshot = Arc::clone(&snapshot);
            let stop = Arc::clone(&stop);
            let hud_dirty = hud_dirty.clone();
            handles.push(thread::spawn(move || {
                collect_bridge_events(hud_events, snapshot, stop, hud_dirty);
            }));
        }

        if socket_path.exists() {
            let snapshot = Arc::clone(&snapshot);
            let stop = Arc::clone(&stop);
            let hud_dirty = hud_dirty.clone();
            handles.push(thread::spawn(move || {
                let Ok(runtime) = tokio::runtime::Runtime::new() else {
                    return;
                };

                runtime.block_on(async move {
                    collect_app_server_state(
                        socket_path,
                        snapshot,
                        stop,
                        consume_events,
                        hud_dirty,
                    )
                    .await;
                });
            }));
        }

        handles.push(mcp::spawn(
            real_codex,
            Arc::clone(&snapshot),
            Arc::clone(&stop),
            hud_dirty.clone(),
        ));

        if let Some(db_path) = cc_switch::db_path() {
            handles.push(cc_switch::spawn(db_path, snapshot, stop, hud_dirty));
        }

        Self { handles }
    }

    pub fn join(self) -> io::Result<()> {
        for handle in self.handles {
            handle
                .join()
                .map_err(|_| io::Error::other("HUD collector thread panicked"))?;
        }

        Ok(())
    }
}

fn collect_bridge_events(
    hud_events: Receiver<serde_json::Value>,
    snapshot: Arc<Mutex<HudSnapshot>>,
    stop: Arc<AtomicBool>,
    hud_dirty: Sender<()>,
) {
    while !stop.load(Ordering::Relaxed) {
        match hud_events.recv_timeout(Duration::from_millis(250)) {
            Ok(message) => {
                if let Ok(mut guard) = snapshot.lock() {
                    if apply_app_server_message(&mut guard, &message) {
                        notify_hud_dirty(&hud_dirty);
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

async fn collect_app_server_state(
    socket_path: PathBuf,
    snapshot: Arc<Mutex<HudSnapshot>>,
    stop: Arc<AtomicBool>,
    consume_events: bool,
    hud_dirty: Sender<()>,
) {
    let client_info = ClientInfo {
        name: "codex-hud".to_string(),
        title: Some("Codex HUD".to_string()),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };

    while !stop.load(Ordering::Relaxed) {
        let mut client = match AppServerClient::connect_unix(&socket_path).await {
            Ok(client) => client,
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(250)).await;
                continue;
            }
        };

        let initialized = tokio::time::timeout(
            Duration::from_secs(1),
            client.initialize(client_info.clone()),
        )
        .await;
        if !matches!(initialized, Ok(Ok(_))) {
            tokio::time::sleep(Duration::from_millis(250)).await;
            continue;
        }
        let initialized =
            tokio::time::timeout(Duration::from_millis(500), client.initialized()).await;
        if !matches!(initialized, Ok(Ok(()))) {
            tokio::time::sleep(Duration::from_millis(250)).await;
            continue;
        }

        let rate_limits = tokio::time::timeout(
            Duration::from_millis(500),
            client.account_rate_limits_read(),
        )
        .await;
        if let Ok(Ok(rate_limits)) = rate_limits {
            let mut guard = match snapshot.lock() {
                Ok(guard) => guard,
                Err(_) => return,
            };
            if apply_app_server_message(&mut guard, &serde_json::json!({"result": rate_limits})) {
                notify_hud_dirty(&hud_dirty);
            }
        }

        if consume_events {
            collect_app_server_events(&mut client, &snapshot, &stop, &hud_dirty).await;
        } else {
            poll_known_thread_state(&mut client, &snapshot, &stop, &hud_dirty).await;
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn collect_app_server_events(
    client: &mut AppServerClient,
    snapshot: &Arc<Mutex<HudSnapshot>>,
    stop: &Arc<AtomicBool>,
    hud_dirty: &Sender<()>,
) {
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }

        let message =
            match tokio::time::timeout(Duration::from_millis(250), client.next_message()).await {
                Ok(Ok(message)) => message,
                Ok(Err(_)) => break,
                Err(_) => continue,
            };

        if let Ok(mut guard) = snapshot.lock() {
            if apply_app_server_message(&mut guard, &message) {
                notify_hud_dirty(hud_dirty);
            }
        }
    }
}

async fn poll_known_thread_state(
    client: &mut AppServerClient,
    snapshot: &Arc<Mutex<HudSnapshot>>,
    stop: &Arc<AtomicBool>,
    hud_dirty: &Sender<()>,
) {
    let mut known_thread_id = snapshot
        .lock()
        .ok()
        .and_then(|guard| guard.thread_id.clone());

    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }

        let current_thread_id = snapshot
            .lock()
            .ok()
            .and_then(|guard| guard.thread_id.clone());

        if current_thread_id.is_some() && current_thread_id != known_thread_id {
            if let Some(thread_id) = current_thread_id.clone() {
                let thread = tokio::time::timeout(
                    Duration::from_millis(500),
                    client.thread_read(&thread_id, false),
                )
                .await;
                if let Ok(Ok(thread)) = thread {
                    if let Ok(mut guard) = snapshot.lock() {
                        if apply_app_server_message(
                            &mut guard,
                            &serde_json::json!({"result": thread}),
                        ) {
                            notify_hud_dirty(hud_dirty);
                        }
                    }
                }
                known_thread_id = current_thread_id;
            }
        }

        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

pub(crate) fn notify_hud_dirty(hud_dirty: &Sender<()>) {
    let _ = hud_dirty.send(());
}

pub(crate) fn sleep_until_stop(stop: &AtomicBool, ticks: u16, tick_duration: Duration) -> bool {
    for _ in 0..ticks {
        if stop.load(Ordering::Relaxed) {
            return true;
        }
        thread::sleep(tick_duration);
    }

    false
}
