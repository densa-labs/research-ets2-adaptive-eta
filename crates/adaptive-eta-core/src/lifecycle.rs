#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleEvent {
    SourceConnected {
        source_epoch: u64,
    },
    SourceDisconnected,
    Paused,
    Started,
    TimerRestart,
    NavigationBecameInvalid,
    NavigationBecameValid,
    JobChanged,
    JobEnded,
    FerryUsed,
    TrainUsed,
    /// Supplied by an adapter when it can identify a load/restart operation.
    LoadOrRestart,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundaryReason {
    SourceConnected,
    SourceDisconnected,
    SourceEpochChanged,
    Paused,
    Started,
    TimerRestart,
    NavigationInvalid,
    NavigationBecameValid,
    JobChanged,
    JobEnded,
    FerryUsed,
    TrainUsed,
    LoadOrRestart,
    InvalidTelemetry,
    TimestampRegressed,
    SequenceRegressed,
    TelemetryGap,
    NavigationDistanceIncreased,
    NavigationTimeIncreased,
    OdometerRegressed,
    ProgressMismatch,
    LongStationary,
}

impl LifecycleEvent {
    #[must_use]
    pub const fn boundary_reason(self) -> BoundaryReason {
        match self {
            Self::SourceConnected { .. } => BoundaryReason::SourceConnected,
            Self::SourceDisconnected => BoundaryReason::SourceDisconnected,
            Self::Paused => BoundaryReason::Paused,
            Self::Started => BoundaryReason::Started,
            Self::TimerRestart => BoundaryReason::TimerRestart,
            Self::NavigationBecameInvalid => BoundaryReason::NavigationInvalid,
            Self::NavigationBecameValid => BoundaryReason::NavigationBecameValid,
            Self::JobChanged => BoundaryReason::JobChanged,
            Self::JobEnded => BoundaryReason::JobEnded,
            Self::FerryUsed => BoundaryReason::FerryUsed,
            Self::TrainUsed => BoundaryReason::TrainUsed,
            Self::LoadOrRestart => BoundaryReason::LoadOrRestart,
        }
    }
}
