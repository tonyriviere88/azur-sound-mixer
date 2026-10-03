//! The glyphs this widget needs and Azur does not ship.
//!
//! Azur's own [`icons`](azur_egui_theme::icons) module is deliberately only about
//! chrome — chevrons, checks, window buttons, status marks — and says domain icons
//! stay in the application. Speakers and microphones are this application's
//! domain, so they live here, drawn to the same contract: fill the rect you are
//! handed, in the colour you are handed, so that any of these is interchangeable
//! with any of Azur's.
//!
//! Painted rather than embedded as an icon font, for the same reason Azur paints
//! its own: nothing to install, and a speaker that stays crisp at 14px in a list
//! row and at 20px in a toggle.

use azur_egui_theme::icons::Pen;
use egui::{pos2, Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2};

/// Azur's icon stroke: 1.5–2px at 16px, never thinner than a hairline.
fn line(rect: Rect) -> f32 {
    (rect.width() / 8.0).clamp(1.0, 2.0)
}

/// The largest square inside `rect` — every glyph is laid out in unit
/// coordinates over this.
fn square(rect: Rect) -> Rect {
    let side = rect.width().min(rect.height());
    Rect::from_center_size(rect.center(), Vec2::splat(side))
}

/// Unit coordinates (`0..=1`, y down) to pixels inside a square.
fn at(r: Rect, x: f32, y: f32) -> Pos2 {
    pos2(r.left() + r.width() * x, r.top() + r.height() * y)
}

fn path(p: &Painter, points: Vec<Pos2>, color: Color32, width: f32) {
    p.add(Shape::line(points, Stroke::new(width, color)));
}

/// An arc, in unit coordinates, swept from `from` to `to` radians.
fn arc(p: &Painter, r: Rect, radius: f32, from: f32, to: f32, color: Color32, width: f32) {
    const STEPS: usize = 14;
    let centre = at(r, 0.51, 0.5);
    let points = (0..=STEPS)
        .map(|i| {
            let angle = from + (to - from) * i as f32 / STEPS as f32;
            centre + Vec2::angled(angle) * radius * r.width()
        })
        .collect();
    path(p, points, color, width);
}

// ---------------------------------------------------------------------------
// Speakers and microphones
// ---------------------------------------------------------------------------
//
// These two are drawn to one shared optical box, because they are the only glyphs
// in the panel that appear stacked one above the other — once in the two master
// rows and again as the two device selects' leading marks. Anything that differs
// between them shows up as a misalignment there.
//
// The box is `INK_LEFT..INK_RIGHT`, centred on `0.5`, and both families fill it:
// the speaker with a cone and its waves, the microphone with a wider cradle than a
// microphone strictly needs. Centring is what makes them right inside a hover fill;
// filling the same width is what makes their left and right edges line up.
//
// The speaker's footprint is the same in *both* of its states, which is the other
// half of the same problem: the glyph is a mute button now, and one that changed
// width when clicked would jump under the pointer.

/// The shared ink box these two glyph families fill.
const INK_LEFT: f32 = 0.15;
const INK_RIGHT: f32 = 0.85;

/// The cone, which both speaker states share: a box on the left opening out into a
/// trapezoid.
fn cone(p: &Painter, r: Rect, color: Color32) {
    p.add(Shape::convex_polygon(
        vec![
            at(r, INK_LEFT, 0.36),
            at(r, 0.29, 0.36),
            at(r, 0.49, 0.16),
            at(r, 0.49, 0.84),
            at(r, 0.29, 0.64),
            at(r, INK_LEFT, 0.64),
        ],
        color,
        Stroke::NONE,
    ));
}

/// A speaker that is on: cone and two waves.
pub fn speaker(p: &Painter, rect: Rect, color: Color32) {
    let r = square(rect);
    let w = line(r).max(1.3);
    cone(p, r, color);
    // Two arcs of the same 100° sweep, a wave apart, about a centre at 0.51 — so the
    // outer one reaches exactly `INK_RIGHT`. The inner is short enough to read as a
    // separate mark at 14px rather than merging with the cone.
    arc(p, r, 0.20, -0.85, 0.85, color, w);
    arc(p, r, 0.34, -0.95, 0.95, color, w);
}

