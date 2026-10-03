//! The things this widget draws that Azur has no component for.
//!
//! Azur covers the buttons, the selects, the sliders and the type; what is left is a
//! glyph hit target inside a hand-painted row, a level meter, an application's own
//! icon, a row that opens somewhere else, and a resize grip. Each takes its colours
//! from [`Theme`] roles and nothing else, so moving a role moves these along with
//! everything Azur draws.

use azur_egui_theme::components::Size;
use azur_egui_theme::icons::Icon;
use azur_egui_theme::tokens::{radius, space};
use azur_egui_theme::Theme;
use egui::{
    pos2, vec2, Color32, CornerRadius, Id, Rect, Response, Sense, Stroke, TextureHandle, Ui,
};

/// Lay one line out, cut to `width` with an ellipsis.
///
/// Azur has this internally but does not export it, and a hand-painted row needs
/// it for every label it draws.
pub fn truncated(
    ui: &Ui,
    text: &str,
    font: egui::FontId,
    color: Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat::simple(font, color),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.0));
    ui.painter().layout_job(job)
}

/// Paint a galley in a row that also holds a glyph, level with that glyph's ink, with
/// its left edge at `x`.
///
/// This used to centre the galley's *box* in the row, which is right for a row of text
/// alone and wrong the moment a glyph appears beside it: a line box reserves space under
/// the baseline for descenders and above the capitals for accents, and a device name
/// uses neither, so box-centred text hangs about 1.5 points below an icon centred on its
/// ink. Every row in this panel holds a glyph — a speaker, an application's icon, an
/// arrow — so every row had that fault, uniformly enough to read as the panel being
/// slightly out of true rather than as a bug.
///
/// Azur measured the figure and wrote the two halves of the fix, and this is them
/// applied together so the answer comes from one place: `ink_baseline` for where the
/// row's baseline is, `galley_on_baseline` to put a galley on it.
///
/// `font` is the row's **principal** font, not necessarily this galley's. One baseline
/// per row is the whole rule: give a 12px reading its own ink baseline beside a 14px
/// name and it sits a point above it, which is the same fault reappearing sideways.
pub fn galley_on_row(
    ui: &Ui,
    row: Rect,
    font: &egui::FontId,
    x: f32,
    galley: std::sync::Arc<egui::Galley>,
) {
    let baseline =
        azur_egui_theme::components::ink_baseline(ui.painter(), font, row.top(), row.height());
    azur_egui_theme::components::galley_on_baseline(ui.painter(), x, baseline, galley);
}

// ---------------------------------------------------------------------------
// glyph_button
// ---------------------------------------------------------------------------

/// A bare glyph that reacts to the pointer, for use inside a row that is painted
/// rather than laid out.
///
/// Azur's `IconButton` would do this in a `Ui`, but a mixer row places its
/// controls at computed offsets — the row is one allocation, so the pieces inside
/// it are `interact` calls on rects rather than widgets in a layout.
pub fn glyph_button(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    icon: Icon<'_>,
    resting: Color32,
    tooltip: &str,
) -> Response {
    let t = azur_egui_theme::components::theme(ui.ctx());
    let response = ui.interact(rect, id, Sense::click());
    let hovered = response.hovered();
    let pressed = response.is_pointer_button_down_on();
    // Both rungs from `desktop`, which is where the window's one hover grey and the
    // rung past it are decided. Asked for by name rather than read off `bg.*` so that
    // this says what it is — a control being hovered and then pressed — and follows the
    // module if it ever reads a different token back.
    let (hover, press) = azur_egui_theme::desktop::control_fills(&t);
    if hovered || pressed {
        let fill = if pressed { press } else { hover };
        ui.painter()
            .rect_filled(rect, CornerRadius::same(radius::SMALL), fill);
    }
    // Only the resting colour is the caller's. Lit, the glyph goes to `text-primary`:
    // a `text-tertiary` glyph on the hover fill has less contrast lit than it had
    // unlit, and on `desktop`'s rung — three up the ramp rather than one — that is
    // twice as true. `the_glyph_is_legible_on_every_fill_it_is_drawn_on` measures it.
    let colour = if hovered || pressed {
        t.text.primary
    } else {
        resting
    };
    let glyph = Rect::from_center_size(rect.center(), vec2(14.0, 14.0));
    icon(ui.painter(), glyph, colour);
    if !tooltip.is_empty() {
        return response.on_hover_text(tooltip);
    }
    response
}

// ---------------------------------------------------------------------------
// peak_meter
// ---------------------------------------------------------------------------

