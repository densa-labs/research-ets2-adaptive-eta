use serde::{Deserialize, Serialize};

/// Raw-ish frame containing only the SCS-derived concepts needed downstream.
/// `None` means the channel was explicitly unavailable; it is never coerced to
/// a numeric zero.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawFrame {
    pub source_epoch: u64,
    pub sequence: u64,
    pub paused_simulation_time_us: u64,
    pub local_scale: Option<f64>,
    pub game_time_minutes: Option<u32>,
    pub navigation_distance_m: Option<f64>,
    pub navigation_time_sec: Option<f64>,
    pub speed_mps: Option<f64>,
    pub odometer_km: Option<f64>,
    #[serde(default)]
    pub timer_restart: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "data",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum RawInput {
    SourceConnected { source_epoch: u64 },
    SourceDisconnected,
    Frame(RawFrame),
    Paused,
    Started,
    TimerRestart,
    LoadOrRestart,
    JobChanged,
    JobEnded,
    FerryUsed,
    TrainUsed,
}
