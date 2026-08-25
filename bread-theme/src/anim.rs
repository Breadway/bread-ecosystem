//! `anim::spring_to` — THEME_SYSTEM_PLAN.md §7/§8: GTK4 has no CSS
//! width/height transition on a widget or a layer-shell surface (unlike the
//! demos' `transition: width .45s var(--spring)` /
//! `transition: max-height .4s var(--spring)`), so a size change that wants
//! to animate has to interpolate a plain integer over the frame clock
//! instead and re-apply it (`set_size_request`, `set_default_width`, ...)
//! every frame.
//!
//! This is the same house technique `breadbar::bar::workspaces::WorkspaceTrail`
//! already uses for the workspace trail's stretch/snap (a local, un-exported
//! `ease`/`ease_overshoot` pair driven by `add_tick_callback` and
//! `Instant::elapsed`) — lifted here, generalized to a plain `i32 -> i32`
//! interpolation with a caller-supplied frame callback, so theme 04's
//! capsule-drawer expand/collapse (and any future popover that wants the
//! same effect) doesn't have to reimplement it.

use gtk4::glib::ControlFlow;
use gtk4::prelude::*;

/// Approximates `cubic-bezier(0.22, 1.35, 0.36, 1)` — the overshoot/"spring"
/// curve every builtin theme's `tokens.spring` names (`Tokens::spring`'s own
/// default). Not a literal bezier solve (same approximation
/// `bar::workspaces::ease_overshoot` uses) — exact enough that the eye can't
/// tell it apart from the real curve at animation speeds.
fn spring_ease(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    let c = 1.35;
    let t1 = t - 1.0;
    1.0 + t1 * t1 * ((c + 1.0) * t1 + c)
}

/// One frame's worth of `spring_to`'s interpolation math — split out from
/// the tick callback below purely so it has a name a unit test can call
/// directly instead of re-deriving the same arithmetic.
///
/// `spring_ease` is a backOut curve that legitimately overshoots past 1.0
/// (peaks ~1.065) partway through the run — fine for a growing animation
/// (`to` > `from`), but on a *shrink* (`from` > `to`, e.g. a drawer
/// collapsing to 0) that overshoot drives the raw interpolated value BELOW
/// `to`, which can go negative for a size request and trip GTK's
/// `height >= -1` assertion. Clamping here, once, protects every caller
/// automatically instead of relying on each call site to remember
/// `h.max(0)` — this already bit breadbar once (see
/// `breadbar::main::animate_drawer_height`'s own clamp, now
/// redundant-but-harmless defense in depth on top of this).
fn frame_value(from: i32, to: i32, t: f64) -> i32 {
    let eased = spring_ease(t);
    let value = from as f64 + (to - from) as f64 * eased;
    let lo = from.min(to) as f64;
    let hi = from.max(to) as f64;
    value.clamp(lo, hi).round() as i32
}

/// Interpolates from `from` to `to` over `duration_ms`, calling `on_frame`
/// with each intermediate value (and, on the final tick, the exact `to` —
/// never an off-by-rounding near-miss) via `widget`'s frame clock.
///
/// Returns the [`gtk4::TickCallbackId`] so a caller that might need to
/// interrupt an in-flight run (e.g. the drawer re-opening before its close
/// animation finished) can `.remove()` it early; a run left to finish on its
/// own needs no cleanup — the callback self-terminates by returning
/// [`ControlFlow::Break`] once `duration_ms` has elapsed, same as
/// `WorkspaceTrail`'s own tick callbacks.
pub fn spring_to(
    widget: &impl IsA<gtk4::Widget>,
    from: i32,
    to: i32,
    duration_ms: f64,
    on_frame: impl FnMut(i32) + 'static,
) -> gtk4::TickCallbackId {
    let started = std::time::Instant::now();
    // `add_tick_callback` requires `Fn`, not `FnMut` — the caller's frame
    // closure almost always needs to mutate captured state (a widget's size
    // request, an `Rc<Cell<..>>` flag), so it's boxed behind a `RefCell`
    // here rather than pushing `Cell`/`RefCell` plumbing onto every call
    // site (every current and future caller wants `FnMut`, none want `Fn`).
    let on_frame = std::cell::RefCell::new(on_frame);
    widget.add_tick_callback(move |_, _| {
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        let mut on_frame = on_frame.borrow_mut();
        if elapsed >= duration_ms {
            on_frame(to);
            return ControlFlow::Break;
        }
        on_frame(frame_value(from, to, elapsed / duration_ms));
        ControlFlow::Continue
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Samples [`frame_value`] — the exact function `spring_to`'s tick
    /// callback calls every frame — across the full `t` timeline at fine
    /// granularity, bypassing the real frame clock (which needs a running
    /// main loop the test environment doesn't have).
    fn sample_all_frames(from: i32, to: i32) -> Vec<i32> {
        let steps = 2000;
        (0..=steps)
            .map(|i| frame_value(from, to, i as f64 / steps as f64))
            .collect()
    }

    #[test]
    fn spring_ease_overshoots_past_one_for_a_backout_curve() {
        // Sanity check on the premise: without a clamp, a shrink would
        // legitimately go negative around this point in the curve.
        assert!(spring_ease(0.8) > 1.0, "expected overshoot past 1.0");
    }

    #[test]
    fn shrink_never_emits_a_value_outside_the_from_to_range() {
        // from > to (a drawer collapsing to 0) is exactly the case the
        // unclamped overshoot could drive negative.
        for value in sample_all_frames(480, 0) {
            assert!(
                (0..=480).contains(&value),
                "shrink emitted {value}, outside [0, 480]"
            );
        }
    }

    #[test]
    fn grow_never_emits_a_value_outside_the_from_to_range() {
        for value in sample_all_frames(0, 480) {
            assert!(
                (0..=480).contains(&value),
                "grow emitted {value}, outside [0, 480]"
            );
        }
    }

    #[test]
    fn zero_span_never_clamps_outside_the_single_point() {
        // from == to: lo == hi == that point for every t, including past
        // the overshoot peak.
        for value in sample_all_frames(120, 120) {
            assert_eq!(value, 120);
        }
    }
}
