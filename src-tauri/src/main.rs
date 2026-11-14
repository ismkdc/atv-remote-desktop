#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use std::process::Stdio;
use tauri::Manager;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
};
struct Bridge {
    _child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}
#[derive(Default)]
struct State(Mutex<Option<Bridge>>);
#[tauri::command]
async fn bridge(
    app: tauri::AppHandle,
    state: tauri::State<'_, State>,
    request: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let mut guard = state.0.lock().await;
    if guard.is_none() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut command = if cfg!(debug_assertions) {
            let python = std::env::var("ATV_PYTHON")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| {
                    root.join(if cfg!(windows) {
                        ".venv/Scripts/python.exe"
                    } else {
                        ".venv/bin/python3"
                    })
                });
            let mut command = Command::new(python);
            command.arg("-u").arg(root.join("Support/bridge.py"));
            command
        } else {
            let executable = std::env::current_exe().map_err(|e| e.to_string())?;
            Command::new(
                executable
                    .parent()
                    .ok_or("Application path could not be determined")?
                    .join(if cfg!(windows) {
                        "atv-bridge.exe"
                    } else {
                        "atv-bridge"
                    }),
            )
        };
        let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
        let mut child = command
            .arg(data)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| e.to_string())?;
        let input = child.stdin.take().ok_or("Could not open helper input")?;
        let output = BufReader::new(child.stdout.take().ok_or("Could not open helper output")?);
        *guard = Some(Bridge {
            _child: child,
            input,
            output,
        });
    }
    let worker = guard.as_mut().unwrap();
    let result = async {
        worker
            .input
            .write_all(format!("{}\n", request).as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        worker.input.flush().await.map_err(|e| e.to_string())?;
        let mut line = String::new();
        worker
            .output
            .read_line(&mut line)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::from_str::<serde_json::Value>(&line).map_err(|_| {
            "The Apple TV helper stopped unexpectedly. Check the helper installation.".to_string()
        })
    }
    .await;
    match result {
        Ok(value) => {
            if let Some(error) = value.get("error") {
                Err(error.as_str().unwrap_or("Connection error").into())
            } else {
                Ok(value)
            }
        }
        Err(e) => {
            *guard = None;
            Err(e)
        }
    }
}
fn main() {
    tauri::Builder::default()
        .manage(State::default())
        .invoke_handler(tauri::generate_handler![bridge])
        .run(tauri::generate_context!())
        .expect("Could not start the application");
}
