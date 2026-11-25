use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

use anyhow::{Result, ensure};
use opus::{Application as OpusMode, Bitrate, Channels, Decoder, Encoder};
use tokio::{
    net::UdpSocket,
    time::{self, MissedTickBehavior},
};
use uuid::Uuid;

use crate::{
    audio::{Frame, SAMPLES, Worker, WorkerKind},
    echo::SystemEcho,
    jitter::{Jitter, Playout},
    model::{AppState, Phase, Settings, SourceMode},
    protocol::{Keys, MAX_OPUS, MAX_PACKET, open_audio, seal_audio},
};

pub async fn monitor_local_audio(state: Arc<AppState>) {
    let mut tuning = state.settings.subscribe();
    let mut settings = tuning.borrow_and_update().clone();
    let guard = Arc::new(AtomicUsize::new(0));
    let mut capture_owner = None;
    let mut workers: [Option<Worker>; 2] = Default::default();
    let mut levels = [0.0_f32; 2];
    let mut tick = time::interval(Duration::from_millis(20));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tick.tick().await;
        if state.network.lock().unwrap().connection.phase == Phase::Connected {
            // The session can start its capture workers once the preview drops its workers.
            workers = Default::default();
            levels = [0.0; 2];
            capture_owner = None;
            continue;
        }

        if capture_owner.is_none() || tuning.has_changed().unwrap_or(false) {
            if capture_owner.is_none() {
                let Ok(owner) = state.capture_owner.try_lock() else {
                    continue;
                };
                capture_owner = Some(owner);
            }
            settings = tuning.borrow_and_update().clone();
            for (lane, kind) in capture_kinds(&settings).into_iter().enumerate() {
                let channel = settings.channels[lane];
                let enabled = channel.enabled && channel.volume > 0;
                match update_worker(&mut workers[lane], kind, enabled, &guard) {
                    Ok(false) => (),
                    Ok(true) => levels[lane] = 0.0,
                    Err(error) => tracing::warn!(%error, lane, "Cannot start local audio meter"),
                }
            }
        }

        for (lane, level) in levels.iter_mut().enumerate() {
            let Some(worker) = &workers[lane] else {
                *level = 0.0;
                continue;
            };
            let volume = settings.channels[lane].volume;
            let samples = SAMPLES * (2 - lane);
            *level *= 0.81;
            for _ in 0..worker.queue.capacity() {
                let Some(mut frame) = worker.queue.pop() else {
                    break;
                };
                *level = level.max(meter(&mut frame[..samples], volume, 0.0));
            }
        }
        state.network.lock().unwrap().update_local_levels(levels);
    }
}

