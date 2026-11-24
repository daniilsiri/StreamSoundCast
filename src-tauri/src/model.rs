use std::{
    collections::HashMap,
    fs, io,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use uuid::Uuid;

pub const CONTROL_PORT: u16 = 45800;
pub const AUDIO_PORT: u16 = 45801;
pub const DISCOVERY_PORT: u16 = 45802;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceMode {
    System,
    Application,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    System,
    Light,
    #[default]
    Dark,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Accent {
    #[default]
    Mint,
    Lime,
    Blue,
    Violet,
    Coral,
    Amber,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    Ru,
    En,
    De,
    Es,
    Fr,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Application {
    pub pid: u32,
    pub name: String,
    pub path: String,
    pub started: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    pub enabled: bool,
    pub volume: u8,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub language: Language,
    pub theme: Theme,
    pub accent: Accent,
    pub device_name: String,
    pub discovery: bool,
    pub reconnect: bool,
    pub source: SourceMode,
    pub application: Option<Application>,
    pub microphone: String,
    pub computer_output: String,
    pub microphone_output: String,
    pub channels: [Channel; 4],
    pub computer_bitrate: u32,
    pub microphone_bitrate: u32,
    pub jitter_ms: u32,
    pub playback_ms: u32,
}

impl Default for Settings {
    fn default() -> Self {
        let on = Channel {
            enabled: true,
            volume: 100,
        };
        Self {
            language: Language::default(),
            theme: Theme::default(),
            accent: Accent::default(),
            device_name: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "StreamSoundCast".into()),
            discovery: true,
            reconnect: true,
            source: SourceMode::System,
            application: None,
            microphone: String::new(),
            computer_output: String::new(),
            microphone_output: String::new(),
            channels: [
                on,
                Channel {
                    enabled: false,
                    ..on
                },
                on,
                on,
            ],
            computer_bitrate: 160,
            microphone_bitrate: 64,
            jitter_ms: 20,
            playback_ms: 20,
        }
    }
}

impl Settings {
    pub fn validate(&mut self) -> anyhow::Result<()> {
        self.device_name = self.device_name.trim().to_owned();
        anyhow::ensure!(
            !self.device_name.is_empty()
                && self.device_name.chars().count() <= 48
                && !self.device_name.chars().any(char::is_control),
            "i18n:invalidName"
        );
        anyhow::ensure!(
            (64..=256).contains(&self.computer_bitrate),
            "i18n:invalidAudioSettings"
        );
        anyhow::ensure!(
            (16..=96).contains(&self.microphone_bitrate),
            "i18n:invalidAudioSettings"
        );
        anyhow::ensure!(
            (20..=80).contains(&self.jitter_ms) && self.jitter_ms % 10 == 0,
            "i18n:invalidAudioSettings"
        );
        anyhow::ensure!(
            (10..=40).contains(&self.playback_ms) && self.playback_ms % 10 == 0,
            "i18n:invalidAudioSettings"
        );
        anyhow::ensure!(
            self.channels.iter().all(|channel| channel.volume <= 100),
            "i18n:invalidAudioSettings"
        );
        Ok(())
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Config {
    id: Uuid,
    settings: Settings,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    pub id: Uuid,
    pub name: String,
    pub ip: Ipv4Addr,
}

#[derive(Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Idle,
    Connecting,
    Pairing,
    Connected,
    Reconnecting,
}

#[derive(Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Metrics {
    pub rtt_ms: Option<f64>,
    pub loss_percent: Option<f64>,
    pub jitter_ms: Option<f64>,
    pub buffer_ms: Option<u32>,
    pub sent: u64,
    pub received: u64,
}

#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub phase: Phase,
    pub peer: Option<Peer>,
    pub code: Option<String>,
    pub incoming: bool,
    pub approved: bool,
    pub message: Option<String>,
    pub metrics: Metrics,
    pub levels: [f32; 4],
    pub channel_messages: [Option<String>; 4],
    pub channel_devices: [Option<ActiveEndpoint>; 4],
}

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveEndpoint {
    pub configured_id: String,
    pub name: String,
}

impl Default for Connection {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            peer: None,
            code: None,
            incoming: false,
            approved: false,
            message: None,
            metrics: Metrics::default(),
            levels: [0.0; 4],
            channel_messages: Default::default(),
            channel_devices: Default::default(),
        }
    }
}

#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub settings: Settings,
    pub connection: Connection,
    pub peers: Vec<Peer>,
    pub addresses: Vec<Ipv4Addr>,
    pub ready: bool,
}

pub struct SessionSlot {
    pub generation: Uuid,
    pub cancel: watch::Sender<bool>,
    pub approval: Option<tokio::sync::oneshot::Sender<bool>>,
}

#[derive(Default)]
pub struct NetworkState {
    pub installing_update: bool,
    pub connection: Connection,
    pub active: Option<SessionSlot>,
    pub ready: bool,
    pub addresses: Vec<Ipv4Addr>,
    pub peers: HashMap<Uuid, (Peer, Instant)>,
}

