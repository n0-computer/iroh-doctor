use anyhow::Result;
use iroh::{discovery::pkarr::PkarrPublisher, key::SecretKey, RelayMap};
use iroh_doctor::{
    doctor,
    protocol::{self, TestConfig},
};
use std::sync::Mutex;
use tauri::State;
use tokio::task::JoinHandle;

#[derive(Default)]
pub struct DoctorApp {
    accept_task: Mutex<Option<JoinHandle<()>>>,
}

impl protocol::GuiExt for DoctorApp {
    type ProgressBar = ProgressBar;

    fn pb(&self) -> &Self::ProgressBar {
        todo!()
    }

    fn set_send(&self, bytes: u64, duration: std::time::Duration) {
        todo!()
    }

    fn set_recv(&self, bytes: u64, duration: std::time::Duration) {
        todo!()
    }

    fn set_echo(&self, bytes: u64, duration: std::time::Duration) {
        todo!()
    }

    fn clear(&self) {
        todo!()
    }
}

#[derive(Clone)]
pub struct ProgressBar;

impl protocol::ProgressBarExt for ProgressBar {
    fn set_message(&self, msg: String) {
        todo!()
    }

    fn set_position(&self, pos: u64) {
        todo!()
    }

    fn set_length(&self, len: u64) {
        todo!()
    }
}

#[tauri::command]
async fn start_accepting_connections(app: State<'_, DoctorApp>) -> Result<String, String> {
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

        let Ok(conn) = incoming.await else {
            continue;
        };

        break conn;
    };

    // Run the active side protocol
    protocol::active_side::<DoctorApp>(connection, &config, None).await?;

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
