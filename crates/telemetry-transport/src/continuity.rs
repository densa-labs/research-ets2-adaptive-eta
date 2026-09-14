use std::collections::HashSet;

use crate::{Envelope, SenderInstance};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContinuityDiagnostic {
    NewSender {
        previous: SenderInstance,
        current: SenderInstance,
    },
    Gap {
        expected: u64,
        received: u64,
        missing: u64,
    },
    Duplicate {
        ordinal: u64,
    },
    OutOfOrder {
        expected: u64,
        received: u64,
    },
    OldSender {
        sender: SenderInstance,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContinuityOutcome {
    pub input: Option<telemetry_adapter::RawInput>,
    pub requires_boundary: bool,
    pub diagnostic: Option<ContinuityDiagnostic>,
}

#[derive(Clone, Debug, Default)]
pub struct ContinuityTracker {
    current_sender: Option<SenderInstance>,
    next_ordinal: Option<u64>,
    retired_senders: HashSet<SenderInstance>,
}

impl ContinuityTracker {
    #[must_use]
    pub fn observe(&mut self, envelope: Envelope) -> ContinuityOutcome {
        if self.retired_senders.contains(&envelope.sender) {
            return dropped(
                false,
                ContinuityDiagnostic::OldSender {
                    sender: envelope.sender,
                },
            );
        }
        let Some(current) = self.current_sender else {
            self.current_sender = Some(envelope.sender);
            self.next_ordinal = envelope.ordinal.checked_add(1);
            return accepted(envelope.input, false, None);
        };
        if current != envelope.sender {
            self.retired_senders.insert(current);
            self.current_sender = Some(envelope.sender);
            self.next_ordinal = envelope.ordinal.checked_add(1);
            return accepted(
                envelope.input,
                true,
                Some(ContinuityDiagnostic::NewSender {
                    previous: current,
                    current: envelope.sender,
                }),
            );
        }

        let Some(expected) = self.next_ordinal else {
            return dropped(
                true,
                ContinuityDiagnostic::OutOfOrder {
                    expected: u64::MAX,
                    received: envelope.ordinal,
                },
            );
        };
        if envelope.ordinal == expected {
            self.next_ordinal = envelope.ordinal.checked_add(1);
            return accepted(envelope.input, false, None);
        }
        if envelope.ordinal == expected.saturating_sub(1) {
            return dropped(
                false,
                ContinuityDiagnostic::Duplicate {
                    ordinal: envelope.ordinal,
                },
            );
        }
        if envelope.ordinal > expected {
            self.next_ordinal = envelope.ordinal.checked_add(1);
            return accepted(
                envelope.input,
                true,
                Some(ContinuityDiagnostic::Gap {
                    expected,
                    received: envelope.ordinal,
                    missing: envelope.ordinal - expected,
                }),
            );
        }
        dropped(
            true,
            ContinuityDiagnostic::OutOfOrder {
                expected,
                received: envelope.ordinal,
            },
        )
    }
}

const fn accepted(
    input: telemetry_adapter::RawInput,
    requires_boundary: bool,
    diagnostic: Option<ContinuityDiagnostic>,
) -> ContinuityOutcome {
    ContinuityOutcome {
        input: Some(input),
        requires_boundary,
        diagnostic,
    }
}

const fn dropped(requires_boundary: bool, diagnostic: ContinuityDiagnostic) -> ContinuityOutcome {
    ContinuityOutcome {
        input: None,
        requires_boundary,
        diagnostic: Some(diagnostic),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use telemetry_adapter::RawInput;

    fn envelope(sender_byte: u8, ordinal: u64) -> Envelope {
        Envelope {
            sender: SenderInstance([sender_byte; 16]),
            ordinal,
            input: RawInput::Paused,
        }
    }

    #[test]
    fn sequential_delivery_is_continuous() {
        let mut tracker = ContinuityTracker::default();
        assert!(!tracker.observe(envelope(1, 1)).requires_boundary);
        assert!(!tracker.observe(envelope(1, 2)).requires_boundary);
    }

    #[test]
    fn gaps_duplicates_and_reordering_are_deterministic() {
        let mut tracker = ContinuityTracker::default();
        let _ = tracker.observe(envelope(1, 1));
        let gap = tracker.observe(envelope(1, 5));
        assert!(gap.requires_boundary);
        assert!(matches!(
            gap.diagnostic,
            Some(ContinuityDiagnostic::Gap { missing: 3, .. })
        ));
        let duplicate = tracker.observe(envelope(1, 5));
        assert!(!duplicate.requires_boundary);
        assert_eq!(duplicate.input, None);
        let old = tracker.observe(envelope(1, 3));
        assert!(old.requires_boundary);
        assert_eq!(old.input, None);
    }

    #[test]
    fn new_sender_boundaries_and_old_sender_is_never_reactivated() {
        let mut tracker = ContinuityTracker::default();
        let _ = tracker.observe(envelope(1, 50));
        let replacement = tracker.observe(envelope(2, 1));
        assert!(replacement.requires_boundary);
        assert!(replacement.input.is_some());
        let old = tracker.observe(envelope(1, 51));
        assert_eq!(old.input, None);
        assert!(matches!(
            old.diagnostic,
            Some(ContinuityDiagnostic::OldSender { .. })
        ));
    }

    #[test]
    fn a_fresh_tracker_accepts_runtime_restart_midstream_safely() {
        let outcome = ContinuityTracker::default().observe(envelope(1, 9_000));
        assert!(outcome.input.is_some());
        assert!(!outcome.requires_boundary);
    }
}
