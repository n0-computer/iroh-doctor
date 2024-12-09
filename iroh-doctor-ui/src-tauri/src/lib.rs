use std::sync::Mutex;
use tauri::State;
use tokio::task::JoinHandle;

#[derive(Default)]
pub struct DoctorApp {
    accept_task: Mutex<Option<JoinHandle<()>>>,
}

#[tauri::command]
async fn start_accepting_connections(app: State<'_, DoctorApp>) -> Result<String, String> {
    let mut accept_task = app.accept_task.lock().map_err(|e| e.to_string())?;

    // Spawn a new task that just exits immediately for now
    let handle = tokio::spawn(async move {
        println!("Started accepting connections...");
        // TODO: Implement actual connection acceptance
    });

    // Even if there's already a task running, replace it.
    *accept_task = Some(handle);

    // Return a mock connection string
    Ok("iroh-doctor connect uvpsmezolzb55a2nknbtf5tkedkibq7fbij2gevaogw6uzljtapa".into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(DoctorApp::default()) // Register our app state
        .invoke_handler(tauri::generate_handler![start_accepting_connections,])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
