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
cargo run --release -- --profile test          # settings and favourites of its own
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
| **Default devices** | Pick the output and input device: the console and multimedia roles, which every ordinary sound on the machine uses. Calls have their own pair — the **Communications** section below — and this does not touch them. |
| **Communications** | Windows keeps a *second* pair of defaults, for calls, and this is the control for it: its own section under the first, one row wide until you open it. Independent of the section above, so a headset chosen for calls stays chosen. |
| **Master volume** | Output and input, on Core Audio's scalar curve — the same one the Windows volume slider moves along, so a number here matches the number there. |
| **Mute** | Click any speaker or microphone glyph. One control, in eight places: the two endpoints and every application. Red when it is off. |
| **Volume mixer** | Every application rendering to the default output, with its real name and its own icon, its own slider and its own mute. Ordered starred → making sound → the rest, alphabetical inside each group. |
| **Level meters** | A 2px bar under each slider: which of these is making the noise. Invisible at silence, so a quiet machine has a still panel. |
| **Favourites** | Star an application to keep it at the top. Remembered in `%APPDATA%\Azur Audio Switcher\favourites.txt` — one name per line, editable by hand. |
| **Resizing** | Drag any edge or corner, or the hatch in the bottom-right. The fixed sections hold their size; the mixer list takes whatever is left. |
| **Summoning** | A global hotkey — `Win+Alt+V` by default, `--hotkey` for anything else — toggles the panel. So does clicking the tray icon. It always opens on the screen the pointer is on. |
| **Stay open** | Header toggle. On: the panel sits above other windows and waits to be dismissed. Off: it hides the moment you click away. |
| **Where it opens** | Header toggle. Either under the mouse, or where you left it — and "where you left it" is measured from the corner of a screen, so it comes back in the same corner of whichever screen the mouse is on. |
| **Windows audio settings** | The ⧉ in the header, leftmost of its four buttons. Everything past picking a device — sample rates, spatial audio, exclusive mode — is the long tail, and the page that already has it is a better place for it than a 340px panel. |
| **Remembered** | Both header toggles and whether the Communications section is open, in `%APPDATA%\Azur Audio Switcher\settings.txt` — three `key = value` lines beside the favourites, editable by hand. Set them once and they are how the widget starts tomorrow. `--profile <name>` gives a run its own copy of both files. |

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

The plate is a **vertical gradient**, about `#232529` across the top down to `#16181C` at
the bottom, and it passes through `#14171C` — two steps of blue off `GRAY_2`, the dark
theme's `bg.layer` and the panel's own surface — near its lower edge. So the icon is very
nearly the colour of the window it opens along one band of the tile rather than all of it,
and a test in [`icons`](src/icons.rs) holds that band there. The faders are `#1E7CFB`, and
that one is *not* a token: `accent.mark` is `#3AA0FF` and the render's blue is deeper than
it. The brief's own line — an icon blue that is nearly the app's blue is worse than an
obviously different one — is why it is written down as its own colour rather than rounded
to the rung it is near.

**The mark does not clear `contrast::TEXT` where it sits, and that is a decision rather
than an oversight.** On the plate's darkest band the pair measures 4.6:1; against the
plate immediately behind the knobs it is 4.3:1, and against the lightest band the topmost
knob sits on, **3.9:1** — taken over every knob-face pixel against the plate row behind
it, the median is 4.2:1 and four fifths are under 4.5. The previous render, a flat plate
and a lighter blue, was 4.9:1 everywhere and the drawn pair before it 6.6:1. `TEXT` is
WCAG AA for a line of body text, an application icon is not read, and this art was chosen
for how it looks at 16px in a notification area — so the shortfall is accepted, written
down in [`mark`](src/mark.rs), and pinned from both sides by a test so that a future
relight cannot move it without saying so.

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

### Two roles, out of Windows' three

`SetDefaultEndpoint` takes one `ERole` and Windows keeps three of them — `eConsole`,
`eMultimedia`, `eCommunications` — but only one of those boundaries is a boundary
anyone can hear. Nothing in the shipping Windows UI separates console from multimedia;
the sound page moves them together, and so does this. So `audio::Role` keeps two:

| `Role` | writes | is |
| --- | --- | --- |
| `Default` | `eConsole` + `eMultimedia` | the *Default devices* section: where sound goes |
| `Communications` | `eCommunications` | the *Communications* section: where a call goes |

