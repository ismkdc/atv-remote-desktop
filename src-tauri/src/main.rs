#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod network_discovery;
mod remote;
use tauri::Manager;
#[tauri::command]
async fn bridge(
    app: tauri::AppHandle,
    state: tauri::State<'_, remote::State>,
    request: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let path = app.path().app_data_dir().map_err(|e| e.to_string())?;
    remote::dispatch(&state, path, request).await
}
fn main() {
    if std::env::args().any(|arg| arg == "--health") {
        println!("{{\"ok\":true,\"backend\":\"rust\"}}");
        return;
    }
    tauri::Builder::default()
        .manage(remote::State::default())
        .invoke_handler(tauri::generate_handler![bridge])
        .run(tauri::generate_context!())
        .expect("Could not start the application");
}