pub async fn run(
    state: Arc<AppState>,
    generation: Uuid,
    socket: Arc<UdpSocket>,
    peer: SocketAddr,
    keys: &Keys,
) -> Result<()> {
    let _capture_owner = state.capture_owner.lock().await;
    #[cfg(windows)]
    let _timer = MultimediaTimer::start();
    let mut tuning = state.settings.subscribe();
    let mut settings = tuning.borrow_and_update().clone();
    let guard = Arc::new(AtomicUsize::new(0));
    let mut workers: [Option<Worker>; 4] = Default::default();
    update_workers(&settings, &mut workers, &guard)?;
    let mut encoders = [
        Encoder::new(48_000, Channels::Stereo, OpusMode::Audio)?,
        Encoder::new(48_000, Channels::Mono, OpusMode::Voip)?,
    ];
    configure_encoders(&settings, &mut encoders)?;
    let mut decoders = [
        Decoder::new(48_000, Channels::Stereo)?,
        Decoder::new(48_000, Channels::Mono)?,
    ];
    let mut jitter = [Jitter::new(), Jitter::new()];
    let mut echo = SystemEcho::new();
    let mut sequence = [0_u32; 2];
    let mut decoded = [0.0; SAMPLES * 2];
    let mut encoded = [0; MAX_OPUS];
    let mut incoming = vec![0; 65_535];
    let mut outgoing = [0; MAX_PACKET];
    let mut levels = [0.0_f32; 4];
    let mut sent = 0_u64;
    let mut received = 0_u64;
    let mut tick = time::interval(Duration::from_millis(10));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut publish = time::interval(Duration::from_millis(120));
    publish.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            result = socket.recv_from(&mut incoming) => {
                let (len, from) = match result {
                    Ok(packet) => packet,
                    Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => continue,
                    Err(error) => return Err(error.into()),
                };
                if from == peer
                    && let Some((lane, seq, opus)) = open_audio(&keys.receive_media, keys.receive_stream, &mut incoming[..len])
                    && jitter[lane].push(seq, opus, Instant::now())
                {
                    received += 1;
                }
            }
            _ = tick.tick() => {
                for lane in 0..2 {
                    let channel = settings.channels[lane];
                    if let Some(worker) = &workers[lane] {
                        let mut frame = worker.queue.pop().unwrap_or([0.0; SAMPLES * 2]);
                        if lane == 0 && settings.source == SourceMode::System {
                            echo.cancel(&mut frame);
                        }
                        let channels = 2 - lane;
                        levels[lane] = meter(&mut frame[..SAMPLES * channels], channel.volume, levels[lane]);
                        let size = encoders[lane].encode_float(&frame[..SAMPLES * channels], &mut encoded)?;
                        let stream = keys.send_stream + lane as u32;
                        let len = seal_audio(&keys.send_media, stream, sequence[lane], &encoded[..size], &mut outgoing);
                        sequence[lane] = sequence[lane].checked_add(1).ok_or_else(|| anyhow::anyhow!("i18n:reconnectRequired"))?;
                        match socket.send_to(&outgoing[..len], peer).await {
                            Ok(_) => sent += 1,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                            Err(error) => return Err(error.into()),
                        }
                    } else {
                        levels[lane] = 0.0;
                    }

                    let payload = match jitter[lane].tick(settings.jitter_ms) {
                        Playout::Packet(data) => Some(data),
                        Playout::Missing => Some(Vec::new()),
                        Playout::Waiting => None,
                    };
                    let channel = settings.channels[lane + 2];
                    let mut frame: Frame = [0.0; SAMPLES * 2];
                    let channels = 2 - lane;
                    if let Some(payload) = payload {
                        let count = match decoders[lane].decode_float(&payload, &mut decoded[..SAMPLES * channels], false) {
                            Ok(count) => count,
                            Err(_) => decoders[lane].decode_float(&[], &mut decoded[..SAMPLES * channels], false).unwrap_or(0),
                        };
                        if lane == 0 {
                            frame[..count * 2].copy_from_slice(&decoded[..count * 2]);
                        } else {
                            for (sample, pair) in decoded[..count].iter().zip(frame.chunks_exact_mut(2)) {
                                pair.fill(*sample);
                            }
                        }
                    }
                    if let Some(worker) = &workers[lane + 2]
                        && !worker.playback_blocked.load(Relaxed)
                    {
                        let volume = if guard.load(Relaxed) == 0 { channel.volume } else { 0 };
                        levels[lane + 2] = meter(&mut frame, volume, levels[lane + 2]);
                        worker.queue.force_push(frame);
                    } else {
                        levels[lane + 2] = 0.0;
                        frame.fill(0.0);
                    }
                    echo.record(lane, &frame);
                }
            }
            result = tuning.changed() => {
                result?;
                settings = tuning.borrow_and_update().clone();
                update_workers(&settings, &mut workers, &guard)?;
                configure_encoders(&settings, &mut encoders)?;
            }
            _ = publish.tick() => {
                let lost = jitter.iter().map(|b| b.lost).sum::<u64>();
                let total = lost + jitter.iter().map(|b| b.played).sum::<u64>();
                let mut messages = std::array::from_fn(|lane| workers[lane].as_ref()
                    .and_then(|worker| worker.message.lock().unwrap().clone()));
                let devices = std::array::from_fn(|lane| workers[lane].as_ref()
                    .and_then(|worker| worker.device.lock().unwrap().clone()));
                if guard.load(Relaxed) > 0 {
                    for message in &mut messages[2..] {
                        message.get_or_insert_with(|| "i18n:loopbackProtection".into());
                    }
                }
                state.update_connection(generation, |connection| {
                    connection.levels = levels;
                    connection.channel_messages = messages;
                    connection.channel_devices = devices;
                    connection.metrics.loss_percent = (total > 0).then(|| 100.0 * lost as f64 / total as f64);
                    connection.metrics.jitter_ms = (received > 0).then(|| jitter.iter().map(|b| b.arrival_jitter_ms).fold(0.0, f64::max));
                    connection.metrics.buffer_ms = (received > 0).then(|| jitter.iter().map(|b| b.target_frames(settings.jitter_ms)).max().unwrap() * 10);
                    connection.metrics.sent = sent;
                    connection.metrics.received = received;
                });
            }
        }
    }
}

#[cfg(windows)]
pub(crate) struct MultimediaTimer(bool);

#[cfg(windows)]
impl MultimediaTimer {
    pub(crate) fn start() -> Self {
        // The multimedia timer period is requested for this process while audio is active.
        Self(unsafe { windows_sys::Win32::Media::timeBeginPeriod(1) == 0 })
    }
}

#[cfg(windows)]
impl Drop for MultimediaTimer {
    fn drop(&mut self) {
        if self.0 {
            unsafe { windows_sys::Win32::Media::timeEndPeriod(1) };
        }
    }
}

