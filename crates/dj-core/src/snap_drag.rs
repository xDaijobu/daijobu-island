//! Snap preview drag gestures and their asynchronous request lifecycle. Coordinates
//! and thresholds are physical screen pixels; the caller supplies native input and
//! snapshots the image payloads for the returned, ordered card IDs.
//!
//! This model owns only its drag busy flags. Other card actions must not start on
//! those cards until the request finishes or is cancelled. Selection, pinning and
//! the other countdown protections remain governed by [`Stack`]. In particular,
//! preserving a selection keeps the stack paused even after drag busy flags clear.

use crate::snap_stack::{ItemId, Stack};
use std::time::Instant;

/// An asynchronous drag generation, never reused by a `DragState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RequestId(pub u64);

/// The exact group to prepare, in displayed order (newest first).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: RequestId,
    pub items: Vec<ItemId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    DroppedCopy,
    /// Includes a target rejecting the drop.
    Cancelled,
    Failed,
}

#[derive(Clone, Copy, Debug)]
struct Press {
    origin: Option<ItemId>,
    point: (i32, i32),
}

#[derive(Debug)]
struct Active {
    request: Request,
    modal: bool,
}

/// At most one press or request is active. Keep this instance across gestures so
/// late worker replies can never match a newer request's ID.
#[derive(Debug, Default)]
pub struct DragState {
    pressed: Option<Press>,
    active: Option<Active>,
    last_request: u64,
    /// Survives finish/cancel until release or a genuinely new press.
    suppress_click: bool,
}

impl DragState {
    /// Arm a body gesture, or record a non-draggable action-row press (`None`).
    /// Selection changes only once the threshold is crossed. Extra presses during
    /// preparation or a modal drag cannot replace that request or leak its busy flags.
    pub fn press(&mut self, origin: Option<ItemId>, point: (i32, i32)) {
        if self.active.is_some() {
            return;
        }
        self.pressed = Some(Press { origin, point });
        self.suppress_click = false;
    }

    /// Issue one request when either axis moves beyond its threshold. Negative
    /// thresholds are treated as zero; a stationary pointer never starts a drag.
    /// Screen-coordinate subtraction is widened before taking absolute values.
    ///
    /// A selected origin uses the whole current selection. An unselected origin
    /// selects itself alone. A stale origin or any busy group member rejects the
    /// entire gesture without changing selection or another operation's busy flags.
    pub fn motion(
        &mut self,
        stack: &mut Stack,
        point: (i32, i32),
        threshold: (i32, i32),
        now: Instant,
    ) -> Option<Request> {
        if self
            .request()
            .is_some_and(|request| request.items.iter().any(|&id| stack.get(id).is_none()))
        {
            self.cancel(stack, now);
        }
        let press = self.pressed?;
        let origin = press.origin?;
        let dx = (i64::from(point.0) - i64::from(press.point.0)).abs();
        let dy = (i64::from(point.1) - i64::from(press.point.1)).abs();
        if dx <= i64::from(threshold.0.max(0)) && dy <= i64::from(threshold.1.max(0)) {
            return None;
        }

        // Crossing the threshold consumes the gesture even when it is ineligible:
        // another motion cannot silently retry it, and release cannot toggle a card.
        self.pressed = None;
        self.suppress_click = true;
        let selected_origin = stack.get(origin)?.selected;
        let items = if selected_origin {
            stack.selected()
        } else {
            vec![origin]
        };
        if items
            .iter()
            .any(|&id| stack.get(id).is_none_or(|item| item.busy))
        {
            return None;
        }
        // Refuse further requests at exhaustion instead of aliasing an old reply.
        let next = self.last_request.checked_add(1)?;
        if !selected_origin {
            stack.clear_selection(now);
            stack.toggle_select(origin, now);
        }
        for &id in &items {
            stack.set_busy(id, true, now);
        }
        let request = Request {
            id: RequestId(next),
            items,
        };
        self.last_request = next;
        self.active = Some(Active {
            request: request.clone(),
            modal: false,
        });
        Some(request)
    }

