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
        let t = spring_ease(elapsed / duration_ms);
        let value = from as f64 + (to - from) as f64 * t;
        on_frame(value.round() as i32);
        ControlFlow::Continue
    })
}
