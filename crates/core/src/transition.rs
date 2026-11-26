use crate::model::SessionStatus;

/// Memeriksa apakah transisi status sesi dari `from` ke `to` diizinkan sesuai DATA_MODEL §5.
#[must_use]
pub const fn can_transition(from: SessionStatus, to: SessionStatus) -> bool {
    use SessionStatus as S;
    matches!(
        (from, to),
        (
            S::Running,
            S::Paused | S::WaitingApproval | S::Tripped | S::Hibernated | S::Completed | S::Failed
        ) | (S::Paused, S::Running | S::Tripped | S::Failed)
            | (S::WaitingApproval, S::Running | S::Failed | S::Tripped)
            | (S::Hibernated, S::Running)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const ALL_STATUSES: [SessionStatus; 7] = [
        SessionStatus::Running,
        SessionStatus::Paused,
        SessionStatus::WaitingApproval,
        SessionStatus::Hibernated,
        SessionStatus::Completed,
        SessionStatus::Tripped,
        SessionStatus::Failed,
    ];

    fn is_in_spec_table(from: SessionStatus, to: SessionStatus) -> bool {
        use SessionStatus as S;
        match from {
            S::Running => matches!(
                to,
                S::Paused
                    | S::WaitingApproval
                    | S::Tripped
                    | S::Hibernated
                    | S::Completed
                    | S::Failed
            ),
            S::Paused => matches!(to, S::Running | S::Tripped | S::Failed),
            S::WaitingApproval => matches!(to, S::Running | S::Failed | S::Tripped),
            S::Hibernated => matches!(to, S::Running),
            S::Completed | S::Tripped | S::Failed => false,
        }
    }

    #[test]
    fn allowed_transitions_match_spec() {
        for &from in &ALL_STATUSES {
            for &to in &ALL_STATUSES {
                assert_eq!(
                    can_transition(from, to),
                    is_in_spec_table(from, to),
                    "mismatch for {from:?} -> {to:?}"
                );
            }
        }
    }

    proptest! {
        #[test]
        fn prop_terminal_states_have_no_outgoing_transitions(
            from_idx in 4usize..7,
            to_idx in 0usize..7
        ) {
            let from = ALL_STATUSES[from_idx];
            let to = ALL_STATUSES[to_idx];
            prop_assert!(from.is_terminal());
            prop_assert!(!can_transition(from, to));
        }

        #[test]
        fn prop_can_transition_equals_spec(from_idx in 0usize..7, to_idx in 0usize..7) {
            let from = ALL_STATUSES[from_idx];
            let to = ALL_STATUSES[to_idx];
            prop_assert_eq!(can_transition(from, to), is_in_spec_table(from, to));
        }
    }
}