**The two are disjoint, and that is the design.** Neither writes the other's role, so
each section means exactly what it says: a device chosen for calls stays chosen however
often the device for sound changes, which it could not do if the section above wrote
this role as well. Two controls that can silently undo one another are not two controls.

The cost is the other side of the same coin and is worth saying plainly: **picking a
headset in *Default devices* alone will not move your calls onto it.** The Windows sound
page conflates the two and this does not, so a machine with a headset needs both set —
and the *Communications* section is the answer to "why are my calls still on the
speakers", which is why it is one click from being visible rather than somewhere else.

Reading is the asymmetric half: `Role::Default` is read back off `eConsole` alone, since
one role answers for both when the two are only ever written together.

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
| **1. one hover** | `background-control-hover` and `-card-hover` move three rungs up the neutral ramp, not one | Every fill in this panel already read the *token* — `glyph_button`, `section_toggle`, the selects, the caption buttons — so all of them moved together and no call site changed. Measured off the framebuffer under the pointer: **`#4d525c`**, which is the rung `desktop::hover_fill` names. |
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
- **`section_toggle`**, the *Communications* heading that opens and closes. Drawn to
  read as a heading rather than as a control — `collection_label`'s body step in
  `text-secondary`, the same as the two sections either side of it — because a row
  that shouted would make the panel look like it had three kinds of section in it.
  What says it can be clicked is the hover fill and the chevron. The label is flush
  at the row's left edge, lining up with those headings rather than with the padded
  text inside the selects; the chevron is inset by a `Size::Medium` control's
  horizontal padding, which puts it in the same column as those selects' own
  chevrons. Padded on one side only, deliberately: each edge answers to the thing it
  sits under. Its height is `control-small`, where Azur's `MenuItem` — the other row
  of this shape — is a fixed 36px, the one measurement in the panel that does not
  follow the control scale.
- **`resize_grip`**, the hatch in the corner.

Azur's [`icons`](../azur-egui-theme/src/icons.rs) is deliberately only chrome —
chevrons, checks, window buttons — and says domain icons stay in the application.
Speakers and microphones are this application's domain, so
[`icons`](src/icons.rs) paints them, to the same contract: fill the rect you are
handed, in the colour you are handed. A muted speaker is a cone with a cross where
the waves were and a muted microphone is a slash, because those are what the
Windows taskbar draws and what still read at 14px. Windows audio settings gets the
box-with-an-arrow-leaving-it, open at the top-right corner — the gap is what stops
the arrow reading as an arrow *inside* a window.