/// The meter's own height.
pub const METER_HEIGHT: f32 = 2.0;

/// Between the rail's bottom edge and the meter's top.
///
/// One pixel. The meter is a readout of the rail directly above it, and at any more
/// than a hairline the two stop reading as one thing and start reading as two rows.
const METER_GAP: f32 = 1.0;

/// Where the meter goes for a slider whose input row is `row`.
///
/// Under the *rail*, not under the row. Azur centres a rail a third the height of
/// the handle inside a row the full height of the handle, so the empty lower half of
/// the handle's travel sits between the two — putting the meter below the row leaves
/// it 7 or 8px away from the line it is reporting on, with nothing in the gap.
///
/// The rail heights are the one thing here that reaches inside a component this
/// crate does not own: `Slider::metrics` is private to the theme, so the table is
/// repeated. It is three numbers, and a wrong one is visible immediately.
pub fn meter_rect(row: Rect, size: Size) -> Rect {
    let rail = match size {
        Size::Small => 4.0,
        Size::Medium => 6.0,
        Size::Large => 8.0,
    };
    let top = row.center().y + rail * 0.5 + METER_GAP;
    Rect::from_min_max(pos2(row.left(), top), pos2(row.right(), top + METER_HEIGHT))
}

/// Book a place in the paint list for a meter, before the slider is drawn into it.
///
/// The meter has to be painted *under* the slider — its band is inside the handle's
/// travel, so a handle passing over it must cover it rather than be cut by it — but
/// its position is only known from the slider's rect, which does not exist until the
/// slider has been added. A reserved `Noop` resolves that: the slot keeps its place
/// in the draw order while the shape that fills it is decided afterwards.
///
/// This is the same trick Azur's own `Select` uses to paint a trigger's background
/// behind content laid out after it.
pub fn reserve_meter(ui: &Ui) -> egui::layers::ShapeIdx {
    ui.painter().add(egui::Shape::Noop)
}

/// Fill in a reserved meter: how loud this thing is *right now*.
///
/// Nothing is drawn at silence — the slot simply stays a `Noop`. No empty track, no
/// zero-width stub. A panel with a dozen rows in it should be still when the machine
/// is, and the meter's job is to answer "which one of these is making the noise",
/// which an unlit row answers as well as a lit one does.
///
/// `text-tertiary`, not the accent: the accent is the colour of the things you can
/// *move* — a slider's fill, a starred row — and a meter is the one mark in a mixer
/// row that reports rather than responds. It still reads as lit, because
/// `text-tertiary` is several steps up the neutral ramp from the `stroke-subtle`
/// rail directly above it.
pub fn peak_meter(ui: &Ui, slot: egui::layers::ShapeIdx, rect: Rect, level: f32, t: &Theme) {
    let level = level.clamp(0.0, 1.0);
    if level <= 0.002 {
        return;
    }
    let mut bar = rect;
    bar.max.x = rect.left() + rect.width() * level;
    ui.painter().set(
        slot,
        egui::Shape::rect_filled(bar, CornerRadius::same(radius::CIRCULAR), t.text.tertiary),
    );
}

/// Seconds for a meter to fall from full scale to silence.
const PEAK_FALL: f32 = 0.6;

/// Decay a peak level towards a new reading.
///
/// Raw `IAudioMeterInformation` peaks at 20Hz flicker — they are instantaneous
/// sample maxima, not an envelope. Rising instantly and falling over about half a
/// second is what a level meter is expected to do, and it is the fall that makes
/// the bar readable as a level rather than a strobe.
pub fn decay_peak(previous: f32, reading: f32, dt: f32) -> f32 {
    reading.max(previous - dt / PEAK_FALL).clamp(0.0, 1.0)
}

// ---------------------------------------------------------------------------
// app_icon
// ---------------------------------------------------------------------------

/// An application's own icon, or a speaker in its place.
///
/// `dim` is for an inactive session: the row's text drops to `text-tertiary`, and
/// an icon that stayed at full opacity beside it would be the brightest thing in a
/// row that is meant to be receding.
pub fn app_icon(
    ui: &Ui,
    rect: Rect,
    texture: Option<&TextureHandle>,
    fallback: Icon<'_>,
    fallback_colour: Color32,
    dim: bool,
) {
    match texture {
        Some(texture) => {
            let tint = if dim {
                Color32::from_white_alpha(110)
            } else {
                Color32::WHITE
            };
            ui.painter().image(
                texture.id(),
                rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                tint,
            );
        }
        None => fallback(ui.painter(), rect, fallback_colour),
    }
}