    /// Return true when the caller must suppress a click. A short press returns
    /// false. Releasing during preparation cancels it; releasing in the modal phase
    /// leaves the native loop to report its outcome through [`Self::finish`].
    pub fn release(&mut self, stack: &mut Stack, now: Instant) -> bool {
        self.pressed = None;
        let suppress = std::mem::take(&mut self.suppress_click);
        if self.is_dragging() {
            return true;
        }
        let preparing = self.active.is_some();
        self.clear_busy(stack, now);
        suppress || preparing
    }

    /// Enter the modal phase exactly once for the matching preparation. Before
    /// calling, the controller must revalidate every ID/payload and the held button,
    /// capture and visibility; this platform-free method cannot observe them.
    pub fn ready(&mut self, id: RequestId) -> bool {
        let Some(active) = self.active.as_mut() else {
            return false;
        };
        if active.request.id != id || active.modal {
            return false;
        }
        active.modal = true;
        true
    }

    /// Invalidate a press, preparation or modal request. The caller separately
    /// cancels its file worker/native loop; a late ready/finish becomes a no-op.
    /// Selection and countdown remainders survive, and the old release is suppressed.
    pub fn cancel(&mut self, stack: &mut Stack, now: Instant) {
        self.suppress_click |= self.pressed.is_some() || self.active.is_some();
        self.pressed = None;
        self.clear_busy(stack, now);
    }

    /// Clear the matching request's busy flags and return dismissed IDs in request
    /// order. Only a modal `DroppedCopy` with every requested ID still present may
    /// dismiss cards; pinned cards retain their selection and saved-file metadata.
    /// Failure, cancellation, a stale group or premature success dismisses nothing.
    pub fn finish(
        &mut self,
        id: RequestId,
        outcome: Outcome,
        stack: &mut Stack,
        now: Instant,
    ) -> Vec<ItemId> {
        if self.request().is_none_or(|request| request.id != id) {
            return Vec::new();
        }
        let active = self.active.take().expect("matching request exists");
        let accepted = active.modal
            && outcome == Outcome::DroppedCopy
            && active
                .request
                .items
                .iter()
                .all(|&id| stack.get(id).is_some());
        let mut removed = Vec::new();
        for id in active.request.items {
            stack.set_busy(id, false, now);
            if accepted && stack.get(id).is_some_and(|item| !item.pinned) {
                stack.remove(id, now);
                removed.push(id);
            }
        }
        removed
    }

    /// The exact snapshot during either preparation or the modal drag.
    pub fn request(&self) -> Option<&Request> {
        self.active.as_ref().map(|active| &active.request)
    }

    /// True only after `ready`, while the native modal drag is outstanding.
    pub fn is_dragging(&self) -> bool {
        self.active.as_ref().is_some_and(|active| active.modal)
    }