/// A muted speaker: cone, and a cross where the waves were, out to the same
/// `INK_RIGHT` the waves reached.
///
/// A cross rather than a slash across the whole glyph, which is both what the
/// Windows taskbar does and the one that still reads at 14px — a diagonal over
/// the cone turns into a blob at that size.
pub fn speaker_muted(p: &Painter, rect: Rect, color: Color32) {
    let r = square(rect);
    let w = line(r).max(1.3);
    cone(p, r, color);
    let stroke = Stroke::new(w, color);
    p.line_segment([at(r, 0.57, 0.34), at(r, INK_RIGHT, 0.68)], stroke);
    p.line_segment([at(r, INK_RIGHT, 0.34), at(r, 0.57, 0.68)], stroke);
}

/// The speaker glyph for a mute state.
///
/// Two states, not four. An earlier version picked between cone-only, one wave and
/// two by volume, which read nicely in a screenshot and was wrong twice over: the
/// footprint changed with the level, so the glyph slid sideways as the slider moved
/// and never lined up with the microphone below it — and the level is already on the
/// screen three times, as the number, the rail's fill and the meter. This glyph's job
/// is to say whether sound is coming out at all.
pub fn speaker_for(muted: bool) -> azur_egui_theme::icons::Icon<'static> {
    if muted {
        &speaker_muted
    } else {
        &speaker
    }
}

/// The capsule, cradle and stem, which both microphone states share.
fn mic_body(p: &Painter, r: Rect, color: Color32, w: f32) {
    // The capsule: a pill, filled, so the microphone reads as solid the way the
    // speaker's cone does.
    p.add(Shape::rect_filled(
        Rect::from_min_max(at(r, 0.36, 0.08), at(r, 0.64, 0.52)),
        egui::CornerRadius::same(u8::MAX),
        color,
    ));
    // The cradle: a half circle under it, stroked. Its radius is what sets the
    // glyph's width, and 0.32 is chosen so the arc's ends land on `INK_LEFT` and
    // `INK_RIGHT` — a shade wider than a microphone needs, and the reason this glyph
    // and the speaker share left and right edges when stacked.
    const STEPS: usize = 16;
    const CRADLE: f32 = 0.32;
    let centre = at(r, 0.5, 0.44);
    let radius = r.width() * CRADLE;
    let points = (0..=STEPS)
        .map(|i| {
            let angle = i as f32 / STEPS as f32 * std::f32::consts::PI;
            centre + Vec2::new(-angle.cos(), angle.sin()) * radius
        })
        .collect();
    path(p, points, color, w);
    // Stem and base. The stem starts where the cradle bottoms out.
    let stroke = Stroke::new(w, color);
    p.line_segment([at(r, 0.5, 0.44 + CRADLE), at(r, 0.5, 0.90)], stroke);
    p.line_segment([at(r, 0.34, 0.90), at(r, 0.66, 0.90)], stroke);
}

/// A live microphone.
pub fn microphone(p: &Painter, rect: Rect, color: Color32) {
    let r = square(rect);
    mic_body(p, r, color, line(r).max(1.3));
}

/// A muted microphone: the glyph with a slash, as the Windows taskbar draws it.
pub fn microphone_muted(p: &Painter, rect: Rect, color: Color32) {
    let r = square(rect);
    let w = line(r).max(1.3);
    mic_body(p, r, color, w);
    p.line_segment(
        [at(r, 0.14, 0.86), at(r, 0.86, 0.14)],
        Stroke::new(w * 1.1, color),
    );
}

pub fn microphone_for(muted: bool) -> azur_egui_theme::icons::Icon<'static> {
    if muted {
        &microphone_muted
    } else {
        &microphone
    }
}

// ---------------------------------------------------------------------------
// Widget chrome
// ---------------------------------------------------------------------------

/// A thumbtack, standing proud: the *stay open* toggle, off.
pub fn pin(p: &Painter, rect: Rect, color: Color32) {
    let r = square(rect);
    let w = line(r).max(1.3);
    let stroke = Stroke::new(w, color);
    // The head, as a stroked circle: hollow reads as the off state beside the filled
    // version below without needing a second glyph shape.
    p.circle_stroke(at(r, 0.5, 0.34), r.width() * 0.2, stroke);
    p.line_segment([at(r, 0.28, 0.60), at(r, 0.72, 0.60)], stroke);
    p.line_segment([at(r, 0.5, 0.60), at(r, 0.5, 0.90)], stroke);
}

/// The same thumbtack, driven in: *stay open*, on.
pub fn pin_filled(p: &Painter, rect: Rect, color: Color32) {
    let r = square(rect);
    let w = line(r).max(1.3);
    let stroke = Stroke::new(w, color);
    p.circle_filled(at(r, 0.5, 0.34), r.width() * 0.2 + w * 0.5, color);
    p.line_segment([at(r, 0.28, 0.60), at(r, 0.72, 0.60)], stroke);
    p.line_segment([at(r, 0.5, 0.60), at(r, 0.5, 0.90)], stroke);
}