// ---------------------------------------------------------------------------
// section_toggle
// ---------------------------------------------------------------------------

/// A section heading that opens and closes: the label, and a chevron at the end
/// saying which way it will go.
///
/// Drawn to read as a *heading* rather than as a control, because the two sections
/// either side of it are headings — `collection_label`'s body step in
/// `text-secondary` — and a row that shouted would make the panel look like it had
/// three different kinds of section in it. What says it can be clicked is the hover
/// fill and the chevron, which is enough for a disclosure and no more than one.
///
/// The label is flush at the row's left edge so that it lines up with the headings
/// above and below rather than with the padded text inside the selects, and the
/// chevron is inset by the horizontal padding of a `Size::Medium` control, which
/// puts it in the same column as those selects' own chevrons. The row is padded on
/// one side only, deliberately: each edge answers to the thing it sits under.
///
/// Azur has no accordion, and this is not one — an accordion owns its content and
/// animates it. The caller keeps the `bool` and simply draws less, which is what a
/// panel whose height is the user's to set wants: a collapsed section gives its
/// space to the mixer list, and nothing has to be told. It is also what lets that
/// `bool` live somewhere a widget could not put it — for the one caller here, in
/// [`crate::settings`], so the section comes back the way it was left.
///
/// The height is `control-small`, matching the selects it sits over. Azur's
/// `MenuItem` is the other row of this shape and is a fixed 36px, which is the one
/// measurement in this panel that does not follow the control scale.
pub fn section_toggle(ui: &mut Ui, label: &str, open: bool) -> Response {
    let t = azur_egui_theme::components::theme(ui.ctx());
    let height = azur_egui_theme::tokens::control::SMALL;
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }

    let corner = CornerRadius::same(radius::SMALL);
    if response.hovered() {
        // `hover_fill`, not `control_fills`: this row is a *place* in the panel rather
        // than a control being operated, which is the distinction rule 1 of `desktop` is
        // drawn around. Both are the same colour by design, and saying which one it is
        // here keeps that a deliberate agreement rather than a coincidence.
        ui.painter()
            .rect_filled(rect, corner, azur_egui_theme::desktop::hover_fill(&t));
    }
    // 12px: Azur's size for a chevron, which is the glyph this is. The 14 the external
    // link mark needed was for a box, an arrow and an arrowhead in one square; two
    // strokes need no such room, and a chevron drawn larger than Azur draws it would not
    // match the ones inside the selects underneath.
    const GLYPH: f32 = 12.0;
    const TRAILING: f32 = space::S4;
    // Down when it is shut, up when it is open — the direction the *content* will move,
    // which is the way every disclosure on the desktop is drawn and the opposite of
    // pointing at the state.
    let chevron: Icon<'_> = if open {
        &azur_egui_theme::icons::chevron_up
    } else {
        &azur_egui_theme::icons::chevron_down
    };
    chevron(
        ui.painter(),
        Rect::from_center_size(
            pos2(rect.right() - TRAILING - GLYPH * 0.5, rect.center().y),
            vec2(GLYPH, GLYPH),
        ),
        t.text.secondary,
    );
    let text = Rect::from_min_max(
        rect.min,
        pos2(rect.right() - TRAILING - GLYPH - space::S3, rect.bottom()),
    );
    let galley = truncated(
        ui,
        label,
        t.fonts.body.clone(),
        t.text.secondary,
        text.width(),
    );
    // On the row's ink baseline, because the row holds a glyph: the label has to be
    // level with the chevron at the far end of it.
    galley_on_row(ui, rect, &t.fonts.body, text.left(), galley);

    if response.has_focus() {
        azur_egui_theme::icons::focus_ring(ui.painter(), rect, corner, t.stroke.focus);
    }
    response
}

// ---------------------------------------------------------------------------
// resize_grip
// ---------------------------------------------------------------------------

/// A triangular hatch in the bottom-right corner: the one part of resizing that has
/// to be *seen*, because an undecorated window offers nothing else to suggest it.
///
/// Three diagonals anchored at the corner, each longer than the last — the shape
/// Windows' own sizing grip draws — in `stroke-strong`, the role Azur uses for a
/// control's outline under the pointer. Visible when looked for, quiet when not.
///
/// `area` must be clear of both the panel's rounded corner and any content: the
/// caller reserves it in the layout, because a tick over the last mixer row reads as
/// a rendering artefact rather than an affordance.
pub fn resize_grip(ui: &Ui, area: Rect, t: &Theme) {
    let stroke = Stroke::new(1.0, t.stroke.strong);
    for i in 1..=3 {
        let reach = area.width() * i as f32 / 3.0;
        ui.painter().line_segment(
            [
                pos2(area.right() - reach, area.bottom()),
                pos2(area.right(), area.bottom() - reach),
            ],
            stroke,
        );
    }
}