pub struct AppState {
    pub id: Uuid,
    config_path: PathBuf,
    config_write: Mutex<()>,
    pub(crate) capture_owner: tokio::sync::Mutex<()>,
    pub settings: watch::Sender<Settings>,
    pub network: Mutex<NetworkState>,
}

impl AppState {
    pub fn load(config_path: PathBuf) -> Arc<Self> {
        let mut config = match fs::read_to_string(&config_path) {
            Ok(raw) => toml::from_str(&raw).unwrap_or_else(|error| {
                tracing::warn!(%error, "Invalid configuration");
                Config::default()
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Config::default(),
            Err(error) => {
                tracing::warn!(%error, "Cannot read configuration");
                Config::default()
            }
        };
        if let Err(error) = config.settings.validate() {
            tracing::warn!(%error, "Invalid settings");
            config.settings = Settings::default();
        }
        if config.id.is_nil() {
            config.id = Uuid::new_v4();
        }
        let state = Arc::new(Self {
            id: config.id,
            config_path,
            config_write: Mutex::new(()),
            capture_owner: tokio::sync::Mutex::new(()),
            settings: watch::channel(config.settings.clone()).0,
            network: Mutex::new(NetworkState::default()),
        });
        if let Err(error) = state.set_settings(config.settings) {
            tracing::warn!(%error, "Cannot persist configuration");
        }
        state
    }

    pub fn set_settings(&self, mut settings: Settings) -> anyhow::Result<Settings> {
        settings.validate()?;
        let _write = self.config_write.lock().unwrap();
        let config = Config {
            id: self.id,
            settings: settings.clone(),
        };
        let raw = toml::to_string_pretty(&config)?;
        fs::create_dir_all(self.config_path.parent().unwrap())?;
        let temporary = self.config_path.with_extension("tmp");
        fs::write(&temporary, raw)?;
        replace_file(&temporary, &self.config_path)?;
        self.settings.send_replace(settings.clone());
        Ok(settings)
    }

    pub fn snapshot(&self) -> Snapshot {
        let network = self.network.lock().unwrap();
        let mut peers = if self.settings.borrow().discovery {
            network
                .peers
                .values()
                .filter(|(_, seen)| seen.elapsed() < Duration::from_secs(7))
                .map(|(peer, _)| peer.clone())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        peers.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.ip.cmp(&b.ip))
        });
        Snapshot {
            settings: self.settings.borrow().clone(),
            connection: network.connection.clone(),
            peers,
            addresses: network.addresses.clone(),
            ready: network.ready,
        }
    }

    pub fn update_connection(&self, generation: Uuid, update: impl FnOnce(&mut Connection)) {
        let mut network = self.network.lock().unwrap();
        if network
            .active
            .as_ref()
            .is_some_and(|session| session.generation == generation)
        {
            update(&mut network.connection);
        }
    }
}

impl NetworkState {
    pub(crate) fn update_local_levels(&mut self, levels: [f32; 2]) {
        if self.connection.phase != Phase::Connected {
            self.connection.levels[..2].copy_from_slice(&levels);
        }
    }
}

#[cfg(windows)]
fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let from: Vec<_> = from.as_os_str().encode_wide().chain([0]).collect();
    let to: Vec<_> = to.as_os_str().encode_wide().chain([0]).collect();
    // Both paths remain valid, NUL-terminated UTF-16 arrays for the duration of the call.
    if unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_levels_leave_connected_levels_and_receive_channels_alone() {
        let mut network = NetworkState::default();
        network.update_local_levels([0.4, 0.2]);
        assert_eq!(network.connection.levels, [0.4, 0.2, 0.0, 0.0]);

        network.connection.phase = Phase::Connected;
        network.connection.levels = [0.6, 0.3, 0.5, 0.1];
        network.update_local_levels([0.1, 0.0]);
        assert_eq!(network.connection.levels, [0.6, 0.3, 0.5, 0.1]);

        network.connection = Connection::default();
        network.update_local_levels([0.1, 0.0]);
        assert_eq!(network.connection.levels, [0.1, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn preferences_round_trip_without_losing_existing_routes() -> anyhow::Result<()> {
        let directory = std::env::temp_dir().join(format!("wavecast-{}", Uuid::new_v4()));
        fs::create_dir(&directory)?;
        let path = directory.join("config.toml");
        fs::write(
            &path,
            "[settings]\nmicrophone = 'my-mic'\ncomputerOutput = 'my-speakers'\n",
        )?;
        let state = AppState::load(path.clone());
        let mut settings = state.settings.borrow().clone();
        assert!(settings.theme == Theme::Dark && settings.accent == Accent::Mint);
        assert!(settings.language == Language::Ru);
        settings.theme = Theme::Light;
        settings.accent = Accent::Lime;
        settings.language = Language::Es;
        state.set_settings(settings)?;

        let reopened = AppState::load(path.clone());
        let settings = reopened.settings.borrow();
        assert!(settings.theme == Theme::Light && settings.accent == Accent::Lime);
        assert!(settings.language == Language::Es);
        assert_eq!(settings.microphone, "my-mic");
        assert_eq!(settings.computer_output, "my-speakers");
        fs::remove_file(path)?;
        fs::remove_dir(directory)?;
        Ok(())
    }
}
