# CLAUDE.md

Operating notes for an agent working in this repository. Facts and rules, each with the
shortest reason that keeps it from being "fixed". The full reasoning lives in the `//!`
module headers and in the comments in `Cargo.toml` — read a module's header before changing it.

`azur-audio-switcher` ("Azur Audio Switcher", repo `azur-sound-mixer`) — a tray-resident
Windows audio panel in Rust on egui 0.35. Windows only: the backend is Core Audio and
per-application volume has no equivalent elsewhere. Depends on `azur-egui-theme` by **path**
(`../azur-egui-theme`), the shared Azur design system; its `desktop` layer is installed with
one call, `desktop::install(ctx, true)`.

---

## Build, run, deploy

**Always build into `target/claude`, via the aliases in `.cargo/config.toml`:**

```powershell
cargo claude-build            # -> target/claude/azur-audio-switcher.exe
cargo claude-test
cargo claude-clippy           # -D warnings, on purpose
cargo claude-doc              # keep it clean: intra-doc links are used heavily
cargo claude-run -- --show --profile claude
```

- `target/debug` and `target/release` belong to the user. Sharing them makes the agent and
  rust-analyzer invalidate and lock each other's artifacts. `[profile.claude]` inherits `dev`
  and changes nothing else.
- **Always pass `--profile claude` when launching the app.** It writes
  `%APPDATA%\Azur Audio Switcher\{settings,favourites}.txt` on every click; `--profile <name>`
  moves both into `...\profiles\<name>`. The flag is deliberately *not* baked into the
  `claude-run` alias — it is the program's flag, not cargo's.
- **Launch cargo from PowerShell, not Git Bash** (Git Bash bakes a lower-case `d:` into
  `CARGO_MANIFEST_DIR`).
- **Do not run bare `cargo fmt`.** It reformats `build.rs`, `src/mark.rs` and
  `src/audio/policy.rs`, which are not touched by most tasks; revert whatever the task did
  not touch.
- `cargo run --release` is the build to judge anything by. `cargo deploy` = release build +
  copy to `D:\Programs` (or `$AZUR_DEPLOY_DIR`, or a path argument), via
  `examples/deploy.rs` → `azur_egui_theme::deploy`. It can replace a running copy (rename
  aside), but the panel already on screen keeps the old build and the hotkey until quit from
  the tray.
- `cargo tree -d -p egui` must show one egui: two versions are two `Context` types and the
  theme silently styles neither.

---

## Architecture

| Layer | Module | Knows about |
| --- | --- | --- |
| Backend | [`audio`](src/audio/) | Core Audio, COM, processes. Nothing about egui. |
| Resident | [`resident`](src/resident.rs) | Tray icon, hotkey, the panel's `HWND`. Nothing about what the panel contains. |
| Widget | [`app`](src/app.rs) | Snapshots and commands. Nothing about COM. |
| Glyphs | [`icons`](src/icons.rs), [`widgets`](src/widgets.rs), [`mark`](src/mark.rs) | Azur roles and an `egui::Painter`; the mark is bitmaps unpacked at build time. |

Also: [`settings`](src/settings.rs) (file values; `app` holds the run's values — two copies
on purpose), [`favourites`](src/favourites.rs), [`hotkey`](src/hotkey.rs),
[`system`](src/system.rs) (Win32 window tweaks, console attach).

**Three threads.** The audio backend runs in its own MTA apartment, publishes immutable
`Arc<Snapshot>`s and takes `Command`s over a channel — the paint loop must never wait on the
audio service. The resident thread has its own hidden top-level window and `GetMessage` loop
and shows/hides the panel itself with `ShowWindow`, because **a hidden egui window paints no
frames**, so nothing inside egui can react to a hotkey.

Backend clocks: poll 50 ms (volumes, mutes, peaks off bound interfaces), session rescan
600 ms, device rescan 1500 ms. Invalidation is by identity (endpoint id, session instance
id), not time. `Bound` holds all interfaces of one endpoint so a rebind is one assignment.
The backend sleeps while the panel is hidden (`audio::Wake`, a condvar), and waking resets
all three clocks.

---

## Do not "fix" these

Each looks like a bug or an oversight and is a measured decision:

- **`IPolicyConfig` ([`audio::policy`](src/audio/policy.rs)) is undocumented.** The vtable
  order is the contract — the unused methods before `SetDefaultEndpoint` must stay declared.
  The IID is the `…41cf…` revision; `…455c…` (the one most write-ups quote) returns
  `E_NOINTERFACE` on Windows 11 26200. A failed `CoCreateInstance` disables device switching
  only, shown in a `MessageBar` **with the `HRESULT`**.
- **Two roles, disjoint.** `Role::Default` writes `eConsole` + `eMultimedia`;
  `Role::Communications` writes `eCommunications` only. Neither writes the other's.
  `Default` is read back off `eConsole` alone.
- **The window is parked 30,000 px off-desktop at startup, not hidden** — eframe's
  `EpiIntegration::post_rendering` forces `set_visible(true)` after the first frame.
  See `main::PARKED`.
