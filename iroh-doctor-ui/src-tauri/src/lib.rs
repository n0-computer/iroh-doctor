use anyhow::Result;
use iroh::{discovery::pkarr::PkarrPublisher, key::SecretKey};
use iroh_doctor::{
    doctor,
    protocol::{self, TestConfig},
};
use std::sync::Mutex;
use tauri::{State, Window};
use tokio::task::JoinHandle;

pub struct DoctorApp {
    accept_task: Mutex<Option<JoinHandle<()>>>,
    secret_key: SecretKey,
}

impl Default for DoctorApp {
    fn default() -> Self {
        Self {
            accept_task: Mutex::new(None),
            secret_key: SecretKey::generate(),
        }
    }
}

struct TauriDoctorGui {
    window: Window,
}

impl TauriDoctorGui {
    fn new(window: Window) -> Self {
        Self { window }
    }

    fn emit_test_stats(&self, test_type: &str, bytes: u64, duration: std::time::Duration) {
        let speed = (bytes as f64) / (1024.0 * 1024.0) / duration.as_secs_f64();
        let _ = self
            .window
            .emit("test-stats", (test_type, format!("{:.2} MiB/s", speed)));
    }

    fn emit_connection_accepted(&self) {
        let _ = self.window.emit("connection-accepted", ());
    }

    fn emit_test_complete(&self) {
        let _ = self.window.emit("test-complete", ());
    }
}

impl protocol::GuiExt for TauriDoctorGui {
    type ProgressBar = ProgressBar;

    fn pb(&self) -> &Self::ProgressBar {
        &ProgressBar::new(self.window.clone())
    }

    fn set_send(&self, bytes: u64, duration: std::time::Duration) {
        self.emit_test_stats("send", bytes, duration);
    }

    fn set_recv(&self, bytes: u64, duration: std::time::Duration) {
        self.emit_test_stats("recv", bytes, duration);
    }

    fn set_echo(&self, bytes: u64, duration: std::time::Duration) {
        self.emit_test_stats("echo", bytes, duration);
    }

    fn clear(&self) {}
}

#[derive(Clone)]
pub struct ProgressBar {
    window: Window,
}

impl ProgressBar {
    fn new(window: Window) -> Self {
        Self { window }
    }
}

impl protocol::ProgressBarExt for ProgressBar {
    fn set_message(&self, msg: String) {
        let _ = self.window.emit("progress-update", ("message", msg));
    }

    fn set_position(&self, pos: u64) {
        let _ = self.window.emit("progress-update", ("position", pos));
    }

    fn set_length(&self, len: u64) {
        let _ = self.window.emit("progress-update", ("length", len));
    }
}

#[tauri::command]
async fn start_accepting_connections(
    app: State<'_, DoctorApp>,
    window: Window,
) -> Result<String, String> {
    // Create the endpoint using the stored secret key
    let endpoint = doctor::make_endpoint(
        app.secret_key.clone(),
        Some(iroh::endpoint::default_relay_mode().relay_map()),
        Some(Box::new(PkarrPublisher::n0_dns(app.secret_key.clone()))),
    )
    .await
    .map_err(|e| e.to_string())?;
    let node_id = endpoint.node_id();

    // Format the connection string
    let connection_string = format!("iroh-doctor connect {node_id}");

    // Create TauriDoctorGui with the window
    let gui = TauriDoctorGui::new(window.clone());

    // Spawn the connection acceptance task
    let handle = tokio::spawn(async move {
        if let Err(e) = accept_connections(endpoint, gui).await {
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

async fn accept_connections(endpoint: iroh::Endpoint, gui: TauriDoctorGui) -> Result<()> {
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
        gui.emit_connection_accepted();

        break conn;
    };

    // Run the active side protocol
    protocol::active_side(connection, &config, Some(&gui)).await?;

    // Signal that all tests are complete
    gui.emit_test_complete();

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
