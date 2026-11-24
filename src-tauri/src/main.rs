#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{net::Ipv4Addr, time::Duration};

use tauri::{Emitter, Manager, State};

use streamsoundcast::{
    audio,
    model::{AppState, Settings, Snapshot},
    monitor_local_audio,
    network::Network,
};

mod updates;

#[tauri::command]
fn snapshot(network: State<'_, Network>) -> Snapshot {
    network.state.snapshot()
}

#[tauri::command]
async fn audio_devices(include_applications: Option<bool>) -> Result<audio::Devices, String> {
    tauri::async_runtime::spawn_blocking(move || {
        audio::devices(include_applications.unwrap_or(true))
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())
}

#[tauri::command]
async fn save_settings(
    network: State<'_, Network>,
    settings: Settings,
) -> Result<Settings, String> {
    let state = network.state.clone();
    tauri::async_runtime::spawn_blocking(move || state.set_settings(settings))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn connect(network: State<'_, Network>, ip: String) -> Result<(), String> {
    let ip = ip
        .trim()
        .parse::<Ipv4Addr>()
        .map_err(|_| "i18n:invalidIP".to_owned())?;
    network.connect(ip).map_err(|error| error.to_string())
}

#[tauri::command]
fn disconnect(network: State<'_, Network>) {
    network.stop();
}

#[tauri::command]
fn scan(network: State<'_, Network>) {
    network.scan();
}

#[tauri::command]
fn approve_pairing(
    network: State<'_, Network>,
    code: String,
    approved: bool,
) -> Result<(), String> {
    network
        .approve(&code, approved)
        .map_err(|error| error.to_string())
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "streamsoundcast=info".into()),
        )
        .try_init()
        .ok();
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(updates::Updates::default())
        .setup(|app| {
            let state = AppState::load(app.path().app_config_dir()?.join("config.toml"));
            let network = Network::new(state.clone());
            app.manage(network.clone());
            tauri::async_runtime::spawn(network.serve());
            tauri::async_runtime::spawn(monitor_local_audio(state.clone()));
            let app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut clock = tokio::time::interval(Duration::from_millis(120));
                clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut last = None;
                loop {
                    clock.tick().await;
                    let current = state.snapshot();
                    if last.as_ref() != Some(&current) {
                        let _ = app.emit("wavecast-state", &current);
                        last = Some(current);
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            snapshot,
            audio_devices,
            save_settings,
            connect,
            disconnect,
            approve_pairing,
            scan,
            updates::check_update,
            updates::install_update
        ])
        .run(tauri::generate_context!())
        .expect("Failed to start StreamSoundCast");
}
