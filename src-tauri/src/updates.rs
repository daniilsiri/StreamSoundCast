use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::Mutex;
use streamsoundcast::network::Network;

#[derive(Default)]
pub struct Updates(Mutex<Option<Update>>);

#[derive(Serialize)]
pub struct UpdateInfo {
    current: String,
    available: Option<String>,
}

#[tauri::command]
pub async fn check_update(
    app: AppHandle,
    updates: State<'_, Updates>,
) -> Result<UpdateInfo, String> {
    let mut pending = updates.0.lock().await;
    *pending = None;
    let updater = app
        .updater_builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| error.to_string())?;
    let update = updater.check().await.map_err(|error| error.to_string())?;
    let available = update.as_ref().map(|release| release.version.clone());
    *pending = update;
    Ok(UpdateInfo {
        current: app.package_info().version.to_string(),
        available,
    })
}

#[tauri::command]
pub async fn install_update(
    app: AppHandle,
    updates: State<'_, Updates>,
    network: State<'_, Network>,
    version: String,
) -> Result<(), String> {
    let pending = updates.0.lock().await;
    let update = pending
        .as_ref()
        .filter(|release| release.version == version)
        .ok_or_else(|| "i18n:installError".to_owned())?;
    if network.state.network.lock().unwrap().active.is_some() {
        return Err("i18n:updateBusy".into());
    }
    let bytes = update
        .download(|_, _| {}, || {})
        .await
        .map_err(install_error)?;
    {
        let mut state = network.state.network.lock().unwrap();
        if state.active.is_some() {
            return Err("i18n:updateBusy".into());
        }
        state.installing_update = true;
    }
    if let Err(error) = update.install(bytes).map_err(install_error) {
        network.state.network.lock().unwrap().installing_update = false;
        return Err(error);
    }
    app.restart();
}

fn install_error(error: tauri_plugin_updater::Error) -> String {
    tracing::warn!(%error, "Update installation failed");
    "i18n:installError".into()
}
