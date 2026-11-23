use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed},
    },
    thread,
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use crossbeam_queue::ArrayQueue;
use serde::Serialize;

use crate::model::{ActiveEndpoint, Application, SourceMode};

pub const SAMPLES: usize = 480;
pub type Frame = [f32; SAMPLES * 2];

#[derive(Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Devices {
    pub microphones: Vec<Device>,
    pub outputs: Vec<Device>,
    pub default_microphone: Option<String>,
    pub default_output: Option<String>,
    pub applications: Option<Vec<Application>>,
}

#[derive(Clone, PartialEq, Eq)]
pub enum WorkerKind {
    Computer(SourceMode, Option<Application>),
    Microphone { id: String, echo_reference: String },
    Playback(String, u32),
}

pub struct Worker {
    pub queue: Arc<ArrayQueue<Frame>>,
    pub message: Arc<Mutex<Option<String>>>,
    pub device: Arc<Mutex<Option<ActiveEndpoint>>>,
    pub kind: WorkerKind,
    stop: Arc<AtomicBool>,
}

impl Worker {
    pub fn start(kind: WorkerKind, guard: Arc<AtomicUsize>) -> Result<Self> {
        let queue = Arc::new(ArrayQueue::new(
            if matches!(kind, WorkerKind::Playback(..)) {
                6
            } else {
                4
            },
        ));
        let message = Arc::new(Mutex::new(None));
        let device = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let (queue_thread, message_thread, stop_thread, kind_thread) =
            (queue.clone(), message.clone(), stop.clone(), kind.clone());
        let device_thread = device.clone();
        thread::Builder::new()
            .name("wavecast-wasapi".into())
            .spawn(move || {
                let result = platform::run(
                    kind_thread,
                    &queue_thread,
                    &stop_thread,
                    &message_thread,
                    &device_thread,
                    &guard,
                );
                if let Err(error) = result {
                    tracing::warn!(%error, detail = ?error, "Audio device stopped");
                    *device_thread.lock().unwrap() = None;
                    *message_thread.lock().unwrap() = Some(error.to_string());
                    while queue_thread.pop().is_some() {}
                }
            })?;
        Ok(Self {
            queue,
            message,
            device,
            kind,
            stop,
        })
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
    }
}

pub fn devices(include_applications: bool) -> Result<Devices> {
    platform::devices(include_applications)
}

#[cfg(not(windows))]
mod platform {
    use super::*;

    pub fn devices(_: bool) -> Result<Devices> {
        Err(anyhow!("i18n:desktopOnly"))
    }

    pub fn run(
        _: WorkerKind,
        _: &ArrayQueue<Frame>,
        _: &AtomicBool,
        _: &Mutex<Option<String>>,
        _: &Mutex<Option<ActiveEndpoint>>,
        _: &AtomicUsize,
    ) -> Result<()> {
        Err(anyhow!("i18n:desktopOnly"))
    }
}

