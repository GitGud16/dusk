# Dusk — Theme

Dark only. Minimal and flat. One brand palette, used in a fixed order of importance.

## Logo

A dusk horizon: the four brand colors stacked as horizontal bands, violet on top, then magenta, pink, and a thin gold stripe at the bottom. Flat shapes, no gradient anywhere, including the splash screen and hero images. Fits in a rounded square for the app icon and reduces cleanly to 16px because it has no fine detail. The wordmark is "Dusk" in Inter semibold, lowercase or sentence case, in `text-1`.

Geometry (M6): a square plate with corners rounded to a fifth of its side; the bands take 9, 5, 4 and 2 twentieths of its height, top to bottom (`primary`, `secondary`, `alert`, `signal`). In the icon each band ends on a whole pixel, so a 16px icon has bands of 7, 4, 3 and 2 pixels, and sizes from 32px leave a sixteenth of the side clear around the plate. `scripts/make-logo.py` draws all of it from these numbers: `dusk-app/assets/dusk.ico` (16 to 256px, built into `dusk.exe`), `dusk-app/assets/dusk.svg` (the windows' icon and the toolbar's mark, 16px beside the wordmark) and `docs/images/logo.png`. Change the numbers there and run it again; never edit the files by hand.

## Brand colors (in order of importance)

| # | Role | Hex | HSL | Used for |
|---|------|-----|-----|----------|
| 1 | **Primary** (violet) | `#5003C0` | `hsl(264, 97%, 38%)` | Primary buttons, selection, video clips, progress bars, active sliders |
| 2 | **Secondary** (magenta) | `#AB03A9` | `hsl(301, 97%, 34%)` | Audio clips, secondary highlights |
| 3 | **Alert** (pink) | `#FF467A` | `hsl(343, 100%, 64%)` | Playhead, destructive actions, errors, recording / live indicators |
| 4 | **Signal** (yellow) | `#FFD51E` | `hsl(49, 100%, 56%)` | Markers, in/out points, keyframes, warnings, snap guides |

### Derived tints

The primary violet is dark (38% lightness), so on a dark background it only reads clearly as a *fill*. For thin or small elements (text, icons, 1px borders, focus rings) use the bright tint.

| Token | HSL | Hex | Used for |
|-------|-----|-----|----------|
| `primary` | `hsl(264, 97%, 38%)` | `#5003C0` | Fills |
| `primary-hover` | `hsl(264, 97%, 46%)` | `#5F04E7` | Hover on primary fills |
| `primary-bright` | `hsl(264, 97%, 70%)` | `#A468FD` | Links, active icons, focus ring, selected-clip border, active tab underline |
| `primary-dim` | `hsl(264, 50%, 24%)` | `#371F5C` | Selected row / selected media item background |
| `secondary` | `hsl(301, 97%, 34%)` | `#AB03A9` | Audio clip fill |
| `secondary-bright` | `hsl(301, 90%, 62%)` | `#F547F2` | Waveform lines on audio clips, small audio accents |

Text on **primary** and **secondary** fills is white `#FFFFFF`. Text on **alert** pink and **signal** yellow fills is `bg-0` (white on pink is only 3.3:1; `bg-0` on pink is 5.9:1).

## Neutrals (violet-tinted grays)

| Token | HSL | Hex | Used for |
|-------|-----|-----|----------|
| `bg-0` | `hsl(255, 15%, 6%)` | `#0E0D12` | App background, timeline background, video preview surround |
| `bg-1` | `hsl(255, 14%, 9%)` | `#15141A` | Panels, toolbar, timeline tracks area |
| `bg-2` | `hsl(255, 13%, 12%)` | `#1D1B23` | Elevated: menus, inputs, pop-out clip editor chrome |
| `bg-3` | `hsl(255, 12%, 16%)` | `#26242E` | Hover state on rows and controls |
| `border` | `hsl(255, 12%, 20%)` | `#302D39` | All 1px borders and dividers |
| `text-1` | `hsl(258, 10%, 92%)` | `#EAE9ED` | Primary text (contrast 16:1 on `bg-0`) |
| `text-2` | `hsl(258, 8%, 65%)` | `#A39FAD` | Secondary text, labels (7:1) |
| `text-3` | `hsl(258, 7%, 45%)` | `#706B7B` | Muted: disabled, placeholders, ruler ticks (3.7:1, never for important text) |

Never use pure black or pure white for surfaces or body text.

## Flat rules

- No gradients anywhere, UI or logo.
- No drop shadows. Elevation is shown by stepping the background (`bg-1` → `bg-2`) plus a 1px `border`.
- Borders are 1px. The only 2px strokes are the selected-clip outline and the keyboard focus ring.
- Corner radius: 4px on controls (buttons, inputs, tabs), 6px on panels / popovers / the pop-out window, 3px on timeline clips.
- Icons: outline style, 16px, single color (`text-2` default, `primary-bright` when active).
- One primary-filled button per view. Everything else is outlined (`border`) or ghost.

## Typography

- UI font: **Inter** 4.1, bundled as the static regular, medium and semibold files (OFL 1.1, 1.25 MB, in `dusk-app/ui/fonts/` with their source and checksums). Fallback: Segoe UI, system sans.
- Numeric font: **JetBrains Mono** 2.304 regular, bundled (OFL 1.1, 274 KB), for timecodes, durations and numeric readouts. Slint cannot switch on Inter's tabular figures, so a monospaced font keeps digits from shifting width.
- Weights: 400 regular, 500 medium, 600 semibold. Nothing heavier.
- Timecodes and numbers: JetBrains Mono at the same pixel size as the surrounding text.
- Sizes (dense editor scale):
  - 11px: ruler labels, clip names, tiny labels
  - 12px: body text, lists, menu items, inputs
  - 13px: panel titles, buttons
  - 15px: dialog titles
  - 20px: large headings (welcome screen, export summary)
- Sentence case everywhere. No ALL CAPS, no Title Case.

## Spacing and sizing

- 4px grid: 4 / 8 / 12 / 16 / 24.
- Control height: 28px. Toolbar: 36px.
- Panel padding: 12px.
- Timeline: ruler 24px, video track 56px, audio track 36px, clip gap 2px.
- Playhead: 2px `alert` line with a small triangle head.
- Markers: 10px `signal` diamond on the ruler.

## Timeline color rules

- Video clip: `primary` fill, white 11px label. Selected: 2px `primary-bright` border.
- Audio clip: `secondary` fill, waveform drawn in `secondary-bright`.
- Playhead: `alert`. In/out points and markers: `signal`.
- Snap guide lines: `signal` at 50% opacity.
- Disabled / muted clip: same fill at 40% opacity with a `text-3` label.

## States

- Hover: background steps to `bg-3` (ghost) or fill steps to `primary-hover` (primary).
- Focus (keyboard): 2px `primary-bright` ring, no glow.
- Destructive buttons: outlined in `alert` with `alert` text, filled `alert` with `bg-0` text only on the final confirm.
- Errors: `alert` text on `bg-2`. Warnings: `signal` text on `bg-2`.
- Status lines (on `bg-1`, at the foot of each window) say what just happened in `text-2`, what to notice about it (an edit that did more than was asked) in `signal`, and what went wrong in `alert`.