/// The thumbtack for a *stay open* state.
///
/// Driven in when the panel stays; standing proud when it will put itself away. The pin
/// is only half of what it used to mean — where the panel *opens* is the other toggle,
/// [`placement_for`].
pub fn pin_for(stay_open: bool) -> azur_egui_theme::icons::Icon<'static> {
    if stay_open {
        &pin_filled
    } else {
        &pin
    }
}

/// A pointer: the panel opens under the mouse.
///
/// Filled rather than outlined. An arrow this size stroked at 2px is a filled arrow with
/// a lighter middle, which is the exception Azur's §4 allows for shapes too narrow to
/// hold an outline — and the reason the blade and the tail are separate primitives is
/// that egui fills convex polygons only, while a cursor is not one.
pub fn at_pointer(p: &Painter, rect: Rect, color: Color32) {
    let pen = Pen::new(p, rect, color);
    // The blade: a vertical left edge with the tip at the top, which is the shape every
    // desktop has drawn for forty years.
    pen.polygon(&[(4.5, 2.0), (4.5, 11.5), (11.5, 7.4)]);
    // The tail, leaving the blade from inside it so the two read as one arrow.
    pen.line(7.4, 9.0, 9.6, 14.0);
}

/// A panel remembered on a screen: it opens where it was left, on this screen.
///
/// The screen is the outline and the panel is the solid block inside it, off in one
/// corner — "somewhere on the screen, and it knows where" in two shapes. Both are
/// rounded, so the pair reads as a window on a display rather than as two boxes.
pub fn at_last_position(p: &Painter, rect: Rect, color: Color32) {
    let pen = Pen::new(p, rect, color);
    pen.rect(1.6, 3.2, 12.8, 9.6, 1.4);
    // Inset by a whole unit from the screen's inner edge, which is what keeps the two
    // apart at 16px once both have been antialiased.
    pen.rect_filled(8.6, 6.4, 4.4, 4.4, 0.8);
}

/// The glyph for a placement mode.
pub fn placement_for(
    placement: crate::resident::Placement,
) -> azur_egui_theme::icons::Icon<'static> {
    match placement {
        crate::resident::Placement::AtPointer => &at_pointer,
        crate::resident::Placement::LastPosition => &at_last_position,
    }
}

/// The five points of a star, in unit coordinates: outer at `0.5`, inner at
/// `0.21`, first point straight up.
fn star_points(r: Rect) -> [Pos2; 10] {
    let centre = r.center();
    let mut points = [Pos2::ZERO; 10];
    for (i, point) in points.iter_mut().enumerate() {
        let radius = if i % 2 == 0 { 0.47 } else { 0.21 } * r.width();
        let angle = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
        *point = centre + Vec2::angled(angle) * radius;
    }
    points
}

/// A star outline — an application that is not a favourite.
pub fn star(p: &Painter, rect: Rect, color: Color32) {
    let r = square(rect);
    let points = star_points(r);
    let mut outline: Vec<Pos2> = points.to_vec();
    outline.push(points[0]);
    path(p, outline, color, line(r).max(1.2));
}

/// A filled star — a favourite.
///
/// Built out of the inner pentagon plus five point triangles rather than one
/// polygon, because a star is concave and egui's fill is only exact for convex
/// paths. Six convex pieces that share edges tessellate cleanly.
pub fn star_filled(p: &Painter, rect: Rect, color: Color32) {
    let r = square(rect);
    let points = star_points(r);
    let pentagon: Vec<Pos2> = (0..5).map(|i| points[i * 2 + 1]).collect();
    p.add(Shape::convex_polygon(pentagon, color, Stroke::NONE));
    for i in 0..5 {
        let tip = points[i * 2];
        let left = points[(i * 2 + 9) % 10];
        let right = points[i * 2 + 1];
        p.add(Shape::convex_polygon(
            vec![left, tip, right],
            color,
            Stroke::NONE,
        ));
    }
}

pub fn star_for(favourite: bool) -> azur_egui_theme::icons::Icon<'static> {
    if favourite {
        &star_filled
    } else {
        &star
    }
}

