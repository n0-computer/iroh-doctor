use anyhow::{Context, Result};
use iroh::{
    discovery::{dns::DnsDiscovery, pkarr::PkarrPublisher},
    SecretKey,
};
use iroh_doctor::{
    doctor,
    protocol::{self, TestConfig},
};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use tauri::{AppHandle, Emitter, Manager, State, Window};
use tokio::task::JoinHandle;

pub struct DoctorApp {
    accept_task: Mutex<Option<JoinHandle<()>>>,
    secret_key: SecretKey,
    gui: Mutex<Option<TauriDoctorGui>>,
}

impl DoctorApp {
    pub async fn new(app: &AppHandle) -> Result<Self> {
        let dir = app.path().app_data_dir()?.join("iroh-doctor-ui");
        let secret_key = iroh_node_util::fs::load_secret_key(dir.join("secret.key")).await?;

        Ok(Self {
            accept_task: Mutex::new(None),
            secret_key,
            gui: Mutex::new(None),
        })
    }

    fn set_gui(&self, gui: TauriDoctorGui) {
        if let Ok(mut state) = self.gui.lock() {
            *state = Some(gui);
        }
    }
}

#[derive(Clone)]
struct TauriDoctorGui {
    window: Window,
    progress_pos: Arc<AtomicU64>,
    progress_len: Arc<AtomicU64>,
    progress_message: Arc<Mutex<String>>,
}

impl TauriDoctorGui {
    fn new(window: Window) -> Self {
        Self {
            window,
            progress_pos: Arc::new(AtomicU64::new(0)),
            progress_len: Arc::new(AtomicU64::new(0)),
            progress_message: Arc::new(Mutex::new(String::new())),
        }
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
    type ProgressBar = Self;

    fn pb(&self) -> &Self::ProgressBar {
        self
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

impl protocol::ProgressBarExt for TauriDoctorGui {
    fn set_message(&self, msg: String) {
        if let Ok(mut message) = self.progress_message.lock() {
            *message = msg;
        }
    }

    fn set_position(&self, pos: u64) {
        self.progress_pos.store(pos, Ordering::Relaxed);
    }

    fn set_length(&self, len: u64) {
        self.progress_len.store(len, Ordering::Relaxed);
    }
}

#[derive(serde::Serialize)]
struct ProgressState {
    position: u64,
    length: u64,
    message: String,
}

#[tauri::command]
fn get_progress_state(app: State<'_, DoctorApp>) -> ProgressState {
    app.gui
        .lock()
        .unwrap()
        .as_ref()
        .map(|gui| ProgressState {
            position: gui.progress_pos.load(Ordering::Relaxed),
            length: gui.progress_len.load(Ordering::Relaxed),
            message: gui.progress_message.lock().unwrap().clone(),
        })
        .unwrap_or_else(|| ProgressState {
            position: 0,
            length: 0,
            message: "Pending".to_string(),
        })
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

    tracing::info!(node_id = node_id.to_string(), "Started endpoint");

    // Create TauriDoctorGui and store it in DoctorApp
    let gui = TauriDoctorGui::new(window.clone());
    app.set_gui(gui.clone());

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
    Ok(node_id.to_string())
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

#[tauri::command]
async fn connect_to_node(
    app: State<'_, DoctorApp>,
    window: Window,
    node_id: String,
) -> Result<(), String> {
    // Parse the node ID
    let node_id = node_id.parse::<iroh::NodeId>().map_err(|e| e.to_string())?;

    // Generate a new random secret key
    let secret_key = iroh::SecretKey::generate(rand::rngs::OsRng);

    // Create the endpoint using the generated secret key
    let endpoint = doctor::make_endpoint(
        secret_key,
        Some(iroh::endpoint::default_relay_mode().relay_map()),
        Some(Box::new(DnsDiscovery::n0_dns())),
    )
    .await
    .map_err(|e| e.to_string())?;

    // Create TauriDoctorGui and store it in DoctorApp
    let gui = TauriDoctorGui::new(window.clone());
    app.set_gui(gui.clone());

    // Connect to the remote node
    let connection = endpoint
        .connect(node_id, &iroh_doctor::doctor::DR_RELAY_ALPN)
        .await
        .map_err(|e| e.to_string())?;

    // Run the passive side protocol
    if let Err(e) = protocol::passive_side(gui, connection).await {
        return Err(format!("Error in passive side: {}", e));
    }

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    println!("hello from iroh-doctor!");

    init_logging();

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();

    #[cfg(mobile)]
    {
        tracing::info!("Installing barcode scanner plugin");
        builder = builder.plugin(tauri_plugin_barcode_scanner::init())
    }

    builder
        .setup(|app| {
            let handle = app.handle();
            // Initialize DoctorApp in an async context
            tauri::async_runtime::block_on(async {
                let doctor_app = DoctorApp::new(&handle)
                    .await
                    .context("Failed to initialize DoctorApp")?;
                handle.manage(doctor_app);
                Ok(())
            })
        })
        .invoke_handler(tauri::generate_handler![
            start_accepting_connections,
            connect_to_node,
            get_progress_state,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn init_logging() {
    use tracing_subscriber::{util::SubscriberInitExt, EnvFilter};

    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("vault_lib=debug".parse().unwrap())
                .add_directive("info".parse().unwrap()),
        )
        .finish();

    subscriber.init();

    tracing::info!("Initialized logger");
}
