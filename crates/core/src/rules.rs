//! How the queue moves (`nori_player::queue`), as the platform asks: what is fetched ahead, what to do
//! when a song will not play, and the previous and repeat buttons (the queue itself is playlist.rs). One call per user action or player event.

use nori_player::queue::{self as q, OnError, PlaybackError};

/// The songs coming up that are fetched ahead, as [first, last] positions after the playing one (0);
/// empty when none.
#[uniffi::export]
pub fn queue_precache(count: u32, mixing: bool, shuffling: bool) -> Vec<u32> {
    q::precache_range(count as usize, mixing, shuffling).map_or(Vec::new(), |(a, b)| vec![a as u32, b as u32])
}

/// How many songs coming up (the playing one included) are measured ahead for AutoMix.
#[uniffi::export]
pub fn queue_measure_ahead() -> u32 {
    q::MEASURE_AHEAD as u32
}

/// What to do when a song will not play. `kind`: 0 the output refused it, 1 the network, 2 anything else.
/// Returns 0 give up offload, 1 hand to the offline bridge, 2 skip, 3 stop.
#[uniffi::export]
pub fn queue_on_error(kind: u8, offload_refused: bool, bridge: bool, skip_on_error: bool, has_next: bool, errors_in_a_row: u32) -> u8 {
    let kind = match kind {
        0 => PlaybackError::Output,
        1 => PlaybackError::Network,
        _ => PlaybackError::Other,
    };
    match q::on_error(kind, offload_refused, bridge, skip_on_error, has_next, errors_in_a_row) {
        OnError::GiveUpOffload => 0,
        OnError::Bridge => 1,
        OnError::Skip => 2,
        OnError::Stop => 3,
    }
}

/// Whether previous restarts the song playing (else the player's own previous decides).
#[uniffi::export]
pub fn queue_previous_restarts(position_ms: i64, has_previous: bool, always_skips: bool) -> bool {
    q::previous_restarts(position_ms, has_previous, always_skips)
}

/// The repeat mode after the button (media3's numbering: off 0, one 1, all 2).
#[uniffi::export]
pub fn queue_next_repeat(mode: u8) -> u8 {
    q::next_repeat(mode)
}

/// The output's rebuild bookkeeping for the equalizer screen and settings changes; see
/// `nori_player::transport::Chain`. Each method says whether to rebuild the output now.
#[derive(uniffi::Object, Default)]
pub struct ChainState(parking_lot::Mutex<nori_player::transport::Chain>);

#[uniffi::export]
impl ChainState {
    #[uniffi::constructor]
    pub fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self::default())
    }

    pub fn tuning(&self, on: bool, eq: bool, idle: bool, playing: bool) -> bool {
        self.0.lock().tuning(on, eq, idle, playing) == nori_player::transport::ChainAct::Rebuild
    }

    /// A rebuild deferred to the boundary; true when it was not already waiting.
    pub fn defer(&self) -> bool {
        self.0.lock().defer()
    }

    pub fn boundary(&self, repeat_one: bool) -> bool {
        self.0.lock().boundary(repeat_one) == nori_player::transport::ChainAct::Rebuild
    }

    pub fn paused(&self) -> bool {
        self.0.lock().paused() == nori_player::transport::ChainAct::Rebuild
    }

    pub fn is_tuning(&self) -> bool {
        self.0.lock().tuning
    }

    pub fn bursting(&self, offloaded: bool) -> bool {
        self.0.lock().bursting(offloaded)
    }
}

/// The sleep timer set to `songs` songs (or the end of this one): [pause at the end of this song (0/1),
/// song changes still to go].
#[uniffi::export]
pub fn sleep_after(songs: u32, end_of_track: bool) -> Vec<u32> {
    let (pause, left) = nori_player::transport::sleep_after(songs, end_of_track);
    vec![pause as u32, left]
}

/// A song change with `left` to go: [changes still to go, pause at the end of this song (0/1)].
#[uniffi::export]
pub fn sleep_song_changed(left: u32) -> Vec<u32> {
    let (left, pause) = nori_player::transport::sleep_song_changed(left);
    vec![left, pause as u32]
}

/// The sleep timer in minutes as [delay ms, slack ms].
#[uniffi::export]
pub fn sleep_delay(minutes: u32) -> Vec<i64> {
    let (d, s) = nori_player::transport::sleep_delay_ms(minutes);
    vec![d, s]
}

/// How much the player reads ahead: [min buffer ms, max ms, to start ms, to resume ms, target bytes].
#[uniffi::export]
pub fn load_control(memory_class_mb: u32) -> Vec<i64> {
    nori_player::transport::load_control(memory_class_mb).to_vec()
}