#[cfg(test)]
mod tests {
    use azur_egui_theme::contrast::{apart, ratio, SAME, SHAPE};
    use azur_egui_theme::desktop;

    /// Every glyph this module paints, on every fill it can be painted on.
    ///
    /// The floor is [`SHAPE`] — 3:1 — rather than the 4.5:1 a label needs, because these
    /// are glyphs, and that is the line WCAG draws for a mark whose meaning is its
    /// outline. `desktop`'s theme is the one under test, not `Theme::dark()`: its hover
    /// rung is three steps up the neutral ramp instead of one, which is a much brighter
    /// thing to put a glyph on, and the point of measuring is that the window renders the
    /// adjusted palette rather than the plain one.
    ///
    /// This is the check that found a real fault. The starred glyph was `accent.default`
    /// — the *fill* ramp, which the dark theme moves two rungs down so white holds on a
    /// primary button. As ink on `background-layer` that is **2.75:1**, under the floor.
    /// `accent.mark` is the same hue kept where it works, at 6.56:1. Nothing looked
    /// wrong; the number was wrong.
    #[test]
    fn the_glyph_is_legible_on_every_fill_it_is_drawn_on() {
        let t = desktop::theme(true);
        let (hover, press) = desktop::control_fills(&t);

        // At rest a glyph is on the panel, in whichever role its caller passed.
        for (name, ink) in [
            ("text.secondary", t.text.secondary),
            ("text.tertiary", t.text.tertiary),
            ("status.danger — a muted row", t.status.danger),
            ("accent.mark — a starred row", t.accent.mark),
        ] {
            let measured = ratio(ink, t.bg.layer);
            assert!(
                measured >= SHAPE,
                "{name} at rest on the panel is {measured:.2}:1, under {SHAPE}:1"
            );
        }

        // Lit, every glyph is `text.primary`, on one of the two control fills.
        for (name, fill) in [("hover", hover), ("press", press)] {
            let measured = ratio(t.text.primary, fill);
            assert!(
                measured >= SHAPE,
                "a lit glyph on the {name} fill is {measured:.2}:1, under {SHAPE}:1"
            );
        }

        // And the reason a lit glyph changes role at all: lighting a glyph must never
        // cost contrast. Leaving the dimmest resting role in place under the pointer
        // would — `text.tertiary` reads 3.78:1 on the panel and 1.65:1 on the hover fill
        // — so the invariant is stated against that worst resting case.
        let lit = ratio(t.text.primary, hover);
        let dimmest_at_rest = ratio(t.text.tertiary, t.bg.layer);
        assert!(
            lit >= dimmest_at_rest,
            "a lit glyph ({lit:.2}:1) is dimmer than the faintest resting one \
             ({dimmest_at_rest:.2}:1), so the pointer is taking contrast away"
        );
    }

    /// The two fills read as two surfaces, and both read as not-the-panel.
    ///
    /// [`SAME`] rather than a contrast ratio: none of these three is ink, and 3 ΔL* is
    /// the point below which two neutral surfaces stop being two surfaces. A press that
    /// cannot be told from a hover is a control with no feedback.
    #[test]
    fn the_fills_are_told_apart_from_each_other_and_from_the_panel() {
        let t = desktop::theme(true);
        let (hover, press) = desktop::control_fills(&t);
        for (name, fill, under) in [
            ("the hover fill", hover, t.bg.layer),
            ("the press fill", press, hover),
        ] {
            let measured = apart(fill, under);
            assert!(
                measured >= SAME,
                "{name} is {measured:.1} ΔL* from what it sits on, under {SAME}"
            );
        }
    }

    /// The meter is a 2px bar on the panel, and it has to be seen without being loud.
    ///
    /// `text-tertiary` is deliberately the quietest role that still clears the floor: the
    /// accent belongs to the things you can *move*, and the meter is the one mark in the
    /// panel that is a readout rather than a control.
    #[test]
    fn the_meter_reads_against_the_panel() {
        let t = desktop::theme(true);
        let measured = ratio(t.text.tertiary, t.bg.layer);
        assert!(
            measured >= SHAPE,
            "the meter is {measured:.2}:1 against the panel, under {SHAPE}:1"
        );
    }
}
