//! Safe screen-pin geometry, retained-payload admission and visibility state.
//! Image/chrome geometry is logical; desktop placement is physical. This model
//! owns neither image buffers nor windows, so source-card expiry cannot close pins.

use crate::{layout::Rect, snap::IRect};
use std::collections::HashMap;

const MAX_PINS: usize = 8;
const MAX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_W: f32 = 664.0;
const MAX_H: f32 = 568.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PinId(pub u64);

/// Identity of the retained source image allocation, independent of stack IDs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceId(pub u64);

/// Identity of retained IO ownership (a queued job or undrained reply) that outlives
/// the pin it was started for. Separate counter from [`PinId`]; never recycled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HoldId(pub u64);

/// A live shared pixel/PNG allocation. Equal keys must always have equal lengths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Allocation {
    pub key: u64,
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Admission {
    New(PinId),
    Existing(PinId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PinAction {
    Copy,
    Save,
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Hit {
    Body,
    Action(PinAction),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub w: f32,
    pub h: f32,
    pub image: Rect,
    pub actions: [(PinAction, Rect); 3],
    pub status: Rect,
}

impl Layout {
    /// Actions take precedence over the draggable image and chrome body.
    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        if !Rect::new(0.0, 0.0, self.w, self.h).contains(x, y) {
            return None;
        }
        self.actions
            .iter()
            .find(|(_, rect)| rect.contains(x, y))
            .map(|(action, _)| Hit::Action(*action))
            .or(Some(Hit::Body))
    }
}

// Bound malformed DPI inputs and maximum GPU allocations consistently. Valid
// Windows monitor scales fall well within this range; non-finite inputs use 100%.
fn sane_scale(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale.clamp(0.5, 8.0)
    } else {
        1.0
    }
}

/// Image fit is at most 640x480 logical pixels and never enlarges source pixels
/// physically. Tiny images sit centred over full-width, legible action chrome.
/// On a work area smaller than the minimum chrome, shrink chrome to keep Close
/// inside the work area; no layout can offer full-size controls on such a screen.
pub fn layout(image: (u32, u32), scale: f32, work: IRect) -> Layout {
    let scale = f64::from(sane_scale(scale));
    let work = usable_work(work);
    let available_w = f64::from(work.w) / scale;
    let available_h = f64::from(work.h) / scale;
    let chrome = (available_w / 256.0).min(available_h / 89.0).min(1.0);
    let pad = 12.0 * chrome;
    let gap = 8.0 * chrome;
    let row = 32.0 * chrome;
    let status_gap = 4.0 * chrome;
    let status_h = 20.0 * chrome;
    let chrome_h = 2.0 * pad + gap + row + status_gap + status_h;
    let (iw, ih) = (f64::from(image.0), f64::from(image.1));
    let fit = if image.0 == 0 || image.1 == 0 {
        0.0
    } else {
        (640.0 / iw)
            .min(480.0 / ih)
            .min(1.0 / scale)
            .min((available_w - 2.0 * pad).max(0.0) / iw)
            .min((available_h - chrome_h).max(0.0) / ih)
    };
    let (image_w, image_h) = (iw * fit, ih * fit);
    let w = (image_w + 2.0 * pad).max(256.0 * chrome);
    let h = image_h + chrome_h;
    let action_y = pad + image_h + gap;
    let action_w = ((w - 2.0 * pad - 2.0 * gap) / 3.0).max(0.0);
    Layout {
        w: w as f32,
        h: h as f32,
        image: Rect::new(
            ((w - image_w) / 2.0) as f32,
            pad as f32,
            image_w as f32,
            image_h as f32,
        ),
        actions: [PinAction::Copy, PinAction::Save, PinAction::Close].map(|action| {
            let index = match action {
                PinAction::Copy => 0,
                PinAction::Save => 1,
                PinAction::Close => 2,
            };
            (
                action,
                Rect::new(
                    (pad + f64::from(index) * (action_w + gap)) as f32,
                    action_y as f32,
                    action_w as f32,
                    row as f32,
                ),
            )
        }),
        status: Rect::new(
            pad as f32,
            (action_y + row + status_gap) as f32,
            (w - 2.0 * pad).max(0.0) as f32,
            status_h as f32,
        ),
    }
}

/// Fixed maximum surface dimensions for all layouts at this DPI; dragging a pin
/// does not change this allocation. Invalid scales use the same policy as layout.
pub fn allocation_size(scale: f32) -> (u32, u32) {
    let scale = f64::from(sane_scale(scale));
    (
        (f64::from(MAX_W) * scale).ceil() as u32,
        (f64::from(MAX_H) * scale).ceil() as u32,
    )
}

/// Place a pin's top-left at the anchor, then keep the entire window reachable.
pub fn place(layout: &Layout, scale: f32, work: IRect, anchor: (i32, i32)) -> IRect {
    let scale = f64::from(sane_scale(scale));
    let extent = |value: f32| {
        if value.is_finite() && value > 0.0 {
            (f64::from(value) * scale).ceil().min(f64::from(i32::MAX)) as i32
        } else {
            0
        }
    };
    clamp(
        IRect::new(anchor.0, anchor.1, extent(layout.w), extent(layout.h)),
        work,
    )
}

// IRect's endpoint helpers add i32 values. Normalize invalid/extreme work bounds
// so even their returned right/bottom endpoints remain representable.
fn usable_work(work: IRect) -> IRect {
    let extent = |origin: i32, length: i32| {
        i64::from(length)
            .max(0)
            .min(i64::from(i32::MAX) - i64::from(origin)) as i32
    };
    IRect::new(
        work.x,
        work.y,
        extent(work.x, work.w),
        extent(work.y, work.h),
    )
}

/// Clamp across monitor removal, negative origins and oversized windows. If a
/// window exceeds the remaining work area, reduce its bounds to that area; the
/// controller recomputes layout for the current monitor so Close stays inside.
pub fn clamp(rect: IRect, work: IRect) -> IRect {
    let work = usable_work(work);
    let w = rect.w.clamp(0, work.w);
    let h = rect.h.clamp(0, work.h);
    let x = i64::from(rect.x).clamp(
        i64::from(work.x),
        i64::from(work.x) + i64::from(work.w) - i64::from(w),
    );
    let y = i64::from(rect.y).clamp(
        i64::from(work.y),
        i64::from(work.y) + i64::from(work.h) - i64::from(h),
    );
    IRect::new(x as i32, y as i32, w, h)
}

#[derive(Debug)]
struct Pin {
    id: PinId,
    source: SourceId,
    allocations: Vec<Allocation>,
    desired_visible: bool,
}

/// Session-long identity and ownership ledger. Keep this model across closes and
/// capture sessions: a late reply must never match a newly admitted reference.
#[derive(Debug)]
pub struct Pins {
    pins: Vec<Pin>,
    last_id: u64,
    holds: Vec<(HoldId, Vec<Allocation>)>,
    last_hold: u64,
    bytes: u64,
    allowed: bool,
    capturing: bool,
}

impl Default for Pins {
    fn default() -> Self {
        Self {
            pins: Vec::new(),
            last_id: 0,
            holds: Vec::new(),
            last_hold: 0,
            bytes: 0,
            allowed: true,
            capturing: false,
        }
    }
}

impl Pins {
    /// Existing sources are revealed without retaining the supplied allocations.
    /// New admissions never evict another pin or consume an ID on failure.
    pub fn admit(
        &mut self,
        source: SourceId,
        allocations: Vec<Allocation>,
    ) -> Result<Admission, String> {
        if let Some(pin) = self.pins.iter_mut().find(|pin| pin.source == source) {
            pin.desired_visible = true;
            return Ok(Admission::Existing(pin.id));
        }
        if self.count() >= MAX_PINS {
            return Err("At most 8 screenshots can be pinned to screen.".into());
        }
        let bytes = self.proposed_bytes(None, &allocations)?;
        let id = PinId(
            self.last_id
                .checked_add(1)
                .ok_or("Screen pin identities are exhausted.")?,
        );
        self.pins.push(Pin {
            id,
            source,
            allocations,
            desired_visible: true,
        });
        self.last_id = id.0;
        self.bytes = bytes;
        Ok(Admission::New(id))
    }

    /// Close this exact lifetime, releasing only allocations no other pin uses.
    pub fn close(&mut self, id: PinId) -> bool {
        let Some(index) = self.pins.iter().position(|pin| pin.id == id) else {
            return false;
        };
        self.pins.remove(index);
        // A subset of a valid ledger cannot exceed its limits or conflict.
        self.bytes = self.proposed_bytes(None, &[]).expect("valid pin ledger");
        true
    }

    pub fn contains(&self, id: PinId) -> bool {
        self.pins.iter().any(|pin| pin.id == id)
    }

    /// Live IDs in stable insertion order; closing a neighbour does not reorder.
    pub fn ids(&self) -> Vec<PinId> {
        self.pins.iter().map(|pin| pin.id).collect()
    }

    pub fn count(&self) -> usize {
        self.pins.len()
    }

    /// Unique allocation bytes retained by live pins and outstanding holds.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Retain allocations for queued IO independently of any pin lifetime. A hold
    /// counts toward the byte budget (shared keys once) but never toward the pin
    /// limit, and it has no source, so it can never reveal a pin. Atomic: on
    /// inconsistent lengths or a union above the limit nothing changes.
    pub fn hold(&mut self, allocations: Vec<Allocation>) -> Result<HoldId, String> {
        let bytes = self.proposed_bytes(None, &allocations)?;
        let id = HoldId(
            self.last_hold
                .checked_add(1)
                .ok_or("Screen pin hold identities are exhausted.")?,
        );
        self.holds.push((id, allocations));
        self.last_hold = id.0;
        self.bytes = bytes;
        Ok(id)
    }

    /// Drop a hold's retention. Idempotent: false when the hold is unknown.
    pub fn release(&mut self, hold: HoldId) -> bool {
        let Some(index) = self.holds.iter().position(|(id, _)| *id == hold) else {
            return false;
        };
        self.holds.remove(index);
        // A subset of a valid ledger cannot exceed its limits or conflict.
        self.bytes = self.proposed_bytes(None, &[]).expect("valid pin ledger");
        true
    }

    /// Outstanding holds, which are not pins.
    pub fn holds(&self) -> usize {
        self.holds.len()
    }

    /// Transactionally reaccount optional caches. The caller can discard a PNG
    /// cache if this fails, retaining the previously admitted payload unchanged.
    pub fn replace_allocations(
        &mut self,
        id: PinId,
        allocations: Vec<Allocation>,
    ) -> Result<(), String> {
        let index = self
            .pins
            .iter()
            .position(|pin| pin.id == id)
            .ok_or("This screen pin is already closed.")?;
        let bytes = self.proposed_bytes(Some(id), &allocations)?;
        self.pins[index].allocations = allocations;
        self.bytes = bytes;
        Ok(())
    }

    // Reconstructing this tiny (at most eight owners) ledger avoids a separate
    // mutable refcount map and makes validation/failure completely atomic.
    fn proposed_bytes(
        &self,
        replacing: Option<PinId>,
        allocations: &[Allocation],
    ) -> Result<u64, String> {
        let held = self.holds.iter().flat_map(|(_, held)| held);
        let mut live = HashMap::new();
        for allocation in self
            .pins
            .iter()
            .flat_map(|pin| &pin.allocations)
            .chain(held.clone())
        {
            live.insert(allocation.key, allocation.bytes);
        }
        for allocation in allocations {
            if live
                .insert(allocation.key, allocation.bytes)
                .is_some_and(|bytes| bytes != allocation.bytes)
            {
                return Err("A shared screenshot allocation has inconsistent lengths.".into());
            }
        }
        let mut unique = HashMap::new();
        let retained = self
            .pins
            .iter()
            .filter(|pin| Some(pin.id) != replacing)
            .flat_map(|pin| &pin.allocations)
            .chain(held)
            .chain(allocations);
        let mut total = 0_u64;
        for allocation in retained {
            if unique.insert(allocation.key, allocation.bytes).is_none() {
                total = total
                    .checked_add(allocation.bytes)
                    .ok_or("Screen pin retained image size overflowed.")?;
            }
        }
        if total > MAX_BYTES {
            return Err("Pinned screenshots exceed the 256 MiB image limit.".into());
        }
        Ok(total)
    }

    pub fn reveal(&mut self, id: PinId) {
        self.set_visible(id, true);
    }

    /// Desired visibility survives capture suppression and the global hide key.
    pub fn set_visible(&mut self, id: PinId, visible: bool) {
        if let Some(pin) = self.pins.iter_mut().find(|pin| pin.id == id) {
            pin.desired_visible = visible;
        }
    }

    pub fn is_visible(&self, id: PinId) -> bool {
        self.allowed
            && !self.capturing
            && self
                .pins
                .iter()
                .any(|pin| pin.id == id && pin.desired_visible)
    }

    pub fn set_allowed(&mut self, allowed: bool) {
        self.allowed = allowed;
    }

    /// Idempotent suppression: no stale visibility snapshot can resurrect pins.
    pub fn begin_capture(&mut self) {
        self.capturing = true;
    }

    /// Used for success, cancellation and failure alike.
    pub fn end_capture(&mut self) {
        self.capturing = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allocation(key: u64, bytes: u64) -> Allocation {
        Allocation { key, bytes }
    }

    fn admit(pins: &mut Pins, source: u64, allocations: Vec<Allocation>) -> PinId {
        let Admission::New(id) = pins.admit(SourceId(source), allocations).unwrap() else {
            panic!("expected a new pin")
        };
        id
    }

    fn inside(rect: Rect, layout: &Layout) {
        assert!(rect.x >= 0.0 && rect.y >= 0.0, "{rect:?}");
        assert!(rect.x + rect.w <= layout.w + 0.001, "{rect:?}");
        assert!(rect.y + rect.h <= layout.h + 0.001, "{rect:?}");
    }

    #[test]
    fn synthetic_images_fit_preserve_aspect_and_never_upscale_physical_pixels() {
        for scale in [1.0, 1.5, 2.0] {
            for image in [(1, 1), (37, 19), (4000, 5), (5, 4000), (3840, 2160)] {
                for work in [
                    IRect::new(0, 40, 1920, 1000),
                    IRect::new(-1280, -900, 1280, 840),
                    IRect::new(0, 48, 450, 300),
                ] {
                    let result = layout(image, scale, work);
                    let display = result.image;
                    assert!(display.w <= 640.0 && display.h <= 480.0);
                    assert!(display.w * scale <= image.0 as f32 + 0.001);
                    assert!(display.h * scale <= image.1 as f32 + 0.001);
                    let ratio = display.w / display.h;
                    let original = image.0 as f32 / image.1 as f32;
                    assert!((ratio / original - 1.0).abs() < 0.0001);
                    inside(display, &result);
                    inside(result.status, &result);
                    let allocation = allocation_size(scale);
                    let placed = place(&result, scale, work, (i32::MAX, i32::MIN));
                    assert!(placed.w as u32 <= allocation.0);
                    assert!(placed.h as u32 <= allocation.1);
                    assert!(placed.x >= work.x && placed.y >= work.y);
                    assert!(placed.right() <= work.right());
                    assert!(placed.bottom() <= work.bottom());
                    for (action, rect) in result.actions {
                        inside(rect, &result);
                        assert_eq!(
                            result.hit(rect.x + rect.w / 2.0, rect.y + rect.h / 2.0),
                            Some(Hit::Action(action))
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn tiny_image_has_centred_image_and_full_size_buttons_at_each_dpi() {
        for scale in [1.0, 1.5, 2.0] {
            let result = layout((4, 2), scale, IRect::new(-1920, 48, 1920, 1032));
            assert_eq!(result.w, 256.0);
            assert_eq!(result.image.w * scale, 4.0);
            assert_eq!(result.image.h * scale, 2.0);
            assert_eq!(result.image.x + result.image.w / 2.0, result.w / 2.0);
            for (_, rect) in result.actions {
                assert_eq!(rect.w, 72.0);
                assert_eq!(rect.h, 32.0);
            }
            assert_eq!(result.hit(0.0, 0.0), Some(Hit::Body));
            assert_eq!(result.hit(result.w, 0.0), None);
            assert_eq!(result.hit(-1.0, 0.0), None);
            assert_eq!(result.hit(0.0, result.h), None);
            assert_eq!(result.hit(f32::NAN, 0.0), None);
        }
    }

    #[test]
    fn fixed_surface_contains_maximum_layout_at_every_supported_integer_dpi() {
        let work = IRect::new(-16000, -12000, 32000, 24000);
        for dpi in 48..=768 {
            let scale = dpi as f32 / 96.0;
            let result = layout((16384, 12288), scale, work);
            let allocation = allocation_size(scale);
            let placed = place(&result, scale, work, (0, 0));
            assert!(placed.w as u32 <= allocation.0, "DPI {dpi}");
            assert!(placed.h as u32 <= allocation.1, "DPI {dpi}");
        }
    }

    #[test]
    fn removed_monitor_and_oversized_window_clamp_into_remaining_work_area() {
        let work = IRect::new(-1000, -800, 1000, 760);
        assert_eq!(
            clamp(IRect::new(2100, 100, 664, 568), work),
            IRect::new(-664, -608, 664, 568)
        );
        assert_eq!(
            clamp(IRect::new(i32::MIN, i32::MAX, 8000, 4000), work),
            work
        );
        assert_eq!(
            place(&layout((100, 100), 1.0, work), 1.0, work, (-990, -790)),
            IRect::new(-990, -790, 256, 188)
        );
    }

    #[test]
    fn tiny_and_malformed_geometry_stays_finite_bounded_and_reachable() {
        for scale in [0.0, -1.0, f32::NAN, f32::INFINITY, 0.001, 100000.0] {
            let allocated = allocation_size(scale);
            assert!(allocated.0 > 0 && allocated.0 <= 5312);
            assert!(allocated.1 > 0 && allocated.1 <= 4544);
            for work in [
                IRect::new(0, 0, 80, 50),
                IRect::new(100, 100, 0, -3),
                IRect::new(i32::MAX - 20, i32::MAX - 30, i32::MAX, i32::MAX),
                IRect::new(i32::MIN, i32::MIN, i32::MAX, i32::MAX),
            ] {
                for image in [(0, 0), (0, 8), (u32::MAX, u32::MAX)] {
                    let result = layout(image, scale, work);
                    assert!(result.w.is_finite() && result.h.is_finite());
                    let placed = place(&result, scale, work, (i32::MAX, i32::MIN));
                    let safe = usable_work(work);
                    assert!(placed.x >= safe.x && placed.y >= safe.y);
                    assert!(placed.right() <= safe.right());
                    assert!(placed.bottom() <= safe.bottom());
                    for (_, rect) in result.actions {
                        inside(rect, &result);
                        if rect.w > 0.0 && rect.h > 0.0 {
                            assert!(result
                                .hit(rect.x + rect.w / 2.0, rect.y + rect.h / 2.0)
                                .is_some());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn duplicate_reveals_without_reallocation_even_when_full() {
        let mut pins = Pins::default();
        let first = admit(&mut pins, 10, vec![allocation(1, 123)]);
        for source in 11..18 {
            admit(&mut pins, source, vec![]);
        }
        pins.set_visible(first, false);
        assert_eq!(
            pins.admit(SourceId(10), vec![allocation(1, u64::MAX)]),
            Ok(Admission::Existing(first))
        );
        assert!(pins.is_visible(first));
        assert_eq!(pins.count(), 8);
        assert_eq!(pins.bytes(), 123);
        assert!(pins.admit(SourceId(18), vec![]).is_err());
        assert_eq!(pins.ids().first(), Some(&first));
    }

    #[test]
    fn unique_shared_allocations_count_once_and_release_at_last_owner() {
        let mut pins = Pins::default();
        let a = admit(&mut pins, 1, vec![allocation(1, 30), allocation(1, 30)]);
        let b = admit(&mut pins, 2, vec![allocation(1, 30), allocation(2, 70)]);
        assert_eq!(pins.bytes(), 100);
        pins.replace_allocations(a, vec![allocation(3, 20)])
            .unwrap();
        assert_eq!(pins.bytes(), 120);
        assert!(pins.close(b));
        assert_eq!(pins.bytes(), 20);
        assert!(pins.close(a));
        assert_eq!(pins.bytes(), 0);
        assert_eq!(pins.count(), 0);
    }

    #[test]
    fn exact_budget_shared_admission_and_rejected_optional_cache_are_atomic() {
        let mut pins = Pins::default();
        let a = admit(&mut pins, 1, vec![allocation(1, MAX_BYTES)]);
        let b = admit(&mut pins, 2, vec![allocation(1, MAX_BYTES)]);
        let before = pins.ids();
        assert!(pins.admit(SourceId(3), vec![allocation(2, 1)]).is_err());
        assert!(pins.replace_allocations(a, vec![allocation(2, 1)]).is_err());
        assert_eq!(pins.ids(), before);
        assert_eq!(pins.bytes(), MAX_BYTES);
        assert!(pins.contains(a) && pins.contains(b));
        assert!(pins.is_visible(a));
        pins.close(b);
        pins.replace_allocations(a, vec![allocation(2, 1)]).unwrap();
        assert_eq!(pins.bytes(), 1);
        assert_eq!(admit(&mut pins, 3, vec![]), PinId(3));
    }

    #[test]
    fn conflicting_keys_and_arithmetic_overflow_never_mutate_ledger() {
        let mut pins = Pins::default();
        let a = admit(&mut pins, 1, vec![allocation(1, 10)]);
        for bad in [
            vec![allocation(1, 11)],
            vec![allocation(2, 10), allocation(2, 11)],
            vec![allocation(2, u64::MAX), allocation(3, 1)],
        ] {
            assert!(pins.admit(SourceId(2), bad.clone()).is_err());
            assert!(pins.replace_allocations(a, bad).is_err());
            assert_eq!(pins.ids(), vec![a]);
            assert_eq!(pins.bytes(), 10);
        }
        // A key identifies a live allocation even if this is its only owner.
        assert!(pins
            .replace_allocations(a, vec![allocation(1, 12)])
            .is_err());
        assert_eq!(pins.bytes(), 10);
    }

    #[test]
    fn stable_nonrecycled_ids_isolate_close_and_late_replies() {
        let mut pins = Pins::default();
        let a = admit(&mut pins, 1, vec![allocation(1, 10)]);
        let b = admit(&mut pins, 2, vec![]);
        let c = admit(&mut pins, 3, vec![]);
        assert!(pins.close(b));
        let next = admit(&mut pins, 2, vec![allocation(2, 20)]);
        assert!(next.0 > c.0);
        assert_eq!(pins.ids(), vec![a, c, next]);
        assert!(!pins.close(b));
        assert!(pins
            .replace_allocations(b, vec![allocation(3, 30)])
            .is_err());
        pins.reveal(b);
        pins.set_visible(b, false);
        assert!(!pins.contains(b) && !pins.is_visible(b));
        assert!(pins.contains(next) && pins.is_visible(next));
        assert_eq!(pins.bytes(), 30);
        for id in pins.ids() {
            assert!(pins.close(id));
        }
        assert_eq!(pins.bytes(), 0);
        assert!(pins.ids().is_empty());
        assert!(admit(&mut pins, 1, vec![]).0 > next.0);
    }

    #[test]
    fn identity_exhaustion_refuses_admission_without_wrapping_or_eviction() {
        let mut pins = Pins::default();
        let a = admit(&mut pins, 1, vec![]);
        pins.last_id = u64::MAX;
        assert!(pins.admit(SourceId(2), vec![allocation(1, 20)]).is_err());
        assert_eq!(pins.ids(), vec![a]);
        assert_eq!(pins.bytes(), 0);
        assert_eq!(pins.admit(SourceId(1), vec![]), Ok(Admission::Existing(a)));
    }

    #[test]
    fn capture_end_restores_only_current_desired_live_pins() {
        // Success/cancel/error integration must all invoke this one exit API.
        let mut pins = Pins::default();
        let a = admit(&mut pins, 1, vec![]);
        let hidden = admit(&mut pins, 2, vec![]);
        let closed = admit(&mut pins, 3, vec![]);
        let later_hidden = admit(&mut pins, 4, vec![]);
        pins.set_visible(hidden, false);
        pins.begin_capture();
        pins.begin_capture(); // Repeated entry must not overwrite desires.
        assert!(pins.ids().iter().all(|id| !pins.is_visible(*id)));
        pins.close(closed);
        pins.set_visible(later_hidden, false);
        let opened = admit(&mut pins, 5, vec![]);
        assert!(!pins.is_visible(opened));
        pins.end_capture();
        pins.end_capture();
        assert!(pins.is_visible(a) && pins.is_visible(opened));
        assert!(!pins.is_visible(hidden));
        assert!(!pins.is_visible(closed));
        assert!(!pins.is_visible(later_hidden));
        assert!(!pins.contains(closed));
    }

    #[test]
    fn hold_survives_pin_close_and_release_recomputes_bytes() {
        let mut pins = Pins::default();
        let a = admit(&mut pins, 1, vec![allocation(1, 40)]);
        let hold = pins
            .hold(vec![allocation(1, 40), allocation(2, 15)])
            .unwrap();
        // The shared key counts once across the pin and the hold.
        assert_eq!(pins.bytes(), 55);
        assert_eq!(pins.holds(), 1);
        assert!(pins.close(a));
        assert_eq!(pins.bytes(), 55);
        assert_eq!(pins.count(), 0);
        assert!(pins.release(hold));
        assert_eq!(pins.bytes(), 0);
        assert_eq!(pins.holds(), 0);
        // Idempotent: a second release or a never-issued ID changes nothing.
        assert!(!pins.release(hold));
        assert!(!pins.release(HoldId(999)));
        assert_eq!(pins.bytes(), 0);
    }

    #[test]
    fn holds_budget_admission_and_replacement_across_pins_and_holds() {
        let mut pins = Pins::default();
        let a = admit(&mut pins, 1, vec![allocation(1, MAX_BYTES - 10)]);
        let hold = pins.hold(vec![allocation(2, 10)]).unwrap();
        assert_eq!(pins.bytes(), MAX_BYTES);
        assert!(pins.admit(SourceId(2), vec![allocation(3, 1)]).is_err());
        assert!(pins
            .replace_allocations(a, vec![allocation(4, MAX_BYTES - 9)])
            .is_err());
        assert!(pins.hold(vec![allocation(5, 1)]).is_err());
        assert_eq!(pins.holds(), 1);
        assert_eq!(pins.bytes(), MAX_BYTES);
        // A held key shared with a replacement or admission is not double counted.
        pins.replace_allocations(a, vec![allocation(2, 10), allocation(6, 5)])
            .unwrap();
        assert_eq!(pins.bytes(), 15);
        assert!(pins.release(hold));
        assert_eq!(pins.bytes(), 15);
        let b = admit(&mut pins, 2, vec![allocation(6, 5)]);
        assert_eq!(pins.bytes(), 15);
        assert!(pins.close(a) && pins.close(b));
        assert_eq!(pins.bytes(), 0);
    }

    #[test]
    fn hold_is_atomic_on_inconsistent_lengths_and_budget_overflow() {
        let mut pins = Pins::default();
        let a = admit(&mut pins, 1, vec![allocation(1, 10)]);
        let hold = pins.hold(vec![allocation(2, 20)]).unwrap();
        for bad in [
            vec![allocation(1, 11)],
            vec![allocation(2, 21)],
            vec![allocation(3, 5), allocation(3, 6)],
            vec![allocation(3, MAX_BYTES)],
            vec![allocation(3, u64::MAX), allocation(4, 1)],
        ] {
            assert!(pins.hold(bad).is_err());
            assert_eq!(pins.holds(), 1);
            assert_eq!(pins.bytes(), 30);
            assert_eq!(pins.ids(), vec![a]);
        }
        // A failed hold does not consume an identity.
        let next = pins.hold(vec![]).unwrap();
        assert_eq!(next.0, hold.0 + 1);
        assert_eq!(pins.bytes(), 30);
    }

    #[test]
    fn holds_never_count_as_pins_or_reveal_and_ids_are_never_recycled() {
        let mut pins = Pins::default();
        let ids: Vec<_> = (1..=8).map(|n| admit(&mut pins, n, vec![])).collect();
        // Holds are not limited by the pin count and are invisible to the pin API.
        let holds: Vec<_> = (0..10).map(|_| pins.hold(vec![]).unwrap()).collect();
        assert_eq!(pins.holds(), 10);
        assert_eq!(pins.count(), 8);
        assert_eq!(pins.ids(), ids);
        assert!(ids.iter().all(|id| pins.contains(*id)));
        assert!(pins.admit(SourceId(9), vec![]).is_err());
        // A hold has no source: releasing it must not reveal an individually hidden pin.
        pins.set_visible(ids[0], false);
        pins.release(holds[0]);
        assert!(!pins.is_visible(ids[0]));
        // Hold IDs are monotonic, independent of PinId, and survive release.
        assert!(holds.windows(2).all(|pair| pair[0].0 < pair[1].0));
        pins.release(holds[9]);
        assert!(pins.hold(vec![]).unwrap().0 > holds[9].0);
        assert!(pins.close(ids[1]));
        let pin = admit(&mut pins, 20, vec![]);
        assert_eq!(pin, PinId(9));
        // Closing a pin keeps unrelated holds.
        let held = pins.holds();
        assert!(pins.close(pin));
        assert_eq!(pins.holds(), held);
    }

    #[test]
    fn hold_identity_exhaustion_is_refused_without_wrapping() {
        let mut pins = Pins {
            last_hold: u64::MAX,
            ..Pins::default()
        };
        assert!(pins.hold(vec![allocation(1, 5)]).is_err());
        assert_eq!(pins.holds(), 0);
        assert_eq!(pins.bytes(), 0);
    }

    #[test]
    fn global_hide_and_reveal_during_capture_do_not_override_each_other() {
        let mut pins = Pins::default();
        let a = admit(&mut pins, 1, vec![]);
        let b = admit(&mut pins, 2, vec![]);
        pins.set_visible(b, false);
        pins.begin_capture();
        pins.set_allowed(false);
        pins.reveal(b);
        pins.end_capture();
        assert!(!pins.is_visible(a) && !pins.is_visible(b));
        pins.set_allowed(true);
        assert!(pins.is_visible(a) && pins.is_visible(b));
        pins.set_allowed(false);
        pins.begin_capture();
        pins.set_allowed(true);
        assert!(!pins.is_visible(a));
        pins.set_visible(a, false);
        pins.end_capture();
        assert!(!pins.is_visible(a) && pins.is_visible(b));
    }
}
