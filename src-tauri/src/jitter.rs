use std::{collections::BTreeMap, time::Instant};

const WINDOW: u32 = 32;

pub enum Playout {
    Packet(Vec<u8>),
    Missing,
    Waiting,
}

pub struct Jitter {
    packets: BTreeMap<u32, Vec<u8>>,
    next: Option<u32>,
    consumed: Option<u32>,
    primed: bool,
    target: u32,
    last_arrival: Option<(u32, Instant)>,
    pub arrival_jitter_ms: f64,
    pub played: u64,
    pub lost: u64,
}

impl Jitter {
    pub fn new() -> Self {
        Self {
            packets: BTreeMap::new(),
            next: None,
            consumed: None,
            primed: false,
            target: 0,
            last_arrival: None,
            arrival_jitter_ms: 0.0,
            played: 0,
            lost: 0,
        }
    }

    pub fn target_frames(&self, base_ms: u32) -> u32 {
        (base_ms / 10 + (self.arrival_jitter_ms / 10.0).round() as u32).clamp(2, 12)
    }

    pub fn push(&mut self, sequence: u32, payload: &[u8], now: Instant) -> bool {
        if self.consumed.is_some_and(|seen| sequence <= seen)
            || self.packets.contains_key(&sequence)
        {
            return false;
        }
        if let Some((last, arrival)) = self.last_arrival {
            if sequence > last {
                let elapsed = now.duration_since(arrival).as_secs_f64() * 1000.0;
                let expected = f64::from(sequence - last) * 10.0;
                if elapsed < 240.0 && expected < 240.0 {
                    let variation = (elapsed - expected).abs();
                    self.arrival_jitter_ms += (variation - self.arrival_jitter_ms) / 16.0;
                }
                self.last_arrival = Some((sequence, now));
            }
        } else {
            self.last_arrival = Some((sequence, now));
        }
        if self
            .next
            .is_some_and(|next| sequence >= next.saturating_add(WINDOW))
        {
            self.consumed = Some(sequence - 1);
            self.packets.clear();
            self.next = None;
            self.primed = false;
        }
        if !self.primed {
            self.next = Some(self.next.map_or(sequence, |next| next.min(sequence)));
        }
        self.packets.insert(sequence, payload.to_vec());
        true
    }

    pub fn tick(&mut self, base_ms: u32) -> Playout {
        let Some(sequence) = self.next else {
            return Playout::Waiting;
        };
        let target = self.target_frames(base_ms);
        let ahead = self
            .packets
            .last_key_value()
            .map_or(0, |(&last, _)| last.saturating_sub(sequence) + 1);
        if ahead == 0 {
            // A stalled or paused sender has not skipped a sequence number yet.
            // Repeating PLC here plays old audio before that same frame arrives.
            self.primed = false;
            return Playout::Waiting;
        }
        if target > self.target && ahead > 0 && ahead < target {
            self.primed = false;
        }
        self.target = target;
        if !self.primed && ahead < target {
            return Playout::Waiting;
        }
        self.primed = true;
        // Dropping a frame corrects sustained sender/renderer clock drift or a burst.
        let sequence = if ahead > target + 3 {
            self.packets.remove(&sequence);
            sequence + 1
        } else {
            sequence
        };
        if let Some(payload) = self.packets.remove(&sequence) {
            self.advance(sequence);
            self.played += 1;
            return Playout::Packet(payload);
        }
        self.advance(sequence);
        self.lost += 1;
        Playout::Missing
    }

    fn advance(&mut self, sequence: u32) {
        self.consumed = Some(sequence);
        self.next = sequence.checked_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reorders_packets_and_rejects_replay() {
        let mut buffer = Jitter::new();
        let now = Instant::now();
        assert!(buffer.push(2, b"second", now));
        assert!(matches!(buffer.tick(20), Playout::Waiting));
        assert!(buffer.push(1, b"first", now));
        assert!(matches!(buffer.tick(20), Playout::Packet(data) if data == b"first"));
        assert!(!buffer.push(1, b"replay", now));
        assert!(matches!(buffer.tick(20), Playout::Packet(data) if data == b"second"));
        assert!(!buffer.push(2, b"late", now));
    }

    #[test]
    fn bounds_backlog_and_conceals_missing_frame() {
        let mut buffer = Jitter::new();
        let now = Instant::now();
        buffer.push(4, b"a", now);
        buffer.push(6, b"c", now);
        assert!(matches!(buffer.tick(20), Playout::Packet(_)));
        assert!(matches!(buffer.tick(20), Playout::Missing));
        assert_eq!(buffer.lost, 1);
        assert!(matches!(buffer.tick(20), Playout::Packet(_)));
        buffer.push(999, b"after gap", now);
        assert!(!buffer.push(7, b"stale", now));
        assert!(matches!(buffer.tick(20), Playout::Waiting));
    }

    #[test]
    fn stalls_rebuffer_without_repeating_the_previous_audio() {
        let mut buffer = Jitter::new();
        let now = Instant::now();
        buffer.push(1, b"one", now);
        buffer.push(2, b"two", now);
        assert!(matches!(buffer.tick(20), Playout::Packet(_)));
        assert!(matches!(buffer.tick(20), Playout::Packet(_)));
        for _ in 0..12 {
            assert!(matches!(buffer.tick(20), Playout::Waiting));
        }
        assert_eq!(buffer.lost, 0);
        buffer.push(3, b"three", now);
        assert!(matches!(buffer.tick(20), Playout::Waiting));
        buffer.push(4, b"four", now);
        assert!(matches!(buffer.tick(20), Playout::Packet(data) if data == b"three"));
        assert!(!buffer.push(3, b"late", now));
        assert!(matches!(buffer.tick(20), Playout::Packet(data) if data == b"four"));
    }
}