fn meter(samples: &mut [f32], volume: u8, previous: f32) -> f32 {
    let gain = f32::from(volume) / 100.0;
    let mut peak = 0.0_f32;
    for sample in samples {
        *sample = if sample.is_finite() {
            (*sample * gain).clamp(-1.0, 1.0)
        } else {
            0.0
        };
        peak = peak.max(sample.abs());
    }
    peak.max(previous * 0.90)
}

fn configure_encoders(settings: &Settings, encoders: &mut [Encoder; 2]) -> Result<()> {
    for (encoder, bitrate) in encoders
        .iter_mut()
        .zip([settings.computer_bitrate, settings.microphone_bitrate])
    {
        ensure!(bitrate <= 256);
        encoder.set_bitrate(Bitrate::Bits((bitrate * 1_000) as i32))?;
    }
    Ok(())
}

fn capture_kinds(settings: &Settings) -> [WorkerKind; 2] {
    let computer = match settings.source {
        SourceMode::System => settings
            .system_output_id()
            .map_or(WorkerKind::Computer(SourceMode::System, None), |id| {
                WorkerKind::OutputCapture(id.to_owned())
            }),
        SourceMode::Application => {
            WorkerKind::Computer(SourceMode::Application, settings.application.clone())
        }
    };
    [
        computer,
        WorkerKind::Microphone {
            id: settings.microphone.clone(),
            echo_reference: if settings.channels[2].enabled {
                settings.computer_output.clone()
            } else {
                settings.microphone_output.clone()
            },
        },
    ]
}

fn update_worker(
    worker: &mut Option<Worker>,
    kind: WorkerKind,
    enabled: bool,
    guard: &Arc<AtomicUsize>,
) -> Result<bool> {
    if !enabled {
        return Ok(worker.take().is_some());
    }
    if worker.as_ref().is_some_and(|current| current.kind == kind) {
        return Ok(false);
    }
    *worker = None;
    *worker = Some(Worker::start(kind, guard.clone())?);
    Ok(true)
}

fn update_workers(
    settings: &Settings,
    workers: &mut [Option<Worker>; 4],
    guard: &Arc<AtomicUsize>,
) -> Result<()> {
    let [computer, microphone] = capture_kinds(settings);
    let captured_output = settings.system_output_id().filter(|_| settings.channels[0].enabled);
    let playback = |id: &str| WorkerKind::Playback {
        id: id.to_owned(),
        ms: settings.playback_ms,
        captured_output: captured_output.map(str::to_owned),
    };
    let kinds = [
        computer,
        microphone,
        playback(&settings.computer_output),
        playback(&settings.microphone_output),
    ];
    for (lane, (worker, kind)) in workers.iter_mut().zip(kinds).enumerate() {
        update_worker(worker, kind, settings.channels[lane].enabled, guard)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_system_audio_can_capture_an_output_device() {
        let mut settings = Settings::default();
        assert!(matches!(capture_kinds(&settings)[0], WorkerKind::Computer(SourceMode::System, None)));
        settings.system_output = Some("virtual-output".into());
        assert!(matches!(&capture_kinds(&settings)[0], WorkerKind::OutputCapture(id) if id == "virtual-output"));
        settings.source = SourceMode::Application;
        assert!(matches!(capture_kinds(&settings)[0], WorkerKind::Computer(SourceMode::Application, None)));
    }

    #[test]
    fn opus_round_trips_stereo_and_mono_with_ten_millisecond_frames() {
        for (channels, mode, stride) in [
            (Channels::Stereo, OpusMode::Audio, 2),
            (Channels::Mono, OpusMode::Voip, 1),
        ] {
            let mut encoder = Encoder::new(48_000, channels, mode).unwrap();
            let mut decoder = Decoder::new(48_000, channels).unwrap();
            let mut output = [0.0_f32; SAMPLES * 2];
            let mut packet = [0; MAX_OPUS];
            let input = (0..SAMPLES)
                .flat_map(|index| {
                    let sample = (index as f32 * std::f32::consts::TAU / 48.0).sin() * 0.25;
                    std::iter::repeat_n(sample, stride)
                })
                .collect::<Vec<_>>();
            for _ in 0..4 {
                let size = encoder.encode_float(&input, &mut packet).unwrap();
                assert_eq!(
                    decoder
                        .decode_float(&packet[..size], &mut output[..SAMPLES * stride], false)
                        .unwrap(),
                    SAMPLES
                );
            }
            assert!(output.iter().any(|sample| sample.abs() > 0.05));
            assert_eq!(
                decoder
                    .decode_float(&[], &mut output[..SAMPLES * stride], false)
                    .unwrap(),
                SAMPLES
            );
        }
    }
}