- **`WS_EX_TOOLWINDOW` is re-asserted every frame from `App::logic`** — winit reverts it,
  and `with_taskbar(false)` leaves the window in Alt-Tab. See `system::keep_out_of_alt_tab`.
  Both this and the parking rely on eframe 0.35 internals; if eframe changes, the symptom is
  the panel appearing at startup or showing in Alt-Tab.
- **The resident's hidden window is top-level, not `HWND_MESSAGE`** — `TaskbarCreated` is a
  broadcast, and without it an Explorer restart loses the tray icon.
- **Quit goes through `WM_CLOSE` on the panel**, so the exit is winit/eframe's own path.
  Dismissing (`Esc`, ✕, focus loss) is hiding.
- **Auto-hide guards:** the panel must have held the focus since *this* appearance (a show
  counter), only OS focus counts (egui popups are not a blur), and `Esc` is consumed after
  the content so a `Select` popup gets it first.
- **Popups close on the focus-loss *edge* and on re-show** (`App::close_stale_popups`,
  `popups_are_stale`), not level-triggered — the first appearance often does not get the
  foreground, and a level-triggered close made every combobox unopenable. The focus is
  also re-requested on every fresh appearance and when a drag starts.
- **`Echo`** is the one piece of local UI state: a value just set is trusted over the
  snapshot for 400 ms *or until the backend agrees*, whichever is first.
- **DWM owns the outline and the corner radius.** The panel paints a square, unstroked rect
  and sets `DWMWA_BORDER_COLOR` (`COLORREF` is `0x00bbggrr`). Painting our own border gave a
  double one.
- **Resizing is implemented by hand** (eight strips → `ViewportCommand::BeginResize`),
  edges registered before corners, both before content. The bottom margin is `space-4` so
  the grip has its own strip. Drag bands in the chrome row are registered after the buttons
  and clipped to `GRIP` from the edge.
- **Renderer is wgpu/Vulkan, never glow.** DWM resamples GL/D3D12 surfaces on NVIDIA and
  destroys 1 px lines. `AZUR_GLOW` is gone and only prints a notice; `AZUR_BACKEND` /
  `AZUR_ADAPTER` belong to the theme crate.
- **Binary size choices in `Cargo.toml`:** `eframe/wgpu_no_default_features` (plain `wgpu`
  turns every backend back on — check `cargo tree -e features -i wgpu` has no `dx12`), no
  `default_fonts`, no `glow`, fat LTO + 1 CGU, `opt-level = "z"` only on the listed run-once
  crates (global `"z"` was measured *bigger*), `panic = "abort"` (nothing catches unwinds).
- **`#![cfg_attr(not(test), windows_subsystem = "windows")]`** — no console in any build;
  `system::attach_parent_console` borrows the shell's. `not(test)` because a windows-subsystem
  test binary silently "succeeds" at writing nothing.
- **Item spacing is zero; every gap is an explicit `add_space`.** Mixer row is 36 px (20 px
  identity line + `Size::Small` slider), below `row::DENSE` on purpose. Masters keep
  `Size::Medium` sliders and the device selects stay `Size::Medium`.
- **Text sits on one ink baseline per row** (`widgets::galley_on_row`, from
  `components::ink_baseline`), from the row's principal font.
- **Peak meter is `text-tertiary`, not the accent**, booked with `reserve_meter` before the
  slider so the handle paints over it. A starred glyph is `accent.mark`, not
  `accent.default` (2.75:1 as ink — fails).
- **The mark's faders are `#1E7CFB`, not a token**, and the mark is accepted at below
  `contrast::TEXT` (3.9:1 worst); tests in `widgets.rs` / `icons.rs` pin it from both sides.
  `icons::sliders` (unknown-app fallback) stays horizontal so it is never confused with the
  mark.
- **The icon pipeline:** `assets/app-*.png` (ten sizes, cut by hand with
  [`assets/icons.py`](assets/icons.py), never part of the build) are the source of truth.
  `build.rs` assembles `assets/app.ico` from them verbatim and decodes the eight ≤64 px
  sizes into `$OUT_DIR` for `mark` to `include_bytes!` — no PNG decoder ships.
  `mark::bitmap` rounds *up* to the nearest size it has.
- Toasts are titled with the exe name; registering an AppUserModelID would mean writing to
  the user's registry, which the program deliberately does not do.

---

## Conventions

- **Every decision carries its reasoning** in a doc comment: what was measured, what the
  alternative was, what broke. Match that voice. When behaviour changes, grep for the
  comments (and README feature lines) that argued for the old behaviour and fix them in the
  same pass.
- Colours come from `Theme` roles only; anything about a colour is claimed in measured
  contrast (`azur_egui_theme::contrast`), not in token steps.
- [README.md](README.md) is end-user documentation: summary, screenshot, download link,
  features, command line. Keep design notes out of it. `docs/overview.png` is its
  screenshot — the user's to regenerate; capture to the scratchpad to check a visual change.
  Screen captures are faithful under Vulkan.