#[cfg(windows)]
mod platform {
    use std::{collections::HashSet, ptr};

    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    use wasapi::{
        AudioClient, AudioClientProperties, DeviceEnumerator, Direction, SampleType,
        StreamCategory, StreamMode, WaveFormat,
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, HWND, LPARAM, TRUE, WAIT_TIMEOUT},
        System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
        UI::WindowsAndMessaging::{
            EnumWindows, GetWindowTextLengthW, GetWindowThreadProcessId, IsWindowVisible,
        },
    };

    use super::*;

    struct Com;

    impl Com {
        fn init() -> Result<Self> {
            wasapi::initialize_mta().ok().context("i18n:audioError")?;
            Ok(Self)
        }
    }

    impl Drop for Com {
        fn drop(&mut self) {
            wasapi::deinitialize();
        }
    }

    pub fn devices(include_applications: bool) -> Result<Devices> {
        let _com = Com::init()?;
        let enumerator = DeviceEnumerator::new()?;
        let list = |direction| -> Result<Vec<Device>> {
            let collection = enumerator.get_device_collection(direction)?;
            Ok(collection
                .into_iter()
                .filter_map(|device| {
                    let device = device.ok()?;
                    Some(Device {
                        id: device.get_id().ok()?,
                        name: device.get_friendlyname().ok()?,
                    })
                })
                .collect())
        };
        let default_id = |direction| {
            enumerator
                .get_default_device(direction)
                .and_then(|device| device.get_id())
                .ok()
        };
        Ok(Devices {
            microphones: list(Direction::Capture)?,
            outputs: list(Direction::Render)?,
            default_microphone: default_id(Direction::Capture),
            default_output: default_id(Direction::Render),
            applications: include_applications.then(|| applications(&enumerator)),
        })
    }

    unsafe extern "system" fn collect_window(window: HWND, context: LPARAM) -> i32 {
        // EnumWindows calls synchronously while the collection is exclusively borrowed.
        unsafe {
            if IsWindowVisible(window) != 0 && GetWindowTextLengthW(window) > 0 {
                let mut pid = 0;
                GetWindowThreadProcessId(window, &mut pid);
                if pid != 0 {
                    (*(context as *mut HashSet<u32>)).insert(pid);
                }
            }
        }
        TRUE
    }

    fn applications(enumerator: &DeviceEnumerator) -> Vec<Application> {
        let mut pids = HashSet::<u32>::new();
        // The callback runs on this thread and does not retain the context pointer.
        unsafe { EnumWindows(Some(collect_window), &mut pids as *mut _ as LPARAM) };
        if let Ok(outputs) = enumerator.get_device_collection(Direction::Render) {
            for output in outputs.into_iter().flatten() {
                if let Ok(sessions) = output
                    .get_iaudiosessionmanager()
                    .and_then(|manager| manager.get_audiosessionenumerator())
                {
                    for index in 0..sessions.get_count().unwrap_or(0) {
                        if let Ok(pid) = sessions
                            .get_session(index)
                            .and_then(|session| session.get_process_id())
                        {
                            pids.insert(pid);
                        }
                    }
                }
            }
        }
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet),
        );
        let mut result = Vec::new();
        for pid in pids {
            let Some(mut process) = system.process(Pid::from_u32(pid)) else {
                continue;
            };
            let Some(path) = process.exe().filter(|p| !p.as_os_str().is_empty()) else {
                continue;
            };
            while let Some(parent) = process.parent().and_then(|pid| system.process(pid)) {
                if parent.pid() == process.pid()
                    || parent.start_time() > process.start_time()
                    || parent.exe() != Some(path)
                {
                    break;
                }
                process = parent;
            }
            let pid = process.pid().as_u32();
            if pid == std::process::id() || result.iter().any(|app: &Application| app.pid == pid) {
                continue;
            }
            let name = path
                .file_stem()
                .unwrap_or(process.name())
                .to_string_lossy()
                .into_owned();
            if name.eq_ignore_ascii_case("msedgewebview2") {
                continue;
            }
            result.push(Application {
                pid,
                name,
                path: path.to_string_lossy().into_owned(),
                started: process.start_time(),
            });
        }
        result.sort_by_cached_key(|app| (app.name.to_lowercase(), app.pid));
        result
    }

    fn select_application(
        enumerator: &DeviceEnumerator,
        preferred: &Application,
    ) -> Option<Application> {
        let apps = applications(enumerator);
        if let Some(exact) = apps.iter().find(|app| app == &preferred) {
            return Some(exact.clone());
        }
        let mut matches = apps
            .into_iter()
            .filter(|app| app.path.eq_ignore_ascii_case(&preferred.path));
        let first = matches.next()?;
        matches.next().is_none().then_some(first)
    }

    struct Process(HANDLE);

    impl Process {
        fn open(pid: u32) -> Result<Self> {
            // The returned handle is owned by this guard.
            let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
            anyhow::ensure!(handle != ptr::null_mut(), "i18n:audioError");
            Ok(Self(handle))
        }

        fn running(&self) -> bool {
            unsafe { WaitForSingleObject(self.0, 0) == WAIT_TIMEOUT }
        }
    }

    impl Drop for Process {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    struct LoopbackGuard<'a>(&'a AtomicUsize);

    impl Drop for LoopbackGuard<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Relaxed);
        }
    }

    fn endpoint(
        enumerator: &DeviceEnumerator,
        direction: Direction,
        id: &str,
    ) -> Result<wasapi::Device> {
        let device = if id.is_empty() {
            enumerator.get_default_device(direction)
        } else {
            enumerator.get_device(id)
        }
        .context("i18n:unavailableDevice")?;
        Ok(device)
    }

    fn mode(buffer_ms: u32) -> StreamMode {
        StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: i64::from(buffer_ms) * 10_000,
        }
    }

    fn initialize(
        mut client: AudioClient,
        channels: usize,
        direction: Direction,
        buffer_ms: u32,
    ) -> Result<AudioClient> {
        let format = WaveFormat::new(32, 32, SampleType::Float, 48000, channels, None);
        client
            .initialize_client(&format, direction, mode(buffer_ms), None)
            .context("i18n:audioError")?;
        Ok(client)
    }

    fn microphone_client(
        device: &wasapi::Device,
        echo_reference: Option<String>,
    ) -> Result<AudioClient> {
        let client = device.get_iaudioclient()?;
        let properties = AudioClientProperties::new().set_category(StreamCategory::Communications);
        if let Err(error) = client.set_properties(properties) {
            tracing::debug!(%error, "Communications capture category unavailable");
        }
        let client = match initialize(client, 1, Direction::Capture, 20) {
            Ok(client) => client,
            Err(error) => {
                tracing::debug!(%error, "Retrying microphone with standard capture");
                initialize(device.get_iaudioclient()?, 1, Direction::Capture, 20)?
            }
        };
        if let Ok(control) = client.get_aec_control()
            && let Err(error) = control.set_echo_cancellation_render_endpoint(echo_reference)
        {
            tracing::debug!(%error, "Unable to set acoustic echo cancellation reference");
        }
        Ok(client)
    }

    fn report_endpoint(
        target: &Mutex<Option<ActiveEndpoint>>,
        device: &wasapi::Device,
        id: String,
    ) {
        if let Ok(name) = device.get_friendlyname() {
            *target.lock().unwrap() = Some(ActiveEndpoint {
                configured_id: id,
                name,
            });
        }
    }

    pub fn run(
        kind: WorkerKind,
        queue: &ArrayQueue<Frame>,
        stop: &AtomicBool,
        message: &Mutex<Option<String>>,
        active_device: &Mutex<Option<ActiveEndpoint>>,
        guard: &AtomicUsize,
    ) -> Result<()> {
        let _com = Com::init()?;
        let enumerator = DeviceEnumerator::new()?;
        match kind {
            WorkerKind::Computer(SourceMode::System, _) => {
                let process_capture =
                    AudioClient::new_application_loopback_client(std::process::id(), false)
                        .map_err(anyhow::Error::from)
                        .and_then(|client| initialize(client, 2, Direction::Capture, 0));
                match process_capture {
                    Ok(client) => capture(client, None, 2, queue, stop),
                    Err(error) => {
                        tracing::info!(%error, "Process exclusion unavailable");
                        guard.fetch_add(1, Relaxed);
                        let _guard = LoopbackGuard(guard);
                        *message.lock().unwrap() = Some("i18n:loopbackProtection".into());
                        let client = initialize(
                            endpoint(&enumerator, Direction::Render, "")?.get_iaudioclient()?,
                            2,
                            Direction::Capture,
                            20,
                        )?;
                        capture(client, None, 2, queue, stop)
                    }
                }
            }
            WorkerKind::Computer(SourceMode::Application, Some(preferred)) => {
                while !stop.load(Relaxed) {
                    if let Some(app) = select_application(&enumerator, &preferred) {
                        let process = Process::open(app.pid)?;
                        let client = AudioClient::new_application_loopback_client(app.pid, true)
                            .context("i18n:appCaptureUnsupported")?;
                        let client = initialize(client, 2, Direction::Capture, 0)?;
                        *message.lock().unwrap() = (app.pid != preferred.pid)
                            .then(|| format!("i18n:sourceRestored|{}", app.pid));
                        capture(client, Some(&process), 2, queue, stop)?;
                        while queue.pop().is_some() {}
                    }
                    *message.lock().unwrap() = Some("i18n:sourceWaiting".into());
                    for _ in 0..10 {
                        if stop.load(Relaxed) {
                            break;
                        }
                        thread::sleep(Duration::from_millis(100));
                    }
                }
                Ok(())
            }
            WorkerKind::Computer(SourceMode::Application, None) => Err(anyhow!("i18n:selectApp")),
            WorkerKind::Microphone { id, echo_reference } => {
                let device = endpoint(&enumerator, Direction::Capture, &id)?;
                let reference = endpoint(&enumerator, Direction::Render, &echo_reference)
                    .ok()
                    .and_then(|device| device.get_id().ok());
                let client = microphone_client(&device, reference)?;
                report_endpoint(active_device, &device, id);
                capture(client, None, 1, queue, stop)
            }
            WorkerKind::Playback(id, ms) => {
                let device = endpoint(&enumerator, Direction::Render, &id)?;
                let client = initialize(device.get_iaudioclient()?, 2, Direction::Render, ms)?;
                report_endpoint(active_device, &device, id);
                playback(client, queue, stop, guard)
            }
        }
    }

    fn capture(
        client: AudioClient,
        process: Option<&Process>,
        channels: usize,
        queue: &ArrayQueue<Frame>,
        stop: &AtomicBool,
    ) -> Result<()> {
        let event = client.set_get_eventhandle()?;
        let capture = client.get_audiocaptureclient()?;
        let block = SAMPLES * channels;
        let mut input = vec![0.0_f32; 48_000 * channels];
        let mut frame = [0.0; SAMPLES * 2];
        let mut written = 0;
        client.start_stream().context("i18n:audioError")?;
        let result = (|| -> Result<()> {
            while !stop.load(Relaxed) && process.is_none_or(Process::running) {
                let _ = event.wait_for_event(50);
                while capture.get_next_packet_size()?.unwrap_or(0) > 0 && !stop.load(Relaxed) {
                    let (read, _) =
                        capture.read_from_device(bytemuck::cast_slice_mut(&mut input))?;
                    if read == 0 {
                        break;
                    }
                    let mut samples = &input[..read as usize * channels];
                    while !samples.is_empty() {
                        let count = (block - written).min(samples.len());
                        frame[written..written + count].copy_from_slice(&samples[..count]);
                        written += count;
                        samples = &samples[count..];
                        if written == block {
                            queue.force_push(frame);
                            written = 0;
                        }
                    }
                }
            }
            Ok(())
        })();
        let _ = client.stop_stream();
        result.context("i18n:audioError")
    }

    fn playback(
        client: AudioClient,
        queue: &ArrayQueue<Frame>,
        stop: &AtomicBool,
        guard: &AtomicUsize,
    ) -> Result<()> {
        let event = client.set_get_eventhandle()?;
        let render = client.get_audiorenderclient()?;
        let mut output = vec![0.0_f32; client.get_buffer_size()? as usize * 2];
        let mut frame = [0.0; SAMPLES * 2];
        let mut offset = frame.len();
        render.write_to_device(bytemuck::cast_slice(&output), None)?;
        client.start_stream().context("i18n:audioError")?;
        let result = (|| -> Result<()> {
            while !stop.load(Relaxed) {
                let _ = event.wait_for_event(50);
                let space = client.get_available_space_in_frames()? as usize * 2;
                if space == 0 {
                    continue;
                }
                let mut remaining = &mut output[..space];
                while !remaining.is_empty() {
                    if offset == frame.len() {
                        let Some(next) = queue.pop() else {
                            remaining.fill(0.0);
                            break;
                        };
                        frame = next;
                        offset = 0;
                    }
                    let count = (frame.len() - offset).min(remaining.len());
                    remaining[..count].copy_from_slice(&frame[offset..offset + count]);
                    offset += count;
                    remaining = &mut remaining[count..];
                }
                if guard.load(Relaxed) > 0 {
                    output[..space].fill(0.0);
                }
                render.write_to_device(bytemuck::cast_slice(&output[..space]), None)?;
            }
            Ok(())
        })();
        let _ = client.stop_stream();
        result.context("i18n:audioError")
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::echo::SystemEcho;
        use crate::media::MultimediaTimer;

        #[test]
        #[ignore = "requires a Windows audio endpoint and plays a quiet test tone"]
        fn system_capture_excludes_wavecast_playback() -> Result<()> {
            let _timer = MultimediaTimer::start();
            let _com = Com::init()?;
            let enumerator = DeviceEnumerator::new()?;
            let device = match std::env::var("WAVECAST_WASAPI_TEST_OUTPUT") {
                Ok(name) => enumerator
                    .get_device_collection(Direction::Render)?
                    .get_device_with_name(&name)?,
                Err(_) => endpoint(&enumerator, Direction::Render, "")?,
            };
            let monitor = initialize(device.get_iaudioclient()?, 2, Direction::Capture, 20)?;
            let monitor_event = monitor.set_get_eventhandle()?;
            let monitor_client = monitor.get_audiocaptureclient()?;
            monitor.start_stream()?;

            let guard = Arc::new(AtomicUsize::new(0));
            let system = Worker::start(
                WorkerKind::Computer(SourceMode::System, None),
                guard.clone(),
            )?;
            let playback = Worker::start(WorkerKind::Playback(device.get_id()?, 20), guard)?;
            let steps = [4300.0, 5300.0, 6100.0]
                .map(|frequency| std::f32::consts::TAU * frequency / 48_000.0);
            let amplitude = |samples: &[f32]| {
                // Match all three frequencies to distinguish the test from unrelated system audio.
                steps
                    .iter()
                    .map(|step| {
                        let (sine, cosine) = samples.chunks_exact(2).enumerate().fold(
                            (0.0, 0.0),
                            |(sine, cosine), (i, pair)| {
                                let (sin, cos) = (step * i as f32).sin_cos();
                                (sine + pair[0] * sin, cosine + pair[0] * cos)
                            },
                        );
                        4.0 * sine.hypot(cosine) / samples.len() as f32
                    })
                    .fold(f32::INFINITY, f32::min)
            };
            let mut monitor_buffer = vec![0.0_f32; 48_000 * 2];
            let mut audible = 0.0_f32;
            let mut recaptured = 0.0_f32;
            let mut cleaned = 0.0_f32;
            let mut measured_frames = 0;
            let mut echo = SystemEcho::new();
            let start = std::time::Instant::now();
            for tick in 0..155 {
                let mut frame = [0.0; SAMPLES * 2];
                if tick < 130 {
                    for (sample, pair) in frame.chunks_exact_mut(2).enumerate() {
                        let i = (tick * SAMPLES + sample) as f32;
                        let tone = steps.iter().map(|step| (step * i).sin()).sum::<f32>();
                        pair.fill(0.004 * tone);
                    }
                }
                echo.record(0, &frame);
                playback.queue.force_push(frame);
                let deadline = start + Duration::from_millis((tick as u64 + 1) * 10);
                thread::sleep(deadline.saturating_duration_since(std::time::Instant::now()));
                let _ = monitor_event.wait_for_event(0);
                while monitor_client.get_next_packet_size()?.unwrap_or(0) > 0 {
                    let (size, _) = monitor_client
                        .read_from_device(bytemuck::cast_slice_mut(&mut monitor_buffer))?;
                    if size > 0 {
                        audible = audible.max(amplitude(&monitor_buffer[..size as usize * 2]));
                    }
                }
                while let Some(mut frame) = system.queue.pop() {
                    let raw = amplitude(&frame);
                    echo.cancel(&mut frame);
                    if (30..130).contains(&tick) {
                        recaptured += raw * raw;
                        cleaned += amplitude(&frame).powi(2);
                        measured_frames += 1;
                    }
                }
            }
            monitor.stop_stream()?;
            anyhow::ensure!(measured_frames > 0, "System capture did not deliver frames");
            let recaptured = (recaptured / measured_frames as f32).sqrt();
            let cleaned = (cleaned / measured_frames as f32).sqrt();
            println!(
                "test tone at endpoint: {audible:.5}; in system capture: {recaptured:.5}; after suppression: {cleaned:.5}"
            );
            anyhow::ensure!(
                audible > 0.001,
                "The control loopback did not receive the test tone"
            );
            anyhow::ensure!(
                cleaned < audible * 0.2
                    && (recaptured < audible * 0.1 || cleaned < recaptured * 0.65),
                "WaveCast's playback remained in the outgoing system audio"
            );
            Ok(())
        }
    }
}
