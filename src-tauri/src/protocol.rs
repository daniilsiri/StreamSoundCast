use anyhow::{Context, Result, anyhow, ensure};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce, aead::AeadInPlace};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use uuid::Uuid;
use x25519_dalek::{PublicKey, StaticSecret};

pub const VERSION: u8 = 1;
pub const MAX_OPUS: usize = 1200;
pub const MAX_PACKET: usize = 24 + MAX_OPUS + 16;
pub const FRAME_SAMPLES: u64 = 480;
pub const CAPABILITIES: &str = "opus/48000/10/stereo+mono";

#[derive(Clone, Serialize, Deserialize)]
pub struct Hello {
    pub version: u8,
    pub capabilities: String,
    pub id: Uuid,
    pub name: String,
    pub audio_port: u16,
    pub public_key: [u8; 32],
    pub challenge: [u8; 16],
    pub proof: Option<[u8; 32]>,
}

pub fn valid_name(name: &str) -> bool {
    !name.trim().is_empty() && name.chars().count() <= 48 && !name.chars().any(char::is_control)
}

impl Hello {
    pub fn validate(&self, local_id: Uuid) -> Result<()> {
        ensure!(
            self.version == VERSION && self.capabilities == CAPABILITIES,
            "i18n:incompatible"
        );
        ensure!(
            self.audio_port != 0
                && !self.id.is_nil()
                && self.id != local_id
                && valid_name(&self.name),
            "i18n:peerDataError"
        );
        Ok(())
    }

    fn resume_mac(&self, key: &[u8; 32]) -> Hmac<Sha256> {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).unwrap();
        mac.update(b"WaveCast v1 resume");
        mac.update(self.id.as_bytes());
        mac.update(&self.public_key);
        mac.update(&self.challenge);
        mac
    }

    pub fn sign_resume(&mut self, key: &[u8; 32]) {
        self.proof = Some(self.resume_mac(key).finalize().into_bytes().into());
    }

    pub fn verify_resume(&self, key: &[u8; 32]) -> bool {
        self.proof
            .as_ref()
            .is_some_and(|proof| self.resume_mac(key).verify_slice(proof).is_ok())
    }
}

pub fn commitment(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

pub struct Keys {
    pub send_media: ChaCha20Poly1305,
    pub receive_media: ChaCha20Poly1305,
    pub send_control: ChaCha20Poly1305,
    pub receive_control: ChaCha20Poly1305,
    pub resume: [u8; 32],
    pub code: String,
    pub send_stream: u32,
    pub receive_stream: u32,
}

impl Keys {
    pub fn derive(
        secret: &StaticSecret,
        peer: &Hello,
        caller: bool,
        local_raw: &[u8],
        remote_raw: &[u8],
    ) -> Result<Self> {
        let shared = secret.diffie_hellman(&PublicKey::from(peer.public_key));
        ensure!(shared.was_contributory(), "i18n:pairingError");
        let (first, second) = if caller {
            (local_raw, remote_raw)
        } else {
            (remote_raw, local_raw)
        };
        let mut transcript = Sha256::new();
        transcript.update(b"WaveCast v1 pairing");
        transcript.update(commitment(first));
        transcript.update(commitment(second));
        let root = Hkdf::<Sha256>::new(Some(&transcript.finalize()), shared.as_bytes());
        let mut material = [0; 168];
        root.expand(
            b"media, control, resume, short authentication string",
            &mut material,
        )
        .map_err(|_| anyhow!("i18n:pairingError"))?;
        let (tx, rx, send_stream, receive_stream) =
            if caller { (0, 32, 1, 3) } else { (32, 0, 3, 1) };
        let cipher =
            |offset| ChaCha20Poly1305::new_from_slice(&material[offset..offset + 32]).unwrap();
        let short = u64::from_be_bytes(material[160..168].try_into().unwrap()) % 1_000_000;
        Ok(Self {
            send_media: cipher(tx),
            receive_media: cipher(rx),
            send_control: cipher(tx + 64),
            receive_control: cipher(rx + 64),
            resume: material[128..160].try_into().unwrap(),
            code: format!("{short:06}"),
            send_stream,
            receive_stream,
        })
    }
}

fn nonce(stream: u32, sequence: u64) -> [u8; 12] {
    let mut result = [0; 12];
    result[..4].copy_from_slice(&stream.to_be_bytes());
    result[4..].copy_from_slice(&sequence.to_be_bytes());
    result
}

pub fn seal_audio(
    cipher: &ChaCha20Poly1305,
    stream: u32,
    seq: u32,
    payload: &[u8],
    wire: &mut [u8; MAX_PACKET],
) -> usize {
    wire[..4].copy_from_slice(b"LANA");
    wire[4] = VERSION;
    wire[5] = 1;
    wire[6..10].copy_from_slice(&stream.to_be_bytes());
    wire[10..14].copy_from_slice(&seq.to_be_bytes());
    wire[14..22].copy_from_slice(&(u64::from(seq) * FRAME_SAMPLES).to_be_bytes());
    wire[22..24].copy_from_slice(&u16::try_from(payload.len()).unwrap().to_be_bytes());
    let end = 24 + payload.len();
    wire[24..end].copy_from_slice(payload);
    let (header, data) = wire.split_at_mut(24);
    let tag = cipher
        .encrypt_in_place_detached(
            Nonce::from_slice(&nonce(stream, seq.into())),
            header,
            &mut data[..payload.len()],
        )
        .unwrap();
    wire[end..end + 16].copy_from_slice(&tag);
    end + 16
}

pub fn open_audio<'a>(
    cipher: &ChaCha20Poly1305,
    first: u32,
    wire: &'a mut [u8],
) -> Option<(usize, u32, &'a [u8])> {
    if !(41..=MAX_PACKET).contains(&wire.len()) || &wire[..6] != b"LANA\x01\x01" {
        return None;
    }
    let stream = u32::from_be_bytes(wire[6..10].try_into().ok()?);
    let lane = stream.checked_sub(first)?;
    let seq = u32::from_be_bytes(wire[10..14].try_into().ok()?);
    let timestamp = u64::from_be_bytes(wire[14..22].try_into().ok()?);
    let len = usize::from(u16::from_be_bytes(wire[22..24].try_into().ok()?));
    if lane > 1 || timestamp != u64::from(seq) * FRAME_SAMPLES || wire.len() != 40 + len {
        return None;
    }
    let (header, rest) = wire.split_at_mut(24);
    let (payload, tag) = rest.split_at_mut(len);
    cipher
        .decrypt_in_place_detached(
            Nonce::from_slice(&nonce(stream, seq.into())),
            header,
            payload,
            (&*tag).into(),
        )
        .ok()?;
    Some((lane as usize, seq, payload))
}

