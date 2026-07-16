use thiserror::Error;

use crate::{EventId, GraphSnapshot, Placement, Point, SnapshotError};

#[derive(Clone, Debug, PartialEq)]
pub struct MoveNode {
    pub event_id: EventId,
    pub destination: Point,
    pub expected_placement_version: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RestorePlacement {
    pub event_id: EventId,
    pub previous: Option<Placement>,
    pub expected_placement_version: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppliedMove {
    pub placement: Placement,
    pub inverse: RestorePlacement,
    pub revision: u64,
}

impl GraphSnapshot {
    /// Applies a placement-only move and returns its guarded inverse.
    ///
    /// # Errors
    ///
    /// Rejects missing events, non-finite coordinates, stale placement
    /// generations, and counter overflow without changing the snapshot.
    pub fn apply_move(&mut self, command: &MoveNode) -> Result<AppliedMove, MoveNodeError> {
        if !self.events.contains_key(&command.event_id) {
            return Err(MoveNodeError::MissingEvent(command.event_id.clone()));
        }
        command.destination.validate()?;

        let previous = self.placements.get(&command.event_id).copied();
        let actual_version = self
            .placement_versions
            .get(&command.event_id)
            .copied()
            .unwrap_or_default();
        if actual_version != command.expected_placement_version {
            return Err(MoveNodeError::VersionConflict {
                event: command.event_id.clone(),
                expected: command.expected_placement_version,
                actual: actual_version,
            });
        }

        let version = actual_version
            .checked_add(1)
            .ok_or(MoveNodeError::VersionOverflow)?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(MoveNodeError::RevisionOverflow)?;
        let placement = Placement {
            point: command.destination,
            pinned: true,
        };
        self.placements.insert(command.event_id.clone(), placement);
        self.placement_versions
            .insert(command.event_id.clone(), version);
        self.revision = revision;

        Ok(AppliedMove {
            placement,
            inverse: RestorePlacement {
                event_id: command.event_id.clone(),
                previous,
                expected_placement_version: version,
            },
            revision,
        })
    }

    /// Applies a move inverse only to the generation it was created for.
    ///
    /// # Errors
    ///
    /// Rejects missing events, stale generations, and counter overflow without
    /// changing the snapshot.
    pub fn restore_placement(&mut self, inverse: &RestorePlacement) -> Result<u64, MoveNodeError> {
        if !self.events.contains_key(&inverse.event_id) {
            return Err(MoveNodeError::MissingEvent(inverse.event_id.clone()));
        }
        let actual_version = self
            .placement_versions
            .get(&inverse.event_id)
            .copied()
            .unwrap_or_default();
        if actual_version != inverse.expected_placement_version {
            return Err(MoveNodeError::VersionConflict {
                event: inverse.event_id.clone(),
                expected: inverse.expected_placement_version,
                actual: actual_version,
            });
        }
        if let Some(previous) = inverse.previous {
            previous.point.validate()?;
        }

        let version = actual_version
            .checked_add(1)
            .ok_or(MoveNodeError::VersionOverflow)?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(MoveNodeError::RevisionOverflow)?;

        match inverse.previous {
            Some(previous) => {
                self.placements.insert(inverse.event_id.clone(), previous);
            }
            None => {
                self.placements.shift_remove(&inverse.event_id);
            }
        }
        self.placement_versions
            .insert(inverse.event_id.clone(), version);
        self.revision = revision;
        Ok(revision)
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum MoveNodeError {
    #[error("cannot move missing event {0:?}")]
    MissingEvent(EventId),
    #[error("placement for {event:?} changed: expected version {expected}, found {actual}")]
    VersionConflict {
        event: EventId,
        expected: u64,
        actual: u64,
    },
    #[error("placement version overflow")]
    VersionOverflow,
    #[error("graph revision overflow")]
    RevisionOverflow,
    #[error(transparent)]
    InvalidPoint(#[from] SnapshotError),
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;
    use proptest::prelude::*;

    use super::*;
    use crate::{GraphSnapshot, ResearchEvent};

    fn snapshot_with_event() -> GraphSnapshot {
        let id = EventId("event-a".to_owned());
        GraphSnapshot {
            events: IndexMap::from([(
                id.clone(),
                ResearchEvent {
                    id,
                    title: "A".to_owned(),
                    date: "2026-07-14".to_owned(),
                    color: 0xff00_0000,
                    summary: "Summary".to_owned(),
                    source_label: "Source".to_owned(),
                    artifacts: vec![],
                    url: None,
                },
            )]),
            ..GraphSnapshot::default()
        }
    }

    proptest! {
        #[test]
        fn move_then_inverse_preserves_every_non_revision_field(x in -1.0e6f64..1.0e6, y in -1.0e6f64..1.0e6) {
            let mut graph = snapshot_with_event();
            let before = graph.clone();
            let applied = graph.apply_move(&MoveNode {
                event_id: EventId("event-a".to_owned()),
                destination: Point { x, y },
                expected_placement_version: 0,
            })?;

            prop_assert_eq!(graph.events.clone(), before.events.clone());
            prop_assert_eq!(graph.bridges.clone(), before.bridges.clone());
            graph.restore_placement(&applied.inverse)?;
            prop_assert_eq!(graph.events, before.events);
            prop_assert_eq!(graph.bridges, before.bridges);
            prop_assert_eq!(graph.placements, before.placements);
        }
    }

    #[test]
    fn stale_inverse_cannot_overwrite_a_later_move() {
        let mut graph = snapshot_with_event();
        let first = graph
            .apply_move(&MoveNode {
                event_id: EventId("event-a".to_owned()),
                destination: Point { x: 1.0, y: 2.0 },
                expected_placement_version: 0,
            })
            .unwrap();
        graph
            .apply_move(&MoveNode {
                event_id: EventId("event-a".to_owned()),
                destination: Point { x: 3.0, y: 4.0 },
                expected_placement_version: 1,
            })
            .unwrap();

        assert!(matches!(
            graph.restore_placement(&first.inverse),
            Err(MoveNodeError::VersionConflict { actual: 2, .. })
        ));
        assert_eq!(
            graph.placements[&EventId("event-a".to_owned())].point,
            Point { x: 3.0, y: 4.0 }
        );
    }

    #[test]
    fn restoring_absence_does_not_resurrect_version_zero() {
        let mut graph = snapshot_with_event();
        let first = graph
            .apply_move(&MoveNode {
                event_id: EventId("event-a".to_owned()),
                destination: Point { x: 1.0, y: 2.0 },
                expected_placement_version: 0,
            })
            .unwrap();
        graph.restore_placement(&first.inverse).unwrap();

        assert!(
            !graph
                .placements
                .contains_key(&EventId("event-a".to_owned()))
        );
        assert_eq!(graph.placement_versions[&EventId("event-a".to_owned())], 2);
        assert!(matches!(
            graph.apply_move(&MoveNode {
                event_id: EventId("event-a".to_owned()),
                destination: Point { x: 3.0, y: 4.0 },
                expected_placement_version: 0,
            }),
            Err(MoveNodeError::VersionConflict { actual: 2, .. })
        ));
    }

    #[test]
    fn revision_overflow_does_not_partially_apply_move_or_restore() {
        let id = EventId("event-a".to_owned());
        let mut graph = snapshot_with_event();
        graph.revision = u64::MAX;
        let before_move = graph.clone();
        assert_eq!(
            graph.apply_move(&MoveNode {
                event_id: id.clone(),
                destination: Point { x: 1.0, y: 2.0 },
                expected_placement_version: 0,
            }),
            Err(MoveNodeError::RevisionOverflow)
        );
        assert_eq!(graph, before_move);

        graph.revision = 0;
        let applied = graph
            .apply_move(&MoveNode {
                event_id: id,
                destination: Point { x: 1.0, y: 2.0 },
                expected_placement_version: 0,
            })
            .unwrap();
        graph.revision = u64::MAX;
        let before_restore = graph.clone();
        assert_eq!(
            graph.restore_placement(&applied.inverse),
            Err(MoveNodeError::RevisionOverflow)
        );
        assert_eq!(graph, before_restore);
    }

    #[test]
    fn corrupt_inverse_cannot_inject_a_nonfinite_placement() {
        let id = EventId("event-a".to_owned());
        let mut graph = snapshot_with_event();
        let before = graph.clone();
        let inverse = RestorePlacement {
            event_id: id,
            previous: Some(Placement {
                point: Point {
                    x: f64::NAN,
                    y: 0.0,
                },
                pinned: true,
            }),
            expected_placement_version: 0,
        };

        assert_eq!(
            graph.restore_placement(&inverse),
            Err(MoveNodeError::InvalidPoint(SnapshotError::NonFinitePoint))
        );
        assert_eq!(graph, before);
    }
}