/// A box with an arrow leaving it: this opens something that is not here.
///
/// The box is drawn open at the top-right corner rather than closed behind the
/// arrow — the gap is what stops the arrow reading as an arrow *inside* a window,
/// and it is why the glyph still parses at 14px where a closed box plus a diagonal
/// turns into a scribble.
pub fn external(p: &Painter, rect: Rect, color: Color32) {
    let r = square(rect);
    let w = line(r).max(1.2);
    // Left, bottom and the two stubs, as one open path: the missing top-right
    // corner is the whole point.
    path(
        p,
        vec![
            at(r, 0.56, 0.26),
            at(r, 0.12, 0.26),
            at(r, 0.12, 0.88),
            at(r, 0.74, 0.88),
            at(r, 0.74, 0.44),
        ],
        color,
        w,
    );
    // The arrow, out through the gap.
    p.line_segment(
        [at(r, 0.50, 0.50), at(r, 0.90, 0.10)],
        Stroke::new(w, color),
    );
    path(
        p,
        vec![at(r, 0.58, 0.10), at(r, 0.90, 0.10), at(r, 0.90, 0.42)],
        color,
        w,
    );
}

/// Three horizontal sliders — a mixer, for an application whose own icon could not
/// be read.
///
/// Horizontal, where the application's own mark ([`crate::mark::paint`]) is three
/// vertical faders: they are the same idea and they must not be the same glyph, because
/// this one stands in a mixer row for "we could not read this program's icon" and that
/// one names the window it is sitting in.
pub fn sliders(p: &Painter, rect: Rect, color: Color32) {
    let r = square(rect);
    let w = line(r).max(1.2);
    let stroke = Stroke::new(w, color);
    for (y, knob) in [(0.24, 0.68), (0.5, 0.34), (0.76, 0.58)] {
        p.line_segment([at(r, 0.08, y), at(r, 0.92, y)], stroke);
        p.circle_filled(at(r, knob, y), r.width() * 0.11 + w * 0.5, color);
    }
}

/// The mark's colours, checked against the roles they claim to be.
///
/// [`crate::mark`] writes the design system's dark-chrome pair as literals, because a
/// raster mark cannot take a `Color32` from a theme that switches. This is what stops
/// that from quietly becoming a lie — the brief is explicit that an app's mark uses
/// palette colours only, and that "an icon blue that is nearly the app's blue is worse
/// than an obviously different one".
///
/// Both are pinned twice: to the palette rung, and to the role in `Theme::dark()` that
/// the design system's `app-icons/README.md` names for it. The second is the one that
/// matters — the icon is the panel's own `bg.layer` and `accent.mark`, which is why it
/// looks like the window it opens rather than merely being near it.
#[cfg(test)]
#[test]
fn the_mark_uses_the_dark_themes_own_colours() {
    use azur_egui_theme::{tokens::palette, Theme};

    let dark = Theme::dark();
    let bytes = |colour: egui::Color32| [colour.r(), colour.g(), colour.b()];
    assert_eq!(
        crate::mark::PLATE,
        bytes(palette::GRAY_2),
        "the plate has drifted from palette::GRAY_2"
    );
    assert_eq!(
        crate::mark::PLATE,
        bytes(dark.bg.layer),
        "the plate is no longer the dark theme's bg.layer"
    );
    assert_eq!(
        crate::mark::INK,
        bytes(palette::AZURE_70),
        "the mark has drifted from palette::AZURE_70"
    );
    assert_eq!(
        crate::mark::INK,
        bytes(dark.accent.mark),
        "the mark is no longer the dark theme's accent.mark"
    );
}

/// And that the pair measures what [`crate::mark`]'s documentation says it measures.
///
/// The design system's `app-icons/README.md` puts the figure at 6.6:1 and this program
/// repeats it in prose, which makes it a claim of ours too. Pinned rather than merely
/// floored, because the number is quoted: a change that kept it legible but moved it to
/// 5.9 would leave two files saying something that is no longer true.
#[cfg(test)]
#[test]
fn the_marks_two_colours_measure_what_is_claimed_for_them() {
    use azur_egui_theme::contrast::{ratio, TEXT};

    let colour = |[r, g, b]: [u8; 3]| egui::Color32::from_rgb(r, g, b);
    let measured = ratio(colour(crate::mark::INK), colour(crate::mark::PLATE));
    assert!(
        measured >= TEXT,
        "the mark is {measured:.2}:1 on its plate, under {TEXT}:1"
    );
    assert!(
        (measured - 6.6).abs() < 0.05,
        "the mark measures {measured:.2}:1, and both READMEs say 6.6:1"
    );
}