pub async fn write_record(out: &mut (impl AsyncWrite + Unpin), data: &[u8]) -> Result<()> {
    ensure!(data.len() <= 2048);
    out.write_u16(data.len() as u16).await?;
    out.write_all(data).await?;
    Ok(())
}

pub async fn read_record(input: &mut (impl AsyncRead + Unpin)) -> Result<Vec<u8>> {
    let len = usize::from(input.read_u16().await?);
    ensure!((1..=2048).contains(&len), "i18n:peerDataError");
    let mut data = vec![0; len];
    input.read_exact(&mut data).await?;
    Ok(data)
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Control {
    Approval { approved: bool },
    Ping { tick: u64 },
    Pong { tick: u64 },
    Bye,
}

pub struct ControlCipher {
    cipher: ChaCha20Poly1305,
    sequence: u64,
}

impl ControlCipher {
    pub fn new(cipher: ChaCha20Poly1305) -> Self {
        Self {
            cipher,
            sequence: 0,
        }
    }

    pub async fn send(
        &mut self,
        out: &mut (impl AsyncWrite + Unpin),
        value: &impl Serialize,
    ) -> Result<()> {
        let mut data = serde_json::to_vec(value)?;
        let iv = nonce(0, self.sequence);
        let tag = self
            .cipher
            .encrypt_in_place_detached(Nonce::from_slice(&iv), b"WaveCast control", &mut data)
            .map_err(|_| anyhow!("i18n:pairingError"))?;
        self.sequence += 1;
        data.extend_from_slice(&tag);
        write_record(out, &data).await
    }

    pub async fn receive<T: DeserializeOwned>(
        &mut self,
        input: &mut (impl AsyncRead + Unpin),
    ) -> Result<T> {
        let mut data = read_record(input).await?;
        ensure!(data.len() > 16, "i18n:peerDataError");
        let size = data.len() - 16;
        let (message, tag) = data.split_at_mut(size);
        let iv = nonce(0, self.sequence);
        self.cipher
            .decrypt_in_place_detached(
                Nonce::from_slice(&iv),
                b"WaveCast control",
                message,
                (&*tag).into(),
            )
            .map_err(|_| anyhow!("i18n:pairingError"))?;
        self.sequence += 1;
        serde_json::from_slice(message).context("i18n:peerDataError")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authenticated_media_rejects_changes_and_old_session_keys() {
        let cipher = ChaCha20Poly1305::new_from_slice(&[1; 32]).unwrap();
        let other = ChaCha20Poly1305::new_from_slice(&[2; 32]).unwrap();
        let mut datagram = [0; MAX_PACKET];
        let len = seal_audio(&cipher, 4, 42, b"opus", &mut datagram);
        assert!(open_audio(&other, 3, &mut datagram[..len]).is_none());
        let mut damaged = datagram;
        damaged[24] ^= 1;
        assert!(open_audio(&cipher, 3, &mut damaged[..len]).is_none());
        assert_eq!(
            open_audio(&cipher, 3, &mut datagram[..len]),
            Some((1, 42, b"opus".as_slice()))
        );
    }
}