That glyph is in the **header** now, leftmost of the four buttons, where it used to
be a labelled row at the foot of the devices section. Two reasons, and the second is
the one that decided it: a row that leaves the widget was the only row in that column
not doing what the rows around it do, and those 24px belong to the mixer list. A
chevron would have been the wrong mark either way — a chevron means "further in,
still here" — so the glyph is the whole message, which is what lets it drop its label
and become an icon button beside the three that are about the window.

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
and the mark against both ends of its plate's gradient: the 4.6:1 and the 3.9:1
[The mark](#the-mark) claims for it, each pinned from both sides so neither the figure nor
the accepted shortfall can drift.

## Density

Every gap in the content column is an explicit `add_space`, with
`item_spacing.y` set to zero — because egui adds item spacing *as well*, between
each pair of items and on **both** sides of every space, which is how an intended
8px gap silently became 24. Zeroing it is what makes the rhythm in the code the
rhythm on screen.

On top of that, Azur's `control-small` scale for the panel's own furniture: a 24px
*Communications* heading row, and a 36px mixer row — a 20px identity line and a
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
name, and the space between them and the buttons — at the row's full height, because a
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

Four buttons in that row, not three: **⧉ Windows audio settings** sits leftmost, then
these two, then ✕. It is the only one of the four that is not about this window, which
is why it is at the far end of them — and it is a plain button rather than a toggle, so
it has no state to draw and needs no `pressed`.

**Both are remembered between runs**, in `%APPDATA%\Azur Audio Switcher\settings.txt`,
along with the one piece of layout that is also a preference — whether the *Communications*
section is open:

```
stay-open = off
placement = at-pointer
communications = shut
```

They are settings rather than window state, which is the distinction that decides what
gets written down here. They say how the panel should *behave* — whether it gets in the
way, where it arrives, what of it there is to look at — and somebody who turned *stay open*
off because a panel over their work is the thing they disliked about it does not want to turn
it off again tomorrow. The window's size and position are still not kept: those are where the
panel happened to be sitting, and "where you left it" is already remembered for as long as it
means anything, which is this session on this arrangement of monitors.

The section earns its place in that file on the same test. The argument against was that a
disclosure which remembers being opened is a section that is never shut again; the answer is
that shutting it is remembered too, so what comes back is the state it was last *left* in —
shut, for the majority who never open it, and the two pickers for somebody who runs a headset
and speakers. `open`/`shut` rather than a third `on`/`off`, because `communications = off`
is a line somebody would read as "do not use a communications device", which is not a setting
this program has and would be an alarming one to think you had turned on by accident.

`--no-stay-open`, `--at-pointer` and `--last-position` are then **overrides for one
run**: they pick what the panel starts as and do not replace what is written down, so a
flag typed into a shell to try something stays a try rather than becoming the preference.
Clicking a toggle is what writes — which is also what brings the two back into step. See
[`settings`](src/settings.rs), which holds the file's values while `app` holds the run's
and says why that is two copies rather than one.

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
  loop on this window, which keeps the focus throughout. That last part is true of a
  window the platform has made *active*; see [A panel without the focus](#a-panel-without-the-focus-has-to-still-be-a-panel)
  for what it took to make it true of every appearance.
- **`Esc` is consumed, not read, and after the content.** A `Select` popup takes
  `Escape` to close itself; checking for it first would put the whole panel away with a
  keystroke aimed at a dropdown.

The one dismissal that looks accidental isn't: the header's ⧉ opens the Windows sound
page, that page takes the focus, and a panel that is not staying open hides behind it.
That is what a button which leaves the widget should do.

### A panel without the focus has to still be a panel

The bug this section is here for: **sometimes the very first appearance would not drag and
would not open a device list.** Hiding it and summoning it again cured it, which is what
made it look like a paint or a timing problem. It was neither — it was the focus, and two
separate faults hanging off it.

`SetForegroundWindow` is a request, not a call. Windows grants a foreground change to the
process that received the last input event, and on the first summon after launch that claim
is at its weakest: a hotkey press is ours, but a **tray click belongs to the shell** and
`--show` belongs to nobody. So the panel can be up, painted, on top, and not have the focus
— reproduced here by summoning it with a posted tray message, where the grant is never
given. What that state then cost:

| | what happened | why |
| --- | --- | --- |
| **No combobox would open** | the click opened it, the next frame closed it, for as long as the panel was up | `desktop::close_popups_on_blur` is level-triggered — it closes every popup on every frame while `!focused`, which is *while* blurred rather than *on* blur |
| **The window would not drag** | `StartDrag` moved it 0px; the same drag on an active window moves it | winit's `drag_window` is `WM_NCLBUTTONDOWN` + `HTCAPTION`, and the modal move loop behind it wants an active window. The press that starts the drag would activate one, but not before the command is sent in that same frame |

Both are fixed where they belong, and each was measured on its own with the other disabled:

- **`App::close_stale_popups`** replaces the level-triggered helper with the edge: popups
  close on the frame the focus is *lost*, and on the frame the panel is *shown again*.
  Between them they still cover the fault that helper existed for — a device list left open
  behind a click into another program, hanging there over a list that may no longer hold
  the same devices when the panel comes back — and they leave a popup alone in a panel
  which never had the focus to lose. The predicate is three booleans and one line, pulled
  out and given a truth table in `popups_are_stale`, because the bug *was* the predicate.
- **The focus is asked for twice more**, from the side that has the best claim each time:
  once on every fresh appearance, from the window's own thread (winit's `focus_window`,
  which the platform honours where the resident thread's attempt was refused), and once
  when a drag starts — where the press that started it *is* the last input event, so the
  grant is ours to have.

Measured, with a posted tray click so the panel never gets the foreground the easy way:
before, no popup and 0px of drag; with only the edge-triggered close, the popup opens and
the drag is still dead; with only the drag-start focus, 50px of drag; with both, both.

The wider principle is a promise the auto-hide already makes above — "a panel that never
receives the focus at all stays up until it is dismissed, which is the right way for this
to fail". That is only true if a panel in that state is a *working* panel.

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

| | glow (before) | wgpu (as shipped) |
| --- | --- | --- |
| release binary | 6.4 MB | 5.8 MB |
| the frame that reaches the screen | resampled once the window goes quiet | as drawn |

That table used to read 11.4 MB and be the uncomfortable half of this decision. It does
not any more, and the reason is worth stating: **almost none of the 5 MB that came off was
wgpu.** It was four embedded fonts behind a font read that cannot fail, every graphics
backend except the one this window presents on, an OpenGL painter kept as a fallback, and
unwinding tables for panics nothing catches — see [Size](#size). The wgpu window now costs
less than the glow window it replaced.

There is still a resident-set cost — the design system measured about 200 MB, and calls it
the price of the frame arriving intact. For a widget that is invisible most of the time
that is the least comfortable trade left in this program, and it is still the right way
round: a window that renders the design system and then has it resampled away has not
rendered it. `AZUR_BACKEND` and `AZUR_ADAPTER` are the design system's own overrides.

`AZUR_GLOW=1` is gone, and so is the OpenGL painter it selected. It was an escape hatch
for a machine where wgpu will not start, which nobody has reported, and it was 313,856
bytes of every copy — but the argument that retired it is not the bytes: OpenGL is the one
row of the table above that *both* leaks and loses the hairlines, so the fallback rendered
the design system wrong. A fallback that produces a window this program would not ship is
not insurance. Setting the variable now prints a line saying it does nothing rather than
being silently ignored, on the rule that a name the program cannot serve is worse than no
name at all.

One thing this bought for free: screen captures are now worth trusting. The resample
appears *only* on the composed path, and taking a capture forces composition — so under
glow every screenshot in this repository landed on the degraded path and looked soft, which
is indistinguishable from nothing being wrong. Under Vulkan the composed copy is the frame,
so the measurements above ("`#4d525c`", "both middles on 349.5") are measurements of what
the window actually shows.

## Size

```
11,547,648 bytes  ->  5,844,480 bytes      -49.4%
```

Nothing this program does was given up. Every number below came out of a linked
executable — a feature change or a profile flag is one build, so there was no reason to
estimate — and the method is the one in
[`binary-size-playbook.md`](../yet-another-file-explorer/binary-size-playbook.md), which
records the same audit on the file explorer.

| Change | Measured | |
| --- | ---: | --- |
| `eframe/wgpu` → `eframe/wgpu_no_default_features` | **−1,096,192** | D3D12, GLES and WebGL, plus naga's HLSL and GLSL writers, for backends this window never presents on |
| drop `eframe/default_fonts` | **−1,418,240** | four TrueType faces behind a font read that does not fail |
| drop `eframe/glow` | −313,856 | the OpenGL painter, `glow` and `glutin` |
| `lto = "fat"` + `codegen-units = 1` | −997,888 | release build 55s → 2m34s; `dev` untouched |
| `opt-level = "z"` on the run-once crates | −224,768 | naga, the unreachable clipboard codecs, the unreachable URL parser |
| `panic = "abort"` | **−1,652,224** | nothing in this process recovers from a panic |

Only one of those carried a risk worth measuring afterwards: naga is 173,056 bytes of the
`"z"` row *and* it runs at startup, translating egui's WGSL into SPIR-V while the window is
still hidden. Launch to a visible panel came out at **92 ms median against 122 ms** —
smaller and, if anything, quicker to open, because the smaller image pages in faster than
the tighter code translates two shaders. Without that A/B this would have been a guess
dressed as a decision.

Three of those are worth more than their bytes, because each is a thing you would not
find by reading the code:

- **The one-word manifest bug.** eframe's `wgpu` feature is defined as
  `["wgpu_no_default_features", "egui-wgpu/default"]`, and `egui-wgpu/default` contains
  `wgpu/default` — *every backend wgpu has*. `azur-egui-theme` pins exactly one backend
  per platform and documents that a consumer must take `wgpu_no_default_features`; this
  manifest took `wgpu`, which quietly turned the whole default back on. No care inside a
  dependency can prevent that. `cargo tree -e features -i wgpu` is the check, and it takes
  a minute: there should be no line reading `wgpu feature "dx12"`.
- **The fonts were the four largest objects in the executable**, at 418,804 + 361,676 +
  324,135 + 309,408 bytes of `.rdata`. `azur_egui_theme::fonts` installs Segoe UI and
  Consolas from disk and starts from `FontDefinitions::empty`, so epaint's four were a
  fallback for a font read that cannot fail on Windows. What it costs is emoji, which
  Segoe UI does not carry: a scan of every string literal in this program found exactly
  one character above U+2189, a `✕` in `--help` text that goes to a console. `.rdata` fell
  from 34.7% of the file to 18.3%.
- **`panic = "abort"` was available**, which is not usually true and is worth checking
  before wanting the number. Nothing in `src/` calls `catch_unwind`, nor does eframe,
  egui, epaint, winit, wgpu, wgpu-core or naga; `wgpu-hal`'s six sites all discard the
  result and wrap a `log!` in an FFI callback that a release build does not register.
  Neither of this program's two threads had a recovery path to lose. `.pdata` — one unwind
  entry per function — fell from 314,368 bytes to 81,408.

Two measurements went the other way and are recorded so they are not re-attempted:

| Attempted | Result |
| --- | --- |
| `opt-level = "z"` for the whole graph | **363,520 bytes *bigger*** than `"s"`, and slower with it. Under fat LTO, `"z"` withholds the inlining the link-time pass needs to prove a call dead. `"z"` survives only on the crates where it was measured to help. |
| removing the clipboard and URL stacks | Not possible from this manifest. `arboard` with an `image`/`png`/`bmp`/`tiff` decoder, and `url` + `idna`, arrive through egui-winit's `clipboard` and `links` features, which eframe hardcodes in its *dependency declaration* rather than putting behind a feature. About 180 KB this program cannot reach and cannot remove; `"z"` is the whole of the available answer. |

What is left is mostly not optional. Roughly: 1.5 MB of epaint's CPU rasterising and text
shaping (`vello_cpu`, `fearless_simd`, `harfrust`, `skrifa`, `read_fonts` — all
non-optional epaint 0.35 dependencies), 1.2 MB of wgpu and naga, 900 KB of std/core/alloc,
220 KB of egui, 105 KB of `.rsrc` for the icon Explorer shows, and 97 KB of this
program's own code. Past this point every byte costs a feature.

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

### No console, in any profile

`#![cfg_attr(not(test), windows_subsystem = "windows")]`. This used to be
`not(debug_assertions)` — a debug build kept a console, on the reasoning that panics and the
backend's diagnostics need somewhere to go — and what that cost was a black window opening
behind the panel on every debug launch, which is the one thing a resident widget must not do.

Two things make dropping it free. `system::attach_parent_console` calls
`AttachConsole(ATTACH_PARENT_PROCESS)`, so a launch from a shell or from `cargo run` *borrows*
that console and `--help`, a mistyped `--hotkey` and the `AZUR_GLOW` notice all still print —
while a launch from Explorer, a shortcut or the Startup folder finds no parent console, creates
nothing, and shows no window. And the guard is `not(test)` rather than nothing at all, because
the test harness is this same crate root: a `windows` subsystem test binary has no standard
handles, and std reports "no stdio handle" as a **successful** write of every byte, so
`cargo test` would print nothing and still exit 0.

### `cargo claude-*` and `--profile`, for an agent in the same checkout

An assistant working in this repository shares it with whoever is sitting at it, and two
things they would otherwise share are worth separating.

```
cargo claude-build            target/claude, not target/debug
cargo claude-test             the same, for the test binaries
cargo claude-clippy           -D warnings, so a warning cannot be scrolled past
cargo claude-doc              the intra-doc links this codebase leans on
cargo claude-run -- --show --profile claude
```

**`target/`**, because cargo keys its fingerprints on the profile: an agent's `cargo test` and
an editor's rust-analyzer take turns invalidating one another's artifacts, and on Windows they
take turns *locking* them — "did not finalize incremental compilation session directory: Accès
refusé" is that collision, and the work of that compilation is thrown away.
[`profile.claude`](Cargo.toml) inherits `dev` and changes nothing else, so what those aliases
build is the same code `cargo build` builds; all it buys is the directory.

**`%APPDATA%`**, because this program *writes* there. Launching it to check a change would
otherwise read and overwrite the settings and the favourites of the copy you actually use —
and a panel is checked by clicking things, which is exactly what writes. `--profile <name>`
moves both files into `%APPDATA%\Azur Audio Switcher\profiles\<name>`, nested under the
default folder rather than beside it so that no-profile resolves to exactly the path it always
has: a flag that exists to leave your settings alone must not move them to introduce itself.
The name is whitelisted to letters, digits, `-` and `_` — it is joined onto a path, so
`--profile ..\..\Startup` is a usage error rather than a directory, and
`a_profile_name_cannot_escape_the_folder_it_names` holds that.

The flag is deliberately **not** baked into the `claude-run` alias. It is the program's flag
rather than cargo's, and an alias that passed it silently is how a run you thought was
isolated writes somewhere you did not mean.

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
