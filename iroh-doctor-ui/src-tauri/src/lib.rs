use anyhow::Result;
use iroh::{discovery::pkarr::PkarrPublisher, key::SecretKey};
use iroh_doctor::{
    doctor,
    protocol::{self, TestConfig},
};
use std::sync::Mutex;
use tauri::{Emitter, State};
use tokio::task::JoinHandle;

#[derive(Default)]
pub struct DoctorApp {
    accept_task: Mutex<Option<JoinHandle<()>>>,
}

struct TauriDoctorGui;

impl protocol::GuiExt for TauriDoctorGui {
    type ProgressBar = ProgressBar;

    fn pb(&self) -> &Self::ProgressBar {
        &ProgressBar
    }

    fn set_send(&self, bytes: u64, duration: std::time::Duration) {
        let speed = (bytes as f64) / (1024.0 * 1024.0) / duration.as_secs_f64();
        if let Some(app) = CURRENT_WINDOW.get() {
            let _ = app.emit("test-stats", ("send", format!("{:.2} MiB/s", speed)));
        }
    }

    fn set_recv(&self, bytes: u64, duration: std::time::Duration) {
        let speed = (bytes as f64) / (1024.0 * 1024.0) / duration.as_secs_f64();
        if let Some(app) = CURRENT_WINDOW.get() {
            let _ = app.emit("test-stats", ("recv", format!("{:.2} MiB/s", speed)));
        }
    }

    fn set_echo(&self, bytes: u64, duration: std::time::Duration) {
        let speed = (bytes as f64) / (1024.0 * 1024.0) / duration.as_secs_f64();
        if let Some(app) = CURRENT_WINDOW.get() {
            let _ = app.emit("test-stats", ("echo", format!("{:.2} MiB/s", speed)));
        }
    }

    fn clear(&self) {}
}

use std::sync::OnceLock;
static CURRENT_WINDOW: OnceLock<tauri::Window> = OnceLock::new();

#[derive(Clone)]
pub struct ProgressBar;

impl protocol::ProgressBarExt for ProgressBar {
    fn set_message(&self, msg: String) {
        if let Some(app) = CURRENT_WINDOW.get() {
            let _ = app.emit("progress-update", ("message", msg));
        }
    }

    fn set_position(&self, pos: u64) {
        if let Some(app) = CURRENT_WINDOW.get() {
            let _ = app.emit("progress-update", ("position", pos));
        }
    }

    fn set_length(&self, len: u64) {
        if let Some(app) = CURRENT_WINDOW.get() {
            let _ = app.emit("progress-update", ("length", len));
        }
    }
}

#[tauri::command]
async fn start_accepting_connections(
    app: State<'_, DoctorApp>,
    window: tauri::Window,
) -> Result<String, String> {
    let _ = CURRENT_WINDOW.set(window);

    let secret_key = SecretKey::generate();

    // Create the endpoint first to get the node id for the connection string
    let endpoint = doctor::make_endpoint(
        secret_key.clone(),
        Some(iroh::endpoint::default_relay_mode().relay_map()),
        Some(Box::new(PkarrPublisher::n0_dns(secret_key))),
    )
    .await
    .map_err(|e| e.to_string())?;
    let node_id = endpoint.node_id();

    // Format the connection string
    let connection_string = format!("iroh-doctor connect {node_id}");

    // Spawn the connection acceptance task
    let handle = tokio::spawn(async move {
        if let Err(e) = accept_connections(endpoint).await {
            eprintln!("Error accepting connections: {}", e);
        }
    });

    let mut accept_task = app.accept_task.lock().map_err(|e| e.to_string())?;
    if let Some(task) = accept_task.take() {
        task.abort();
    }
    *accept_task = Some(handle);
    Ok(connection_string)
}

async fn accept_connections(endpoint: iroh::Endpoint) -> Result<()> {
    // Set up the test configuration
    let config = TestConfig {
        size: 16 * 1024 * 1024, // 16MB
        iterations: Some(1),
    };

    // Accept a single connection
    let connection = loop {
        let Some(incoming) = endpoint.accept().await else {
            return Err(anyhow::anyhow!("Endpoint closed"));
        };

        let Ok(mut connecting) = incoming.accept() else {
            continue;
        };

        let Ok(alpn) = connecting.alpn().await else {
            continue;
        };

        if alpn != iroh_doctor::doctor::DR_RELAY_ALPN {
            continue;
        };

        let Ok(conn) = connecting.await else {
            continue;
        };

        // Notify the UI that a connection was accepted
        if let Some(window) = CURRENT_WINDOW.get() {
            let _ = window.emit("connection-accepted", ());
        }

        break conn;
    };

    // Run the active side protocol
    protocol::active_side(connection, &config, Some(&TauriDoctorGui)).await?;

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(DoctorApp::default())
        .invoke_handler(tauri::generate_handler![start_accepting_connections])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
