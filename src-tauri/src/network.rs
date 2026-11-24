use std::{
    collections::HashMap,
    net::{Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, ensure};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use tokio::{
    io::AsyncRead,
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{Notify, mpsc, oneshot, watch},
    time,
};
use uuid::Uuid;
use x25519_dalek::{PublicKey, StaticSecret};

use crate::{
    media,
    model::{
        AUDIO_PORT, AppState, CONTROL_PORT, Connection, DISCOVERY_PORT, Peer, Phase, SessionSlot,
    },
    protocol::{
        CAPABILITIES, Control, ControlCipher, Hello, Keys, VERSION, commitment, read_record,
        valid_name, write_record,
    },
};

#[derive(Serialize, Deserialize)]
struct Announce {
    protocol: String,
    version: u8,
    id: Uuid,
    name: String,
}

#[derive(Clone)]
pub struct Network {
    pub state: Arc<AppState>,
    audio: Arc<Mutex<Option<Arc<UdpSocket>>>>,
    remembered: Arc<Mutex<HashMap<Uuid, [u8; 32]>>>,
    scan: Arc<Notify>,
}

impl Network {
    pub fn new(state: Arc<AppState>) -> Self {
        Self {
            state,
            audio: Default::default(),
            remembered: Default::default(),
            scan: Default::default(),
        }
    }

    pub fn scan(&self) {
        self.scan.notify_one();
    }

    pub async fn serve(self) {
        tokio::spawn(self.clone().discovery());
        loop {
            match self.listen().await {
                Ok(()) => return,
                Err(error) => {
                    tracing::warn!(%error, "Network sockets unavailable");
                    let mut network = self.state.network.lock().unwrap();
                    network.ready = false;
                    network.connection.message = Some(error.to_string());
                    *self.audio.lock().unwrap() = None;
                }
            }
            time::sleep(Duration::from_secs(3)).await;
        }
    }

    async fn listen(&self) -> Result<()> {
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, CONTROL_PORT))
            .await
            .with_context(|| format!("i18n:portUnavailable|{CONTROL_PORT}"))?;
        let audio = Arc::new(
            UdpSocket::bind((Ipv4Addr::UNSPECIFIED, AUDIO_PORT))
                .await
                .with_context(|| format!("i18n:portUnavailable|{AUDIO_PORT}"))?,
        );
        {
            let mut network = self.state.network.lock().unwrap();
            *self.audio.lock().unwrap() = Some(audio);
            network.ready = true;
            network.connection.message = None;
        }
        loop {
            let (stream, addr) = listener.accept().await?;
            let SocketAddr::V4(addr) = addr else { continue };
            if let Ok((generation, cancel)) = self.reserve(None) {
                let network = self.clone();
                let ip = *addr.ip();
                tokio::spawn(async move {
                    let result = network
                        .session(stream, ip, generation, false, None, cancel)
                        .await;
                    network.finish(generation, result);
                });
            }
        }
    }

    pub fn connect(&self, ip: Ipv4Addr) -> Result<()> {
        ensure!(
            !ip.is_unspecified() && !ip.is_loopback() && !ip.is_broadcast() && !ip.is_multicast(),
            "i18n:invalidIP"
        );
        let peer = {
            let state = self.state.network.lock().unwrap();
            ensure!(!state.addresses.contains(&ip), "i18n:selfIP");
            state
                .peers
                .values()
                .find(|(peer, seen)| peer.ip == ip && seen.elapsed() < Duration::from_secs(7))
                .map(|(peer, _)| peer.clone())
                .unwrap_or(Peer {
                    id: Uuid::nil(),
                    name: ip.to_string(),
                    ip,
                })
        };
        let (generation, cancel) = self.reserve(Some(peer.clone()))?;
        let network = self.clone();
        tokio::spawn(async move {
            let result = network.outbound(peer, generation, cancel).await;
            network.finish(generation, result);
        });
        Ok(())
    }

    fn reserve(&self, peer: Option<Peer>) -> Result<(Uuid, watch::Receiver<bool>)> {
        let mut state = self.state.network.lock().unwrap();
        ensure!(state.ready, "i18n:networkUnavailable");
        ensure!(!state.installing_update, "i18n:networkUnavailable");
        ensure!(state.active.is_none(), "i18n:busySession");
        let generation = Uuid::new_v4();
        let (cancel, receiver) = watch::channel(false);
        state.connection = Connection {
            phase: Phase::Connecting,
            incoming: peer.is_none(),
            peer,
            ..Connection::default()
        };
        state.active = Some(SessionSlot {
            generation,
            cancel,
            approval: None,
        });
        Ok((generation, receiver))
    }

    pub fn stop(&self) {
        let state = self.state.network.lock().unwrap();
        if let Some(session) = &state.active {
            session.cancel.send_replace(true);
        }
    }

    pub fn approve(&self, code: &str, approved: bool) -> Result<()> {
        let mut state = self.state.network.lock().unwrap();
        ensure!(
            state.connection.code.as_deref() == Some(code),
            "i18n:pairingEnded"
        );
        let sender = state
            .active
            .as_mut()
            .and_then(|session| session.approval.take())
            .context("i18n:pairingEnded")?;
        sender
            .send(approved)
            .map_err(|_| anyhow!("i18n:pairingEnded"))?;
        state.connection.approved = approved;
        Ok(())
    }

    fn finish(&self, generation: Uuid, result: Result<()>) {
        let mut state = self.state.network.lock().unwrap();
        if state
            .active
            .as_ref()
            .is_some_and(|session| session.generation == generation)
        {
            let cancelled = state
                .active
                .take()
                .is_some_and(|session| *session.cancel.borrow());
            let message = match result {
                Err(error) if !cancelled => {
                    tracing::info!(detail = ?error, "Session ended");
                    Some(error.to_string())
                }
                _ => None,
            };
            state.connection = Connection {
                message,
                ..Connection::default()
            };
        }
    }

    async fn outbound(
        &self,
        mut peer: Peer,
        generation: Uuid,
        mut cancel: watch::Receiver<bool>,
    ) -> Result<()> {
        let mut retry_until = None;
        loop {
            let address = (peer.ip, CONTROL_PORT);
            let stream = tokio::select! {
                _ = cancelled(&mut cancel) => return Ok(()),
                result = time::timeout(Duration::from_secs(4), TcpStream::connect(address)) => result,
            };
            let attempt = match stream {
                Ok(Ok(stream)) => {
                    self.session(
                        stream,
                        peer.ip,
                        generation,
                        true,
                        Some(peer.id),
                        cancel.clone(),
                    )
                    .await
                }
                _ => Err(anyhow!("i18n:connectError")),
            };
            let Err(error) = attempt else { return Ok(()) };
            if *cancel.borrow() {
                return Ok(());
            }
            {
                let network = self.state.network.lock().unwrap();
                let connection = &network.connection;
                if connection.phase == Phase::Connected {
                    peer = connection.peer.clone().unwrap_or(peer);
                    retry_until = Some(Instant::now() + Duration::from_secs(45));
                } else if connection.phase == Phase::Pairing {
                    return Err(error);
                }
            }
            if !self.state.settings.borrow().reconnect
                || retry_until.is_none_or(|until| Instant::now() >= until)
            {
                return Err(error);
            }
            self.state.update_connection(generation, |connection| {
                *connection = Connection {
                    phase: Phase::Reconnecting,
                    peer: Some(peer.clone()),
                    ..Connection::default()
                };
            });
            tokio::select! {
                _ = cancelled(&mut cancel) => return Ok(()),
                _ = time::sleep(Duration::from_secs(2)) => {}
            }
        }
    }

    async fn session(
        &self,
        mut stream: TcpStream,
        ip: Ipv4Addr,
        generation: Uuid,
        caller: bool,
        expected: Option<Uuid>,
        mut cancel: watch::Receiver<bool>,
    ) -> Result<()> {
        stream.set_nodelay(true)?;
        let audio = self
            .audio
            .lock()
            .unwrap()
            .clone()
            .context("i18n:networkUnavailable")?;
        let audio_port = audio.local_addr()?.port();
        let (mut reader, mut writer) = stream.split();
        let handshake = async {
            let claimed = if caller {
                write_record(&mut writer, self.state.id.as_bytes()).await?;
                expected.filter(|id| !id.is_nil())
            } else {
                Some(Uuid::from_slice(&read_record(&mut reader).await?)?)
            };
            let remembered =
                claimed.and_then(|id| self.remembered.lock().unwrap().get(&id).copied());
            let secret = StaticSecret::random_from_rng(OsRng);
            let mut challenge = [0; 16];
            OsRng.fill_bytes(&mut challenge);
            let mut local = Hello {
                version: VERSION,
                capabilities: CAPABILITIES.into(),
                id: self.state.id,
                name: self.state.settings.borrow().device_name.clone(),
                audio_port,
                public_key: PublicKey::from(&secret).to_bytes(),
                challenge,
                proof: None,
            };
            if let Some(key) = &remembered {
                local.sign_resume(key);
            }
            let local_raw = serde_json::to_vec(&local)?;
            write_record(&mut writer, &commitment(&local_raw)).await?;
            let remote_commitment = read_record(&mut reader).await?;
            ensure!(remote_commitment.len() == 32);
            write_record(&mut writer, &local_raw).await?;
            let remote_raw = read_record(&mut reader).await?;
            ensure!(
                remote_commitment == commitment(&remote_raw),
                "i18n:pairingError"
            );
            let remote: Hello = serde_json::from_slice(&remote_raw)?;
            remote.validate(local.id)?;
            ensure!(claimed.is_none_or(|id| id == remote.id), "i18n:peerChanged");
            let keys = Keys::derive(&secret, &remote, caller, &local_raw, &remote_raw)?;
            let trusted = remembered
                .as_ref()
                .is_some_and(|key| remote.verify_resume(key));
            Ok::<_, anyhow::Error>((remote, keys, trusted))
        };
        let (hello, keys, trusted) = tokio::select! {
            _ = cancelled(&mut cancel) => return Ok(()),
            result = time::timeout(Duration::from_secs(6), handshake) => {
                result.context("i18n:noReply")??
            },
        };
        let peer = Peer {
            id: hello.id,
            name: hello.name,
            ip,
        };
        let mut sender = ControlCipher::new(keys.send_control.clone());
        let mut receiver = ControlCipher::new(keys.receive_control.clone());
        let approval = if trusted {
            None
        } else {
            let (send, receive) = oneshot::channel();
            let mut state = self.state.network.lock().unwrap();
            if let Some(session) = state
                .active
                .as_mut()
                .filter(|session| session.generation == generation)
            {
                session.approval = Some(send);
            }
            state.connection = Connection {
                phase: Phase::Pairing,
                peer: Some(peer.clone()),
                code: Some(keys.code.clone()),
                incoming: !caller,
                ..Connection::default()
            };
            Some(receive)
        };
        let pairing = async {
            if let Some(local) = approval {
                let incoming = receiver.receive::<Control>(&mut reader);
                tokio::pin!(incoming);
                tokio::pin!(local);
                let remote_first = tokio::select! {
                    remote = &mut incoming => {
                        check_approval(remote?)?;
                        true
                    }
                    approved = &mut local => {
                        let approved = approved.unwrap_or(false);
                        sender.send(&mut writer, &Control::Approval { approved }).await?;
                        ensure!(approved, "i18n:pairingDeclined");
                        false
                    }
                };
                if remote_first {
                    let approved = local.await.unwrap_or(false);
                    sender
                        .send(&mut writer, &Control::Approval { approved })
                        .await?;
                    ensure!(approved, "i18n:pairingDeclined");
                } else {
                    check_approval(incoming.await?)?;
                }
            } else {
                sender
                    .send(&mut writer, &Control::Approval { approved: true })
                    .await?;
                check_approval(receiver.receive(&mut reader).await?)?;
            }
            Ok::<_, anyhow::Error>(())
        };
        tokio::select! {
            _ = cancelled(&mut cancel) => return Ok(()),
            result = time::timeout(Duration::from_secs(60), pairing) => {
                result.context("i18n:pairingTimeout")??;
            }
        }
        {
            let mut remembered = self.remembered.lock().unwrap();
            if remembered.len() >= 32 && !remembered.contains_key(&peer.id) {
                remembered.clear();
            }
            remembered.insert(peer.id, keys.resume);
        }
        self.state.update_connection(generation, |connection| {
            *connection = Connection {
                phase: Phase::Connected,
                peer: Some(peer.clone()),
                ..Connection::default()
            };
        });
        let destination = SocketAddr::from((ip, hello.audio_port));
        let (messages, mut inbox) = mpsc::channel(8);
        let read = read_control(receiver, reader, messages);
        let control = async {
            let now = Instant::now();
            let mut last_reply = now;
            let mut clock = time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    _ = cancelled(&mut cancel) => {
                        let _ = time::timeout(Duration::from_millis(300), sender.send(&mut writer, &Control::Bye)).await;
                        return Ok(());
                    }
                    _ = clock.tick() => {
                        ensure!(last_reply.elapsed() < Duration::from_secs(6), "i18n:connectionLost");
                        let tick = now.elapsed().as_micros() as u64;
                        time::timeout(Duration::from_secs(2), sender.send(&mut writer, &Control::Ping { tick })).await??;
                    }
                    message = inbox.recv() => match message {
                        Some(Control::Ping { tick }) => {
                            time::timeout(Duration::from_secs(2), sender.send(&mut writer, &Control::Pong { tick })).await??;
                        }
                        Some(Control::Pong { tick }) => {
                            last_reply = Instant::now();
                            let rtt = (now.elapsed().as_micros() as u64).saturating_sub(tick) as f64 / 1000.0;
                            self.state.update_connection(generation, |connection| connection.metrics.rtt_ms = Some(rtt));
                        }
                        Some(Control::Bye) => return Ok(()),
                        _ => return Err(anyhow!("i18n:connectionLost")),
                    }
                }
            }
        };
        let result = tokio::select! {
            result = read => result.context("i18n:connectionLost"),
            result = control => result,
            result = media::run(self.state.clone(), generation, audio, destination, &keys) => result.context("i18n:audioInterrupted"),
        };
        if result.is_ok() {
            self.remembered.lock().unwrap().remove(&peer.id);
        }
        result
    }

    async fn discovery(self) {
        let mut socket = None;
        let mut buffer = [0; 513];
        let mut clock = time::interval(Duration::from_secs(2));
        loop {
            tokio::select! {
                _ = async { tokio::select! { _ = clock.tick() => {}, _ = self.scan.notified() => {} } } => {
                    let mut addresses = Vec::new();
                    let mut broadcast = vec![Ipv4Addr::BROADCAST];
                    if let Ok(interfaces) = if_addrs::get_if_addrs() {
                        for interface in interfaces.into_iter().filter(|item| !item.is_loopback()) {
                            if let if_addrs::IfAddr::V4(address) = interface.addr {
                                addresses.push(address.ip);
                                if let Some(target) = address.broadcast { broadcast.push(target); }
                            }
                        }
                    }
                    addresses.sort();
                    addresses.dedup();
                    let announce = {
                        let mut state = self.state.network.lock().unwrap();
                        state.addresses = addresses;
                        state.peers.retain(|_, (_, seen)| seen.elapsed() < Duration::from_secs(7));
                        state.ready && self.state.settings.borrow().discovery
                    };
                    if !announce { continue; }
                    if socket.is_none() {
                        match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT)).await {
                            Ok(bound) if bound.set_broadcast(true).is_ok() => socket = Some(bound),
                            Ok(_) => {}
                            Err(error) => tracing::debug!(%error, "Discovery socket unavailable"),
                        }
                    }
                    if let Some(socket) = &socket {
                        let data = Announce { protocol: "WaveCast".into(), version: VERSION, id: self.state.id, name: self.state.settings.borrow().device_name.clone() };
                        let packet = serde_json::to_vec(&data).unwrap();
                        broadcast.sort();
                        broadcast.dedup();
                        for ip in broadcast {
                            let _ = socket.send_to(&packet, (ip, DISCOVERY_PORT)).await;
                        }
                    }
                }
                packet = receive_discovery(socket.as_ref(), &mut buffer) => {
                    if let Ok((size, SocketAddr::V4(addr))) = packet
                        && size <= 512 && self.state.settings.borrow().discovery
                        && let Ok(data) = serde_json::from_slice::<Announce>(&buffer[..size])
                        && data.protocol == "WaveCast" && data.version == VERSION
                        && data.id != self.state.id && !data.id.is_nil() && valid_name(&data.name)
                    {
                        let mut state = self.state.network.lock().unwrap();
                        if state.peers.len() < 128 || state.peers.contains_key(&data.id) {
                            state.peers.insert(data.id, (Peer { id: data.id, name: data.name, ip: *addr.ip() }, Instant::now()));
                        }
                    } else if packet.is_err() {
                        socket = None;
                    }
                }
            }
        }
    }
}

