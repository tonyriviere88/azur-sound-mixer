# Azur Audio Switcher

The three audio controls you actually reach for — which device, how loud, and how
loud *that one application* — in one 340px panel you summon with a keystroke, in the
shape the Xbox Game Bar's audio widget put them in.

```
cargo run --release                            # resident: Win+Alt+V opens it
cargo run --release -- --show                  # and open it straight away
cargo run --release -- --no-stay-open          # hides itself when it loses the focus
cargo run --release -- --at-pointer            # opens under the mouse, not where you left it
cargo run --release -- --hotkey ctrl+alt+f9    # some other keystroke
```

It is **resident**. Starting it puts an icon in the notification area and nothing on
the screen; the panel appears when the hotkey is pressed, and — unless it is set to
stay open — puts itself away again the moment you look at something else. The tray
icon's menu is how you quit.

Rust, [egui](https://github.com/emilk/egui) 0.35, and the
[Azur design system](../azur-egui-theme) for every colour, radius and type step — its
opt-in [`desktop`](../azur-egui-theme/src/desktop.rs) layer for the decisions a dense
application window makes, its [`render`](../azur-egui-theme/src/render.rs) module for the
graphics backend, and its [icon brief](../azur-egui-theme/docs/icons.md) for how the
application's own mark reaches its three surfaces, rather than inventing a look.
Windows only: the backend is Core Audio, and per-application volume has no
equivalent elsewhere that a stub could stand in for.

## What it does

| | |
| --- | --- |
| **Default devices** | Pick the output and input device. Sets all three roles — console, multimedia *and* communications — which is what the Windows sound page does, and what stops your calls from staying on the old headset. |
| **Master volume** | Output and input, on Core Audio's scalar curve — the same one the Windows volume slider moves along, so a number here matches the number there. |
| **Mute** | Click any speaker or microphone glyph. One control, in eight places: the two endpoints and every application. Red when it is off. |
| **Volume mixer** | Every application rendering to the default output, with its real name and its own icon, its own slider and its own mute. Ordered starred → making sound → the rest, alphabetical inside each group. |
| **Level meters** | A 2px bar under each slider: which of these is making the noise. Invisible at silence, so a quiet machine has a still panel. |
| **Favourites** | Star an application to keep it at the top. Remembered in `%APPDATA%\Azur Audio Switcher\favourites.txt` — one name per line, editable by hand. |
| **Resizing** | Drag any edge or corner, or the hatch in the bottom-right. The fixed sections hold their size; the mixer list takes whatever is left. |
| **Summoning** | A global hotkey — `Win+Alt+V` by default, `--hotkey` for anything else — toggles the panel. So does clicking the tray icon. It always opens on the screen the pointer is on. |
| **Stay open** | Header toggle. On: the panel sits above other windows and waits to be dismissed. Off: it hides the moment you click away. |
| **Where it opens** | Header toggle. Either under the mouse, or where you left it — and "where you left it" is measured from the corner of a screen, so it comes back in the same corner of whichever screen the mouse is on. |

Follows the system rather than itself: press a keyboard volume key, or move a
slider in Windows' own mixer, and this moves.

## Four layers

| Layer | Module | Knows about |
| --- | --- | --- |
| Backend | [`audio`](src/audio/) | Core Audio, COM, processes. Nothing about egui. |
| Resident | [`resident`](src/resident.rs) | The tray icon, the hotkey, and the panel's `HWND`. Nothing about what the panel contains. |
| Widget | [`app`](src/app.rs) | Snapshots and commands. Nothing about COM. |
| Glyphs | [`icons`](src/icons.rs), [`widgets`](src/widgets.rs), [`mark`](src/mark.rs) | Azur roles, and an `egui::Painter` — or, for the mark, a table of bitmaps unpacked at build time. |

The boundary in the middle is a thread. Core Audio calls block — enumerating
endpoints talks to the audio service, and a device being reconfigured can take
tens of milliseconds to answer — so the backend runs in its own MTA apartment and
publishes immutable `Snapshot`s, taking `Command`s over a channel. The paint loop
never waits on the audio service, which matters most exactly when it is most
visible: while a slider is being dragged.

```
       Command ──────────────►┌──────────────┐
  app                          │ audio thread │  Core Audio
       ◄────── Arc<Snapshot> ──└──────────────┘
                + request_repaint()
```

## Resident: why the tray and the hotkey are a second thread

An egui application runs code only inside a frame, and **a hidden window is painted
no frames at all** — there is no `WM_PAINT` for a window with no visible region, so
`ViewportCommand` has nowhere to run and `request_repaint` has nothing to repaint. A
widget that waits for its own paint loop to notice a hotkey waits for ever. So
[`resident`](src/resident.rs) owns a plain Win32 thread with its own hidden top-level
window and its own `GetMessage` loop, which keeps running whether the panel is visible
or not, and which does the showing and hiding *itself* with `ShowWindow` on the panel's
`HWND` — the one mechanism that works from either side.

```
resident thread                 egui thread
───────────────                 ───────────
WM_HOTKEY ─┐
tray click ┼─→ toggle ─→ ShowWindow ─→ WM_PAINT ─→ App::ui
menu Show ─┘             shows += 1        ↑          │ focus lost, and
                         wake             └──────────┘ not staying open
                          remember                     ShowWindow(SW_HIDE)
menu Quit  ─→ WM_CLOSE ─→ (winit CloseRequested, eframe returns, tray icon removed)
```

Four details that are not obvious, each of them measured rather than assumed:

- **The window is parked, not hidden, at startup.** `with_visible(false)` does not
  survive: eframe's `EpiIntegration::post_rendering` calls `set_visible(true)` after
  the first painted frame to avoid a white flash, whatever the viewport builder said.
  Re-hiding on the next frame trades eframe's flash for one of our own, so the window
  is created 30,000px off the corner of the desktop instead — where being revealed
  costs nothing — and the panel hides itself properly on the first frame it is given.
- **`WS_EX_TOOLWINDOW` has to be re-asserted, not set.** egui's `with_taskbar(false)`
  removes the taskbar button via `ITaskbarList::DeleteTab` and leaves the window in
  Alt-Tab. Setting the extended style from the constructor reads back correctly and is
  then reverted by winit — so `App::logic`, the one hook that runs even when the window
  is invisible, reads the style every frame and writes only when it has been changed
  under us.
- **The hidden window is top-level, not `HWND_MESSAGE`.** `TaskbarCreated` is a
  broadcast, and broadcasts do not reach message-only windows; without it, restarting
  Explorer would take the tray icon away for good.
- **The backend sleeps while the panel is hidden.** A resident widget is invisible
  almost all of the time, and a 20Hz COM poll computing values nobody is looking at is
  the difference between a widget and a background process. `audio::Wake` is a
  condition variable rather than a flag, because on the way *down* a late answer costs
  nothing while on the way up it sits between the hotkey and a panel with live values
  in it. Waking resets all three clocks: the session list, the device list and the
  defaults are exactly the things that change while a widget is not looking.

Quitting goes through `WM_CLOSE` on the panel rather than anything of ours, so the
path is the one winit and eframe already have: `CloseRequested` → `run_native` returns
→ `main` takes the icon out of the notification area. A hidden window still receives
messages, which is the asymmetry the whole module is built around.

## The mark

Three faders with a speaker tucked under them, blue on a near-black plate. The art is a
render rather than a drawing, and it lives in [`assets/`](assets/) as one PNG per size —
[`assets/icons.py`](assets/icons.py) cuts them out of it, by hand and not as part of the
build.

One image, ten sizes, three surfaces, which is what Azur's
[icon brief](../azur-egui-theme/docs/icons.md) (Part B) asks for:

| surface | how it gets there |
| --- | --- |
| Explorer, shortcuts, a pinned taskbar button | [`build.rs`](build.rs) assembles the ten PNGs into `assets/app.ico` and the linker embeds it |
| the notification area, and the window icon | [`mark::bitmap`](src/mark.rs) at run time, at the size `SM_CXSMICON` asks for |
| the panel's own header, 16px | [`mark::bitmap`](src/mark.rs) again, uploaded as a texture at the size the display scale draws it |

The plate is `#14171C`, which is two steps of blue off `GRAY_2` — the dark theme's
`bg.layer`, the panel's own surface, so the icon is very nearly the colour of the window
it opens, and a test in [`icons`](src/icons.rs) holds it there. The faders are `#3682FB`,
and that one is *not* a token: `accent.mark` is `#3AA0FF` and the render's blue is deeper
than it. The brief's own line — an icon blue that is nearly the app's blue is worse than
an obviously different one — is why it is written down as its own colour rather than
rounded to the rung it is near. On the plate it measures **4.9:1**, which clears
`contrast::TEXT` where the old drawn pair had 6.6:1 to spare.

What the plate costs is worth knowing: near-black against Windows 11's dark taskbar is
about **1.1:1**. On a light taskbar this is a deliberate dark tile; on a dark one the
plate all but disappears and the icon reads as blue faders floating on the chrome — which
is how a good many Windows 11 icons look, and here it is the art's own choice rather than
an accident.

### One pipeline, two media

Ten PNGs — 16, 20, 24, 28, 32, 40, 48, 64, 128 and 256 — are the whole source of truth,
and `build.rs` splits them two ways:

- **The `.ico` is assembled from them verbatim.** An `.ico` is a header, a table and a
  run of payloads, and since Vista each payload may be a whole PNG file. So there is no
  encoder in the build: the bytes Explorer decodes are byte for byte the bytes
  `icons.py` wrote, and the file is 102KB where the same ten images as uncompressed BMP
  entries would be 363KB. `mark::tests::the_ico_holds_every_size` parses the header back
  and checks every entry against the file it came from — it is the one surface with no
  run-time check at all, on a machine that is not this one.
- **The small sizes' pixels are unpacked at build time.** The other two surfaces cannot
  be a file: the notification area wants an `HICON` at `SM_CXSMICON` — 16, 20, 24, 28,
  32, 40, 48 or 64px, decided by the display scale while the program is running — and
  the window icon and the header texture want straight RGBA. So `build.rs` decodes those
  eight into `$OUT_DIR` and [`mark`](src/mark.rs) `include_bytes!`s them, rather than
  putting a PNG decoder in the dependency graph of a binary built with
  `opt-level = "s"`. It costs 44KB of `.rodata` and nothing at startup; the 128 and the
  256, which only the shell reads, stay compressed in the `.ico`. `png` is a *build*
  dependency for that decode and a dev-dependency for the test that the pixels in the
  binary are the pixels in `assets/` — nothing in the shipped binary decodes a PNG.

**One bitmap per size, not one master scaled.** 16px is not a small 64px: each size was
reduced from the render with its own filter pass, and the ones at or under 64 were
sharpened afterwards, which at 16px is the difference between three knobs and three
smudges. So `mark::bitmap` hands back the size it actually has along with the pixels, and
rounds *up* when asked for something in between — a 40px icon shown at 36 is soft, where
a 32px one shown at 36 is soft and blocky. The header asks it for `16 * pixels_per_point`
for the same reason: egui samples a texture bilinearly and keeps no mipmaps, so one 64px
upload drawn into 16 logical pixels would lose the faders and find them again every time
the window changed monitors.

`icons::sliders` — the stand-in for an application whose own icon could not be read —
stays *horizontal* on purpose: it is the same idea as the mark and it must not be the
same glyph, because one of them means "we could not read this program" and the other
names the window it is sitting in.

Two things about the branding that are the shell's to decide, not ours:

- **The tray icon starts in the overflow.** Windows 11 puts every new notification-area
  icon behind the `^` chevron. Drag it onto the taskbar once and it stays.
- **Toasts are titled `azur-audio-switcher`**, the executable's name, because Windows
  takes a notification's identity from an AppUserModelID and a display name registered
  under `HKCU\Software\Classes\AppUserModelId`. Writing to a user's registry to make one
  string prettier is not a thing this program should do on its own; if you want it, it
  is a few lines and a decision about installing.

### What the backend caches, and what invalidates it

Interface pointers, aggressively — because activating `IAudioEndpointVolume` or
building an `IAudioSessionEnumerator` are the expensive calls, while reading a
volume off an interface you already hold is a read out of the audio engine's
shared memory. So:

| Clock | Every | What it does |
| --- | --- | --- |
| Poll | 50ms | Volumes, mutes and peak levels, off already-bound interfaces. |
| Session rescan | 600ms | A new `IAudioSessionEnumerator`; rows already held keep their resolved name and icon. |
| Device rescan | 1500ms | Both device lists and both defaults; rebinds an endpoint whose id has changed. |

Invalidation is by identity, not by time: an endpoint is rebound when the default
device's id stops matching, and a session is dropped when its instance identifier
stops appearing in the enumeration. `Bound` holds every interface for one endpoint
in a single value, so replacing the default device is one assignment and cannot
leave half of them pointing at the device that just stopped being it.

### The one local piece of state

`Echo`. The UI writes 42, the next snapshot still says 38 because it was taken
before the write landed, and the handle jumps backwards under the cursor. So a
value the user just set is trusted over the backend's reading for 400ms **or until
the reported value agrees, whichever comes first** — and the second half matters as
much as the first: it is what makes a write that *did* land stop being echoed
immediately, so an external change a moment later is not swallowed.

## Setting a default device

Windows has no public API for it. The MMDevice API can only *read* the current
default; the setter lives on `IPolicyConfig`, an undocumented interface that the
sound control panel and the Settings app use, and which every third-party device
switcher on Windows has used since Vista. [`audio::policy`](src/audio/policy.rs)
declares it, which means three things are true and all three are said out loud
there:

- **The vtable order is the contract.** COM dispatches by slot, so the ten methods
  before `SetDefaultEndpoint` are declared even though nothing calls them — a
  missing one silently shifts every later slot and calls the wrong function. They
  take opaque pointers, and are the only private methods in the interface.
- **The IID has to be the surviving one.** Two revisions of the interface have
  been in circulation for years under the same name and behind the same coclass.
  `…455c…` is the one the C# switchers spread and most write-ups quote; `…41cf…`
  is the one the C/C++ ones use. On Windows 11 26200 only `…41cf…` is answered —
  the other returns `E_NOINTERFACE` and its bytes are not in `AudioSes.dll` any
  more. Xbox Game Bar, `mmsys.cpl` and the volume flyout all ask that coclass for
  the surviving one. The method list is identical either way.
- **It can disappear.** A failed `CoCreateInstance` means device switching is
  unavailable, not that the widget refuses to start. The panel says so in a
  `MessageBar`, *with the `HRESULT`* — because a bare "not provided" cannot be
  told apart from "provided, under an IID this build no longer answers", which is
  exactly the failure that happened — and the volume controls carry on working.

## Naming and drawing an application

A session knows a pid and, in principle, a display name it set for itself — which
in practice almost nothing does. So the name comes from the executable, the way
Windows' own mixer derives it:

1. the session's `GetDisplayName`, when it is set and is not an unresolved
   `@dll,-id` indirect string;
2. the executable's `FileDescription` — "Microsoft Teams", not "ms-teams.exe" —
   read with the language declared in `\VarFileInfo\Translation` rather than a
   guessed `040904b0`;
3. the file stem, which is what a driver or a helper service without version info
   gets, and what Windows shows for those too.

The icon is the shell's 32×32, `GetDIBits`'d to RGBA and uploaded once per
executable. An icon with no alpha anywhere is a legacy one whose transparency is
in the AND-mask, and that case is handled rather than rendered as a black square.

## Using Azur

One call: **`desktop::install(ctx, true)`**. Azur's `desktop` layer is opt-in, and its
first paragraph names this shape of window — "dense, navigational, open all day — a file
manager, a process table, a launcher, a mixer" — so this panel is not adapting to it so
much as being one of the things it was written for. It brings the fonts, the dark palette
with one role moved, and the density this panel had already asked for by hand
(`control-small`, `selectable_labels: false` — every mixer row is painted rather than laid
out, and a stray text selection over one is only ever an accident).

What the layer changes here, rule by rule:

| | | |
| --- | --- | --- |
| **1. one hover** | `background-control-hover` and `-card-hover` move three rungs up the neutral ramp, not one | Every fill in this panel already read the *token* — `glyph_button`, `link_row`, the selects, the caption buttons — so all of them moved together and no call site changed. Measured off the framebuffer under the pointer: **`#4d525c`**, which is the rung `desktop::hover_fill` names. |
| **2. a selected row** | the accent as a surface | Nothing here is selected: a mixer row is not a list item you pick, and a favourite is marked by a star rather than by a fill. Not adopted, and not an omission. |
| **3. nothing structural is rounded** | `desktop::panel` — square, unringed, marginless | What this panel had reached by hand, now said in one call. It matters twice over here: the corner and the outline both belong to the compositor, so anything drawn at the edge is a second border. |
| **4. rows are 24pt and they touch** | `row::DENSE` | Half adopted, deliberately — see [Density](#density). |
| **5. everything is centred on its ink** | `ink_baseline` | The one that found a real fault. Below. |

Everything else is Azur as it ships:

| Azur | Where |
| --- | --- |
| `Select` | The two device pickers, `Size::Medium`, with a leading speaker / microphone glyph. |
| `Slider` | Every volume — `Size::Medium` for the masters, `Size::Small` in the mixer rows. |
| `Divider`, `Text`, `collection_label`, `IconButton`, `MessageBar` | Chrome, headings, sections. A `Divider` is `stroke-subtle`, which is the colour `desktop::seam` returns — these rules divide sections *inside* one surface rather than two panels, so the component is the right one and the paint agrees anyway. |

Five things are drawn here because Azur has no component for them, and each one
takes its colours from `Theme` roles and nothing else — so moving a role moves
these along with everything Azur draws:

- **`glyph_button`**, a hit target at a computed offset inside a row that is one
  allocation rather than a layout. Every mute in the panel is one of these: the two
  endpoints and every application, the same control in eight places.
- **`peak_meter`**, and the fall rate that makes a 20Hz sample maximum read as a
  level instead of a strobe. `text-tertiary`, not the accent: the accent is the
  colour of the things you can *move* — a slider's fill, a starred row — and the
  meter is the one mark in a row that reports rather than responds. It sits 1px
  under the rail, which puts it inside the slider's own row: Azur centres a rail a
  third the height of the handle in a row the full height of it, so "under the
  slider" would be 7 or 8px from the line the meter is reporting on, with nothing in
  the gap. Being inside the row means the handle crosses it, which is why the shape
  is booked with a reserved `Noop` before the slider is drawn — `reserve_meter` — so
  the handle paints over the meter rather than the meter cutting across the handle.
- **`app_icon`**, an uploaded texture or a fallback glyph, dimmed for an inactive
  session.
- **`link_row`**, which replaced `MenuItem` for "Manage all devices". `MenuItem`
  draws this shape, but its height is `line-body + space-3 × 2` — a fixed 36px, the
  one measurement in the panel that does not follow the control scale. One glyph,
  trailing, and nothing leading: with no leading icon the label starts flush at the
  row's left edge, lining up with the section headings above and below it rather
  than with the padded text inside the selects.
- **`resize_grip`**, the hatch in the corner.

Azur's [`icons`](../azur-egui-theme/src/icons.rs) is deliberately only chrome —
chevrons, checks, window buttons — and says domain icons stay in the application.
Speakers and microphones are this application's domain, so
[`icons`](src/icons.rs) paints them, to the same contract: fill the rect you are
handed, in the colour you are handed. A muted speaker is a cone with a cross where
the waves were and a muted microphone is a slash, because those are what the
Windows taskbar draws and what still read at 14px. "Manage all devices" gets the
box-with-an-arrow-leaving-it, open at the top-right corner — the gap is what stops
the arrow reading as an arrow *inside* a window.

That glyph sits where the row's chevron used to, rather than beside it: a chevron
means "further in, still here", and this row is the one thing in the panel that acts
somewhere other than here. Two marks that disagree about that are worse than one
that does not.

### One baseline per row

Every row in this panel holds a glyph — a speaker, an application's icon, an arrow — and
every one of them used to centre its text by the text's *box*. A line box reserves space
under the baseline for descenders and above the capitals for accents; a device name uses
neither, so box-centred text hangs about **1.5 points** below a glyph centred on its ink.
Uniformly, in every row, which is why it read as the panel being faintly out of true
rather than as a bug.

`components::ink_baseline` is Azur's measurement of that, and `widgets::galley_on_row` is
the two halves of the fix applied together, so the answer comes from one place. One
baseline per *row*, from the row's principal font — a 12px reading given its own ink
baseline beside a 14px name sits a point above it, which is the same fault reappearing
sideways.

Measured off the framebuffer afterwards, on a mixer row: the icon's ink spans rows
343–356 and the name's spans 345–354, so both middles land on **349.5** — 0.0px apart.

### A number that was wrong without looking wrong

`contrast` is public so an application can hold its own colours to the same floor, and
doing that found one. A starred application's glyph was `accent.default` — but in the dark
theme that is the accent's *fill* ramp, moved two rungs down so white holds on a primary
button. As **ink** on `background-layer` it measures **2.75:1**, under the 3:1 WCAG asks
of a shape. `accent.mark` is the same hue kept where it works: **6.56:1**.

`widgets.rs`'s tests now measure every glyph on every fill it can be drawn on — at rest on
the panel, and lit on the hover and press fills — plus the two fills against each other,
and the mark on its plate against the 4.9:1 [The mark](#the-mark) claims for it.

## Density

Every gap in the content column is an explicit `add_space`, with
`item_spacing.y` set to zero — because egui adds item spacing *as well*, between
each pair of items and on **both** sides of every space, which is how an intended
8px gap silently became 24. Zeroing it is what makes the rhythm in the code the
rhythm on screen.

On top of that, Azur's `control-small` scale for the panel's own furniture: a 24px
"Manage all devices" row, and a 36px mixer row — a 20px identity line and a
`Size::Small` slider, two apart, with the meter *inside* the slider's own row rather
than on a strip below it.

That identity line is *below* `row::DENSE`, the densest row Azur names, and stays there:
rule 4's own argument is that every point of row height is a line that did not fit, and 24
would cost one application per four rows for a line that already has 20px of glyph in it.
The half of the rule this does keep is the one that matters more — the rows touch, with
`space-1` between a line and its own slider and nothing between one application and the
next but the 8px block gap.

The largest single saving was deleting a component: a pair of 40px labelled tiles
("Sound on" / "Mic muted") that said what the glyph beside each slider already said,
in three times the space. Mute moved onto the glyphs.

| | Before | After |
| --- | --- | --- |
| Fixed sections | ~490px | ~295px |
| Mixer row | 56px | 36px |
| Rows at the 620px default | 2 | 7 |

Two things are deliberately *out* of that scale, because density is not the only
thing worth having:

- **The masters keep the 18px `Size::Medium` slider.** They are the controls most
  often dragged, and 4px of handle is worth more there than 4px of list.
- **The two device selects are `Size::Medium`** — 32px on the body step. They are
  the only controls in the panel carrying a whole sentence rather than a number or
  a glyph, and a device name is the one string here worth *reading* rather than
  recognising. `Size::Small` would fit a few more characters before the ellipsis at
  the cost of the 12px caption step, which is the wrong trade for the text that
  tells you where your sound is going.

## Window

Undecorated and transparent, so there is no caption for the platform to move the window
by. **Everything in the chrome row that is not a button drags it** — the mark, the app
name, and the space between them and the toggles — at the row's full height, because a
caption you can only grab in a band through the middle feels broken without ever looking
it. `Esc` closes.

Two details make that work. The bands are registered with `ui.interact` *after* the
buttons rather than as one strip across the row, so the buttons keep their own clicks;
and they are clipped to `GRIP` from the window's edge, because the chrome's top margin
is `space-2` = 4px against a 6px resize reach, and `resize_edges` runs *before* the
content — without the clip, two pixels of the top edge would drag the window instead of
resizing it. Measured: at 2px and 5px in, the top edge resizes; at 10px, it drags.

### Two toggles, because they are two questions

| toggle | question it answers | on | off |
| --- | --- | --- | --- |
| **stay open** 📌 | when does it go away? | `WindowLevel::AlwaysOnTop`, and it waits to be dismissed | `Normal`, and it hides as soon as it loses the focus |
| **where it opens** 🖱 | where does it come back? | under the mouse | where you left it, on the mouse's screen |

`--no-stay-open` and `--at-pointer` pick the starting positions; neither is remembered
between runs, the same as every other piece of window state here.

These were **one** control — a pin — on the theory that a panel you want to keep is also
a panel you have placed. That is true often enough to be tempting and not often enough
to be a design: wanting the panel to survive a click elsewhere says nothing about
whether it should reappear under the pointer. The pin also had a third, unasked-for
behaviour — a pinned panel could not be moved by a summon at all — and that is gone.

Always-on-top does still travel with *stay open* rather than being a third switch: a
panel that survives a click elsewhere and then sits behind the window that took the
click has not survived it in any sense the user meant.

### Whichever mode, it opens on the screen the mouse is on

A widget summoned by a keystroke has to arrive in front of the person who pressed it, so
both modes start from `MonitorFromPoint(cursor)` and differ only in where *on* that
monitor the panel lands. That is what makes "where you left it" usable on more than one
screen: the position is remembered as an **offset from a work-area corner**, not as a
desktop coordinate, so a panel parked bottom-right comes back bottom-right of whichever
screen the mouse is on — rather than coming back on a screen nobody is looking at.

The offset is taken on every hide, so dragging the panel and putting it away remembers
the drag. Both modes clamp the result into the work area, which is why summoning near
the bottom of a screen puts the panel's *bottom* against the taskbar rather than its top
under the pointer. Between monitors of different scale the arrival is approximate — the
clamp uses the size the window has before the move and winit rescales it a frame later —
so it can be out by the difference in that one case, and it self-corrects on the next
hide. Opening on the right screen is worth more than the last few pixels.

Dismissing is **hiding**, not closing: `Esc`, the header's ✕, and losing the focus all
put the panel away and leave the program running. Quit is in the tray icon's menu,
which is the only place a resident program's exit belongs.

Four things make the auto-hide behave:

- **It has to have held the focus first.** `InputState::focused` starts `false` and
  winit only says otherwise once the window has actually been given it, so without the
  guard the widget would hide again on the frame it appeared.
- **And the guard resets on every appearance.** `resident` counts shows, and the panel
  compares that count with the one its focus tracking belongs to. Without that, the
  first summon that failed to take the foreground would vanish on its first frame,
  because the guard would still be set from last time.
- **Only the OS focus counts.** `Select` popups and every tooltip in the panel are
  egui `Area`s inside this same viewport, so opening one is not a blur: picking a
  device cannot dismiss the panel out from under the pointer. Dragging and resizing
  are likewise safe — `BeginResize` and `StartDrag` hand over to a platform modal
  loop on this window, which keeps the focus throughout.
- **`Esc` is consumed, not read, and after the content.** A `Select` popup takes
  `Escape` to close itself; checking for it first would put the whole panel away with a
  keystroke aimed at a dropdown.

The one dismissal that looks accidental isn't: *Manage all devices* opens Settings,
Settings takes the focus, and a panel that is not staying open hides behind it. That is what a row
which leaves the widget should do.

**The outline belongs to the compositor.** The panel used to paint its own
`radius-xlarge` corner and `stroke-default` edge, and that produced a double border,
worst in the top corners — because on Windows 11 the window already *has* an outline:
DWM rounds every top-level window and strokes it, whatever the window said about
decorations. Two rounded borders at two radii, with the gap between the arcs showing
through.

DWM's cannot be removed. `DWMWA_BORDER_COLOR = DWMWA_COLOR_NONE` clears the sides and
the bottom but leaves a 1px line across the top, because the window still carries
`WS_CAPTION` for DWM to draw a caption edge from — and clearing that style bit does
not stick, since winit puts it back. The line is composited over the client area, so
nothing egui paints can cover it.

So it goes the other way round: DWM owns the shape, and is told what colour to use
(`DWMWA_BORDER_COLOR` = `stroke-default`, as a `COLORREF`, which is `0x00bbggrr` —
blue first). The panel paints a square, unstroked rect and lets the platform cut it
out, which is what the transparency is for. One border, one radius, antialiased by
the compositor, and the corners clipped by the same code that clips every other
window on the desktop.

The cost is the radius: it is the platform's, not Azur's `radius-xlarge`. That is the
right way round — a window's shape is platform chrome and should match the window
next to it, where `radius-xlarge` is for the cards and dialogs *inside* an
application.

**Resizing is ours to implement.** An undecorated winit window on Windows keeps
`WS_THICKFRAME` but extends its client area over the frame, so `WM_NCHITTEST`
never reports an edge and the platform's own resize borders are unreachable —
dragging an edge does nothing at all. So [`app`](src/app.rs) lays eight invisible
strips over the edges and corners and sends
`ViewportCommand::BeginResize(direction)`; from there the drag is the platform's
own modal resize loop, honouring the minimum size and snapping like any other
window. Three details that are not obvious:

- **Edges are registered before corners, and both before the content.** egui gives
  an overlapping drag to whichever widget was registered *last*, so a corner beats
  the two edges it sits between, and a slider beats all of them.
- **They never actually overlap anything.** The content is inset by `space-3` and
  the mixer's scrollbar sits inside that margin, so a 6px reach is clear of
  everything clickable. Drag-to-scroll would have been the exception, and egui's
  default (`DragScroll::OnTouch`) already leaves it off with a mouse.
- **The grip is drawn in reserved space.** The bottom inner margin is `space-4`
  rather than `space-3` precisely so the hatch has a strip of its own — at the true
  corner it is half outside the rounded fill, and over the list it reads as a
  rendering artefact rather than an affordance.

The rest is cursor feedback: `set_cursor_icon` after the response, so it beats the
pointing hand the theme asks for on anything interactive.

## Presentation: why this is a wgpu window

Not performance — correctness, and it is not this program's decision.
`azur_egui_theme::render` measured the Desktop Window Manager's composed copy of a window
against the frame the application actually rendered, and on an NVIDIA GPU a window
presented through **OpenGL or D3D12 comes back vertically resampled**: 4.85% of pixels
altered, a 1px hairline down from amplitude 30 to 23 and leaking into the row below, lines
moved by up to a whole row. On Vulkan it is byte-identical, 0.37%.

This panel is built almost entirely out of the things that costs: a 1px window outline
handed to DWM, `Divider` rules one pixel tall, a 2px peak meter one pixel under a rail,
and text at 12 and 14 points. So `main.rs` asks for `Renderer::Wgpu` and
`render::wgpu_options()`, which carries the backend, the high-performance adapter and
wgpu's memory-favouring allocator together.

| | glow (before) | wgpu (now) |
| --- | --- | --- |
| release binary | 6.4 MB | 11.4 MB |
| the frame that reaches the screen | resampled once the window goes quiet | as drawn |

The binary is the visible cost and there is a resident-set cost too — the design system
measured about 200 MB, and calls it the price of the frame arriving intact. For a widget
that is invisible most of the time that is the least comfortable trade in this program,
and it is still the right way round: a window that renders the design system and then has
it resampled away has not rendered it. `AZUR_GLOW=1` goes back to the OpenGL painter for a
machine where wgpu will not start; `AZUR_BACKEND` and `AZUR_ADAPTER` are the design
system's own overrides.

One thing this bought for free: screen captures are now worth trusting. The resample
appears *only* on the composed path, and taking a capture forces composition — so under
glow every screenshot in this repository landed on the degraded path and looked soft, which
is indistinguishable from nothing being wrong. Under Vulkan the composed copy is the frame,
so the measurements above ("`#4d525c`", "both middles on 349.5") are measurements of what
the window actually shows.

## Building and deploying

```
cargo run --release           the one to judge anything by
cargo deploy                  release build, then copy it to D:\Programs
cargo deploy E:\Tools         somewhere else
cargo deploy --debug          the debug build, for once
```

`cargo deploy` is an alias in `.cargo/config.toml` standing for
`run --release --example deploy`, because nothing in cargo will build and *then* copy: an
alias can only stand for a subcommand, and `build.rs` runs before the build and never sees
its own output. So the alias runs a small program that does both, and that program is
[`azur_egui_theme::deploy`](../azur-egui-theme/src/deploy.rs), shared with the other Azur
applications — the theme crate's README has the reasoning. `examples/deploy.rs` here is one
call, and what it passes are the two things only this package knows: its binary and its manifest.
`$AZUR_DEPLOY_DIR` moves the destination for every Azur application at once.

It matters more here than in a program you start and quit. This one is **resident**, so the
copy in `D:\Programs` is almost always running when you deploy over it — and Windows will not
let you overwrite the image of a running process. It will let you *rename* it, so the deploy
moves the old binary aside, copies, and sweeps the leftover away next time. What it cannot do
is change the panel already on screen: that is still the old build, and it keeps the hotkey
until you quit it from the tray.

## Version compatibility

egui **0.35**, which is what `azur-egui-theme` tracks. Two egui versions in one
dependency graph are two different `Context` types and the theme would silently
style neither, so:

```
cargo tree -d -p egui
```

`windows-core` is named directly as well as through `windows`, because
`#[interface]` expands to `windows_core::` paths. Same version, so Cargo unifies
them rather than putting two COM runtimes in the graph.

The two places this leans on eframe's *internals* rather than its API are both in the
resident model, and both are pinned to 0.35 by measurement rather than by
documentation: the reveal after the first painted frame
(`EpiIntegration::post_rendering`), and `App::logic` being the hook that still runs
while the window is invisible. If a future eframe changes either, the symptom is
specific and visible — the panel appears at startup instead of staying parked, or it
comes back into the Alt-Tab list — and [`main::PARKED`](src/main.rs) and
[`system::keep_out_of_alt_tab`](src/system.rs) say what to look at.