    fn clear_busy(&mut self, stack: &mut Stack, now: Instant) {
        if let Some(active) = self.active.take() {
            for id in active.request.items {
                stack.set_busy(id, false, now);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snap_stack::{Admit, CardStatus, Focus};
    use std::path::PathBuf;
    use std::time::Duration;

    const LIFE: Duration = Duration::from_secs(10);
    const POINT: (i32, i32) = (-1920, -1080);
    const THRESHOLD: (i32, i32) = (4, 6);
    const A: ItemId = ItemId(1);
    const B: ItemId = ItemId(2);
    const C: ItemId = ItemId(3);

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn add(stack: &mut Stack, now: Instant) -> ItemId {
        match stack.admit(16, 8, 512, now) {
            Admit::Added { id, evicted } => {
                assert!(evicted.is_empty());
                id
            }
            Admit::Full => panic!("test stack is full"),
        }
    }

    fn filled(n: usize, now: Instant) -> Stack {
        let mut stack = Stack::new(LIFE);
        for _ in 0..n {
            add(&mut stack, now);
        }
        stack
    }

    fn begin(drag: &mut DragState, stack: &mut Stack, origin: ItemId, now: Instant) -> Request {
        drag.press(Some(origin), POINT);
        drag.motion(stack, (POINT.0 + 5, POINT.1), THRESHOLD, now)
            .expect("eligible drag")
    }

    fn assert_busy(stack: &Stack, ids: &[ItemId]) {
        let busy: Vec<_> = stack
            .items()
            .iter()
            .filter(|item| item.busy)
            .map(|item| item.id)
            .collect();
        assert_eq!(busy, ids);
    }

    #[test]
    fn idle_events_have_no_effect() {
        let now = Instant::now();
        let mut stack = filled(2, now);
        let before = stack.items().to_vec();
        let mut drag = DragState::default();
        assert!(drag.request().is_none());
        assert!(!drag.is_dragging());
        assert!(drag.motion(&mut stack, POINT, THRESHOLD, now).is_none());
        assert!(!drag.ready(RequestId(1)));
        assert!(drag
            .finish(RequestId(1), Outcome::DroppedCopy, &mut stack, now)
            .is_empty());
        drag.cancel(&mut stack, now);
        assert!(!drag.release(&mut stack, now));
        assert_eq!(stack.items(), before);
    }

    #[test]
    fn short_press_and_exact_threshold_remain_a_click() {
        let now = Instant::now();
        let mut stack = filled(2, now);
        stack.toggle_select(B, now);
        let before = stack.items().to_vec();
        let mut drag = DragState::default();
        drag.press(Some(A), POINT);
        for (x, y) in [(0, 0), (3, 5), (-4, -6), (4, 6)] {
            assert!(drag
                .motion(&mut stack, (POINT.0 + x, POINT.1 + y), THRESHOLD, now)
                .is_none());
        }
        assert!(drag.request().is_none());
        assert_eq!(stack.items(), before);
        assert!(!drag.release(&mut stack, now));
        // The caller can now apply its normal body click exactly once.
        stack.toggle_select(A, now);
        assert_eq!(stack.selected(), [B, A]);
        assert!(drag
            .motion(&mut stack, (POINT.0 + 50, POINT.1), THRESHOLD, now)
            .is_none());
    }

    #[test]
    fn either_axis_crosses_the_threshold_in_both_directions() {
        let now = Instant::now();
        for (x, y) in [(5, 0), (-5, 0), (0, 7), (0, -7)] {
            let mut stack = filled(1, now);
            let mut drag = DragState::default();
            drag.press(Some(A), POINT);
            let request = drag
                .motion(&mut stack, (POINT.0 + x, POINT.1 + y), THRESHOLD, now)
                .unwrap();
            assert_eq!(request.items, [A]);
            assert_eq!(drag.request(), Some(&request));
            assert!(!drag.is_dragging(), "preparation is not a modal drag");
            assert_busy(&stack, &[A]);
            assert!(drag.motion(&mut stack, POINT, THRESHOLD, now).is_none());
            assert_eq!(drag.request(), Some(&request), "motion emits only once");
        }
    }

    #[test]
    fn physical_coordinates_do_not_overflow_and_thresholds_require_movement() {
        let now = Instant::now();
        for (start, end) in [
            ((i32::MIN, 0), (i32::MAX, 0)),
            ((i32::MAX, 0), (i32::MIN, 0)),
            ((0, i32::MIN), (0, i32::MAX)),
            ((0, i32::MAX), (0, i32::MIN)),
        ] {
            let mut stack = filled(1, now);
            let mut drag = DragState::default();
            drag.press(Some(A), start);
            assert!(drag
                .motion(&mut stack, end, (i32::MAX, i32::MAX), now)
                .is_some());
        }
        for threshold in [(0, 0), (-1, -1), (i32::MIN, i32::MIN)] {
            let mut stack = filled(1, now);
            let mut drag = DragState::default();
            drag.press(Some(A), POINT);
            assert!(drag.motion(&mut stack, POINT, threshold, now).is_none());
            assert!(drag
                .motion(&mut stack, (POINT.0 + 1, POINT.1), threshold, now)
                .is_some());
        }
    }

    #[test]
    fn action_row_presses_never_prepare_a_drag() {
        let now = Instant::now();
        let mut stack = filled(2, now);
        stack.select_all(now);
        let before = stack.items().to_vec();
        let mut drag = DragState::default();
        drag.press(None, POINT);
        assert!(drag
            .motion(&mut stack, (i32::MAX, i32::MIN), THRESHOLD, now)
            .is_none());
        assert!(!drag.release(&mut stack, now));
        assert!(drag.request().is_none());
        assert_eq!(stack.items(), before);
    }

    #[test]
    fn selected_origin_snapshots_the_whole_group_in_display_order() {
        let now = Instant::now();
        let mut stack = filled(3, now);
        stack.toggle_select(A, now);
        stack.toggle_select(C, now);
        stack.toggle_pin(A, now);
        let mut drag = DragState::default();
        let request = begin(&mut drag, &mut stack, A, now);
        assert_eq!(request.items, [C, A], "origin does not change order");
        assert_eq!(stack.selected(), [C, A]);
        assert_busy(&stack, &[C, A]);
        assert!(stack.get(A).unwrap().pinned);

        stack.clear_selection(now);
        stack.toggle_select(B, now);
        let newest = add(&mut stack, now);
        stack.toggle_select(newest, now);
        assert!(drag.motion(&mut stack, POINT, THRESHOLD, now).is_none());
        assert_eq!(drag.request(), Some(&request), "selection is a snapshot");
        // The returned owned request cannot mutate the internal snapshot either.
        let mut changed = request.clone();
        changed.items.clear();
        assert_eq!(drag.request(), Some(&request));
    }

    #[test]
    fn an_unselected_origin_selects_itself_alone_after_crossing() {
        let now = Instant::now();
        let mut stack = filled(3, now);
        stack.toggle_select(A, now);
        stack.toggle_select(C, now);
        stack.toggle_pin(A, now);
        stack.set_busy(C, true, now); // An unrelated save keeps its busy flag.
        let mut drag = DragState::default();
        drag.press(Some(B), POINT);
        assert_eq!(stack.selected(), [C, A]);
        let request = drag
            .motion(&mut stack, (POINT.0 + 5, POINT.1), THRESHOLD, now)
            .unwrap();
        assert_eq!(request.items, [B]);
        assert_eq!(stack.selected(), [B]);
        assert_busy(&stack, &[C, B]);
        drag.cancel(&mut stack, now);
        assert_busy(&stack, &[C]);
        assert_eq!(stack.selected(), [B]);
        assert!(stack.get(A).unwrap().pinned);
    }

    #[test]
    fn busy_origin_or_selected_member_rejects_the_whole_gesture() {
        let now = Instant::now();
        for selected_group in [false, true] {
            let mut stack = filled(3, now);
            stack.toggle_select(C, now);
            if selected_group {
                stack.toggle_select(A, now);
            }
            let busy = if selected_group { C } else { A };
            stack.set_busy(busy, true, now);
            let before = stack.items().to_vec();
            let mut drag = DragState::default();
            drag.press(Some(A), POINT);
            assert!(drag
                .motion(&mut stack, (POINT.0 + 5, POINT.1), THRESHOLD, now)
                .is_none());
            assert!(drag.request().is_none());
            assert_eq!(stack.items(), before);
            stack.set_busy(busy, false, now);
            assert!(
                drag.motion(&mut stack, (POINT.0 + 50, POINT.1), THRESHOLD, now)
                    .is_none(),
                "a refused gesture cannot retry without a new press"
            );
            assert!(drag.release(&mut stack, now), "no trailing selection click");
            assert!(begin(&mut drag, &mut stack, A, now).items.contains(&A));
        }
    }

    #[test]
    fn a_stale_pressed_origin_never_substitutes_its_neighbour() {
        let now = Instant::now();
        let mut stack = filled(2, now);
        let mut drag = DragState::default();
        drag.press(Some(A), POINT);
        stack.remove(A, now);
        let replacement = add(&mut stack, now);
        assert_ne!(replacement, A);
        assert!(drag
            .motion(&mut stack, (POINT.0 + 5, POINT.1), THRESHOLD, now)
            .is_none());
        assert!(drag.request().is_none());
        assert!(stack.selected().is_empty());
        assert_busy(&stack, &[]);
        assert!(drag.release(&mut stack, now));
    }

    #[test]
    fn release_before_readiness_invalidates_late_ready_and_success() {
        let now = Instant::now();
        let mut stack = filled(2, now);
        let mut drag = DragState::default();
        let request = begin(&mut drag, &mut stack, A, now + secs(3));
        assert_eq!(stack.remaining(A, now + secs(100)), Some(secs(7)));
        assert!(drag.release(&mut stack, now + secs(100)));
        assert!(drag.request().is_none());
        assert!(!drag.ready(request.id));
        assert!(drag
            .finish(
                request.id,
                Outcome::DroppedCopy,
                &mut stack,
                now + secs(101)
            )
            .is_empty());
        assert_eq!(stack.len(), 2);
        assert_eq!(stack.selected(), [A]);
        assert_busy(&stack, &[]);
        stack.clear_selection(now + secs(101));
        assert_eq!(stack.remaining(A, now + secs(102)), Some(secs(6)));
        assert_eq!(stack.remaining(B, now + secs(102)), Some(secs(6)));
    }

    #[test]
    fn readiness_is_once_only_and_modal_release_waits_for_the_result() {
        let now = Instant::now();
        let mut stack = filled(1, now);
        let mut drag = DragState::default();
        let request = begin(&mut drag, &mut stack, A, now);
        assert!(!drag.ready(RequestId(request.id.0 + 1)));
        assert!(!drag.is_dragging());
        assert!(drag.ready(request.id));
        assert!(!drag.ready(request.id));
        assert!(drag.is_dragging());
        assert!(drag.release(&mut stack, now));
        assert_eq!(drag.request(), Some(&request));
        assert_busy(&stack, &[A]);
        assert_eq!(
            drag.finish(request.id, Outcome::DroppedCopy, &mut stack, now),
            [A]
        );
        assert!(stack.is_empty());
        assert!(drag.request().is_none());
        assert!(!drag.is_dragging());
    }

    #[test]
    fn a_success_report_before_modal_readiness_cannot_dismiss_cards() {
        let now = Instant::now();
        let mut stack = filled(1, now);
        let mut drag = DragState::default();
        let request = begin(&mut drag, &mut stack, A, now);
        assert!(drag
            .finish(request.id, Outcome::DroppedCopy, &mut stack, now)
            .is_empty());
        assert!(drag.request().is_none());
        assert_eq!(stack.selected(), [A]);
        assert_busy(&stack, &[]);
        assert!(!drag.ready(request.id));
        assert!(drag.release(&mut stack, now));
    }

    #[test]
    fn cancellation_is_idempotent_in_every_phase_and_preserves_other_busy_work() {
        let now = Instant::now();
        // Idle, action-row press, body press, preparation, modal drag.
        for phase in 0..5 {
            let mut stack = filled(3, now);
            stack.set_busy(C, true, now);
            let mut drag = DragState::default();
            let request = match phase {
                1 => {
                    drag.press(None, POINT);
                    None
                }
                2 => {
                    drag.press(Some(A), POINT);
                    None
                }
                3 | 4 => {
                    let request = begin(&mut drag, &mut stack, A, now);
                    if phase == 4 {
                        assert!(drag.ready(request.id));
                    }
                    Some(request)
                }
                _ => None,
            };
            let selected = stack.selected();
            drag.cancel(&mut stack, now);
            drag.cancel(&mut stack, now);
            assert!(drag.request().is_none());
            assert!(!drag.is_dragging());
            assert_eq!(stack.len(), 3);
            assert_eq!(stack.selected(), selected);
            assert_busy(&stack, &[C]);
            if let Some(request) = request {
                assert!(!drag.ready(request.id));
                assert!(drag
                    .finish(request.id, Outcome::DroppedCopy, &mut stack, now)
                    .is_empty());
            }
            assert_eq!(drag.release(&mut stack, now), phase != 0);
            drag.press(Some(B), POINT);
            assert!(
                !drag.release(&mut stack, now),
                "a new short press is a click"
            );
        }
    }

    #[test]
    fn cancellation_and_failure_preserve_cards_selection_and_saved_metadata() {
        let now = Instant::now();
        for modal in [false, true] {
            for outcome in [Outcome::Cancelled, Outcome::Failed] {
                let mut stack = filled(3, now);
                stack.toggle_select(A, now);
                stack.toggle_select(B, now);
                stack.toggle_pin(B, now);
                stack.set_status(A, CardStatus::Saved);
                stack.set_saved(A, Some(PathBuf::from("saved-shot.png")));
                let before = stack.items().to_vec();
                let mut drag = DragState::default();
                let request = begin(&mut drag, &mut stack, A, now);
                if modal {
                    assert!(drag.ready(request.id));
                }
                assert!(drag
                    .finish(request.id, outcome, &mut stack, now + secs(100))
                    .is_empty());
                assert_eq!(stack.items(), before, "{modal:?}, {outcome:?}");
                assert!(drag.request().is_none());
                assert!(!drag.is_dragging());
                assert!(drag.release(&mut stack, now + secs(100)));
            }
        }
    }

    #[test]
    fn every_cancel_path_resumes_each_cards_exact_countdown_remainder() {
        let now = Instant::now();
        // Cancel, a cancelled result, a failed result, or release before readiness.
        for (modal, exit) in [
            (false, 0),
            (true, 0),
            (false, 1),
            (true, 1),
            (false, 2),
            (true, 2),
            (false, 3),
        ] {
            let mut stack = Stack::new(LIFE);
            assert_eq!(add(&mut stack, now), A);
            assert_eq!(add(&mut stack, now + secs(1)), B);
            assert_eq!(add(&mut stack, now + secs(2)), C);
            stack.select_all(now + secs(3));
            let mut drag = DragState::default();
            let request = begin(&mut drag, &mut stack, B, now + secs(10));
            // Busy must protect the snapshot even if selection is cleared later.
            stack.clear_selection(now + secs(20));
            if modal {
                assert!(drag.ready(request.id));
            }
            assert!(!stack.any_running());
            assert!(stack.tick(now + secs(80)).is_empty());
            assert_eq!(stack.remaining(A, now + secs(80)), Some(secs(7)));
            assert_eq!(stack.remaining(B, now + secs(80)), Some(secs(8)));
            assert_eq!(stack.remaining(C, now + secs(80)), Some(secs(9)));

            let done = now + secs(100);
            match exit {
                0 => drag.cancel(&mut stack, done),
                1 | 2 => {
                    let outcome = if exit == 1 {
                        Outcome::Cancelled
                    } else {
                        Outcome::Failed
                    };
                    assert!(drag
                        .finish(request.id, outcome, &mut stack, done)
                        .is_empty());
                }
                3 => assert!(drag.release(&mut stack, done)),
                _ => unreachable!(),
            }
            assert_busy(&stack, &[]);
            assert!(stack.items().iter().all(|item| stack.is_running(item.id)));
            assert_eq!(stack.remaining(A, done + secs(1)), Some(secs(6)));
            assert_eq!(stack.remaining(B, done + secs(1)), Some(secs(7)));
            assert_eq!(stack.remaining(C, done + secs(1)), Some(secs(8)));
            assert!(stack.tick(done + secs(6)).is_empty());
            assert_eq!(stack.tick(done + secs(7)), [A]);
            assert_eq!(stack.tick(done + secs(8)), [B]);
            assert_eq!(stack.tick(done + secs(9)), [C]);
        }
    }

    #[test]
    fn cancelling_never_clears_pin_hover_focus_or_hidden_countdown_protection() {
        let now = Instant::now();
        for protection in 0..4 {
            let mut stack = filled(1, now);
            let mut drag = DragState::default();
            let request = begin(&mut drag, &mut stack, A, now + secs(3));
            match protection {
                0 => stack.toggle_pin(A, now + secs(10)),
                1 => stack.set_hover(Some(A), now + secs(10)),
                2 => stack.set_focus(Some(Focus::Card(A)), now + secs(10)),
                3 => stack.set_hidden(true, now + secs(10)),
                _ => unreachable!(),
            }
            stack.clear_selection(now + secs(20));
            assert!(drag.ready(request.id));
            drag.cancel(&mut stack, now + secs(100));
            assert_busy(&stack, &[]);
            assert!(!stack.is_running(A));
            assert!(stack.tick(now + secs(200)).is_empty());
            assert_eq!(stack.remaining(A, now + secs(200)), Some(secs(7)));
            match protection {
                0 => stack.toggle_pin(A, now + secs(200)),
                1 => stack.set_hover(None, now + secs(200)),
                2 => stack.set_focus(None, now + secs(200)),
                3 => stack.set_hidden(false, now + secs(200)),
                _ => unreachable!(),
            }
            assert!(stack.is_running(A));
            assert_eq!(stack.remaining(A, now + secs(201)), Some(secs(6)));
        }
    }

    #[test]
    fn confirmed_copy_removes_only_requested_unpinned_cards_in_snapshot_order() {
        let now = Instant::now();
        let mut stack = filled(4, now);
        for id in [A, B, C] {
            stack.toggle_select(id, now + secs(3));
        }
        stack.toggle_pin(B, now + secs(3));
        stack.set_status(B, CardStatus::Saved);
        let saved = PathBuf::from("kept-shot.png");
        stack.set_saved(B, Some(saved.clone()));
        let mut drag = DragState::default();
        let request = begin(&mut drag, &mut stack, A, now + secs(10));
        assert!(drag.ready(request.id));
        assert_eq!(
            drag.finish(
                request.id,
                Outcome::DroppedCopy,
                &mut stack,
                now + secs(100)
            ),
            [C, A]
        );
        assert_eq!(
            stack.items().iter().map(|item| item.id).collect::<Vec<_>>(),
            [ItemId(4), B]
        );
        assert_eq!(stack.selected(), [B]);
        let kept = stack.get(B).unwrap();
        assert!(kept.pinned);
        assert_eq!(kept.status, CardStatus::Saved);
        assert_eq!(kept.saved.as_ref(), Some(&saved));
        assert_busy(&stack, &[]);
        assert!(drag.release(&mut stack, now + secs(100)));
        stack.clear_selection(now + secs(100));
        assert_eq!(stack.remaining(ItemId(4), now + secs(101)), Some(secs(6)));
        assert_eq!(stack.remaining(B, now + secs(101)), Some(secs(7)));
    }

    #[test]
    fn a_kept_card_retains_its_current_selection_after_success() {
        let now = Instant::now();
        let mut stack = filled(2, now);
        stack.select_all(now);
        stack.toggle_pin(A, now);
        stack.toggle_pin(B, now);
        let mut drag = DragState::default();
        let request = begin(&mut drag, &mut stack, A, now);
        assert!(drag.ready(request.id));
        stack.toggle_select(A, now);
        assert!(drag
            .finish(request.id, Outcome::DroppedCopy, &mut stack, now)
            .is_empty());
        assert_eq!(stack.selected(), [B]);
        assert_eq!(stack.len(), 2);
        assert_busy(&stack, &[]);
    }

    #[test]
    fn a_stale_group_cannot_dismiss_a_successful_subset_or_a_replacement() {
        let now = Instant::now();
        let mut stack = filled(3, now);
        stack.select_all(now);
        let mut drag = DragState::default();
        let request = begin(&mut drag, &mut stack, A, now);
        assert!(drag.ready(request.id));
        stack.remove(B, now);
        let replacement = add(&mut stack, now);
        stack.toggle_select(replacement, now);
        assert!(drag
            .finish(request.id, Outcome::DroppedCopy, &mut stack, now)
            .is_empty());
        assert_eq!(stack.selected(), [replacement, C, A]);
        assert_busy(&stack, &[]);
        assert!(drag.request().is_none());
        assert!(drag.release(&mut stack, now));
    }

    #[test]
    fn motion_cancels_a_stale_active_snapshot_in_both_phases() {
        let now = Instant::now();
        for modal in [false, true] {
            let mut stack = filled(2, now);
            stack.select_all(now);
            let mut drag = DragState::default();
            let request = begin(&mut drag, &mut stack, A, now);
            if modal {
                assert!(drag.ready(request.id));
            }
            stack.remove(B, now);
            assert!(drag.motion(&mut stack, POINT, THRESHOLD, now).is_none());
            assert!(drag.request().is_none());
            assert!(!drag.is_dragging());
            assert_busy(&stack, &[]);
            assert_eq!(stack.selected(), [A]);
            assert!(!drag.ready(request.id));
            assert!(drag.release(&mut stack, now));
        }
    }

    #[test]
    fn late_and_duplicate_replies_cannot_change_a_new_generation() {
        let now = Instant::now();
        let mut stack = filled(1, now);
        let mut drag = DragState::default();
        let old = begin(&mut drag, &mut stack, A, now);
        drag.cancel(&mut stack, now);
        let current = begin(&mut drag, &mut stack, A, now);
        assert_ne!(old.id, current.id);
        assert!(!drag.ready(old.id));
        assert!(drag
            .finish(old.id, Outcome::Failed, &mut stack, now)
            .is_empty());
        assert_eq!(drag.request(), Some(&current));
        assert_busy(&stack, &[A]);
        assert!(drag.ready(current.id));
        assert!(drag
            .finish(old.id, Outcome::DroppedCopy, &mut stack, now)
            .is_empty());
        assert!(drag.is_dragging());
        assert_busy(&stack, &[A]);
        stack.toggle_pin(A, now);
        assert!(drag
            .finish(current.id, Outcome::DroppedCopy, &mut stack, now)
            .is_empty());
        stack.toggle_pin(A, now);
        assert!(
            drag.finish(current.id, Outcome::DroppedCopy, &mut stack, now)
                .is_empty(),
            "a duplicate result cannot remove a formerly pinned card"
        );
        assert!(!drag.ready(current.id));
        assert_eq!(stack.selected(), [A]);
        assert_busy(&stack, &[]);
    }

    #[test]
    fn extra_presses_cannot_replace_an_active_request_or_leak_busy_flags() {
        let now = Instant::now();
        for modal in [false, true] {
            let mut stack = filled(2, now);
            let mut drag = DragState::default();
            let request = begin(&mut drag, &mut stack, A, now);
            if modal {
                assert!(drag.ready(request.id));
            }
            for origin in [Some(B), None] {
                drag.press(origin, (0, 0));
                assert!(drag.motion(&mut stack, POINT, THRESHOLD, now).is_none());
                assert_eq!(drag.request(), Some(&request));
                assert_eq!(stack.selected(), [A]);
                assert_busy(&stack, &[A]);
            }
            drag.cancel(&mut stack, now);
            // A new press also resets suppression if the old release was lost.
            drag.press(Some(B), POINT);
            assert!(!drag.release(&mut stack, now));
            assert_busy(&stack, &[]);
        }
    }

    #[test]
    fn request_id_exhaustion_never_reuses_an_old_generation() {
        let now = Instant::now();
        let mut stack = filled(2, now);
        let mut drag = DragState {
            last_request: u64::MAX - 1,
            ..DragState::default()
        };
        let last = begin(&mut drag, &mut stack, A, now);
        assert_eq!(last.id, RequestId(u64::MAX));
        drag.cancel(&mut stack, now);
        stack.clear_selection(now);
        let before = stack.items().to_vec();
        drag.press(Some(B), POINT);
        assert!(drag
            .motion(&mut stack, (POINT.0 + 5, POINT.1), THRESHOLD, now)
            .is_none());
        assert!(drag.request().is_none());
        assert!(!drag.ready(last.id));
        assert_eq!(stack.items(), before);
        assert!(drag.release(&mut stack, now));
    }
}