fn check_approval(control: Control) -> Result<()> {
    ensure!(
        matches!(control, Control::Approval { approved: true }),
        "i18n:pairingDeclined"
    );
    Ok(())
}

async fn cancelled(cancel: &mut watch::Receiver<bool>) {
    let _ = cancel.wait_for(|value| *value).await;
}

async fn read_control(
    mut cipher: ControlCipher,
    mut reader: impl AsyncRead + Unpin,
    messages: mpsc::Sender<Control>,
) -> Result<()> {
    loop {
        let message = cipher.receive(&mut reader).await?;
        let bye = matches!(message, Control::Bye);
        messages.send(message).await?;
        if bye {
            return std::future::pending().await;
        }
    }
}

async fn receive_discovery(
    socket: Option<&UdpSocket>,
    buffer: &mut [u8],
) -> std::io::Result<(usize, SocketAddr)> {
    match socket {
        Some(socket) => socket.recv_from(buffer).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_node(path: std::path::PathBuf) -> Network {
        let state = AppState::load(path);
        let mut settings = state.settings.borrow().clone();
        for channel in &mut settings.channels {
            channel.enabled = false;
        }
        // Send Opus silence through both UDP paths without opening capture or playback.
        settings.source = crate::model::SourceMode::Application;
        settings.channels[0].enabled = true;
        state.set_settings(settings).unwrap();
        state.network.lock().unwrap().ready = true;
        let node = Network::new(state);
        *node.audio.lock().unwrap() = Some(Arc::new(
            UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap(),
        ));
        node
    }

    async fn await_state(node: &Network, predicate: impl Fn(&Connection) -> bool) -> Connection {
        time::timeout(Duration::from_secs(8), async {
            loop {
                let state = node.state.snapshot().connection;
                if predicate(&state) {
                    return state;
                }
                time::sleep(Duration::from_millis(15)).await;
            }
        })
        .await
        .expect("session did not reach the expected state")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn both_peers_confirm_pairing_before_bidirectional_udp() {
        let dir = std::env::temp_dir().join(format!("wavecast-{}", Uuid::new_v4()));
        let a = test_node(dir.join("a.toml")).await;
        let b = test_node(dir.join("b.toml")).await;
        let local_meter = tokio::spawn(media::monitor_local_audio(a.state.clone()));
        time::timeout(Duration::from_secs(2), async {
            while a.state.capture_owner.try_lock().is_ok() {
                time::sleep(Duration::from_millis(15)).await;
            }
        })
        .await
        .expect("local metering did not start");
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let (id_a, stop_a) = a
            .reserve(Some(Peer {
                id: b.state.id,
                name: "B".into(),
                ip: Ipv4Addr::LOCALHOST,
            }))
            .unwrap();
        let (id_b, stop_b) = b.reserve(None).unwrap();
        let a_task = {
            let a = a.clone();
            let expected = b.state.id;
            tokio::spawn(async move {
                let stream = TcpStream::connect(address).await.unwrap();
                a.session(
                    stream,
                    Ipv4Addr::LOCALHOST,
                    id_a,
                    true,
                    Some(expected),
                    stop_a,
                )
                .await
            })
        };
        let b_task = {
            let b = b.clone();
            tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                b.session(stream, Ipv4Addr::LOCALHOST, id_b, false, None, stop_b)
                    .await
            })
        };
        let a_waiting = await_state(&a, |state| state.phase == Phase::Pairing).await;
        let b_waiting = await_state(&b, |state| state.phase == Phase::Pairing).await;
        let code = a_waiting.code.unwrap();
        assert_eq!(Some(&code), b_waiting.code.as_ref());
        a.approve(&code, true).unwrap();
        assert!(b.state.snapshot().connection.phase == Phase::Pairing);
        b.approve(&code, true).unwrap();
        await_state(&a, |state| {
            state.phase == Phase::Connected && state.metrics.rtt_ms.is_some()
        })
        .await;
        await_state(&b, |state| {
            state.phase == Phase::Connected && state.metrics.rtt_ms.is_some()
        })
        .await;
        for node in [&a, &b] {
            await_state(node, |state| {
                state.metrics.sent > 0
                    && state.metrics.received > 0
                    && state.metrics.loss_percent.is_some()
            })
            .await;
        }
        a.stop();
        let (a_result, b_result) = time::timeout(Duration::from_secs(3), async {
            tokio::join!(a_task, b_task)
        })
        .await
        .unwrap();
        a_result.unwrap().unwrap();
        b_result.unwrap().unwrap();
        a.finish(id_a, Ok(()));
        time::timeout(Duration::from_secs(2), async {
            while a.state.capture_owner.try_lock().is_ok() {
                time::sleep(Duration::from_millis(15)).await;
            }
        })
        .await
        .expect("local metering did not resume after disconnecting");
        local_meter.abort();
        let _ = local_meter.await;
        std::fs::remove_file(dir.join("a.toml")).unwrap();
        std::fs::remove_file(dir.join("b.toml")).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
