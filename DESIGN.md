---
version: alpha
name: "RiceExplorer"
description: "A native file explorer whose interface shifts with the user's desktop rice while file operations stay consistent."
colors:
  primary: "#1766a6"
  background: "#e5edf5"
  chrome: "#c9dfed"
  panel: "#ffffff"
  sidebar: "#f3f8fc"
  text: "#202e3a"
  muted: "#607283"
  selection: "#c6e6fb"
  border: "#9fb8cd"
  folder: "#e8b94e"
  file: "#6ba7d2"
typography:
  sans:
    fontFamily: "system-ui, sans-serif"
  mono:
    fontFamily: "ui-monospace, monospace"
rounded:
  DEFAULT: "3px"
  square: "0px"
  modern: "12px"
spacing:
  list-row: "28px"
  sidebar-width: "210px"
components:
  toolbar: {}
  tab: {}
  file-item: {}
  sidebar: {}
  modal: {}
---

# RiceExplorer design system

## Overview

RiceExplorer is a workbench for files, not a dashboard. It should feel like the exact operating-system era the user chooses—without abandoning recognizable navigation or file safety. The signature is a **composable explorer shell**: classic, Luna, Aero, Ribbon, Aqua, ChromeOS and Finder identities can select and arrange their UI components while sharing the same file operations and navigation state. Its restrained file grid/list remains the stable center.

Audience: people using RiceBowl on Linux X11/Wayland, including older ThinkPads and small desktops. Product register, English UI in v0.1, WCAG 2.2 AA as the target. Do not use decorative gradients, large marketing cards or gratuitous animation.

**Token ownership:** The TOML files under `themes/` are runtime-canonical values. The frontmatter documents the default `win7` mapping and shape/density range; `src/theme.rs` parses colors, metrics, schema version and shell composition. Changes to `win7.toml` must update this frontmatter. A custom file changes runtime tokens without changing the maintained default.

## Colors

The default Windows 7 profile is a cool mist-blue frame around a neutral reading surface. Its muted ink keeps metadata quiet, while the selection blue remains visibly distinct. Each profile owns all eleven color roles in its TOML, never hardcoded per screen. Destructive confirmations use a shared danger treatment; the theme's accent never silently changes their meaning.

## Typography

The renderer uses egui's bundled system-like proportional font, with a technical monospace fallback where available. Filenames are left-aligned in list view and centered, truncated with a visible ellipsis in the icon grid. Paths remain literal and are not used for shell interpolation. Large interactive titles are restrained to the folder name.

## Layout

- Classic: menu bar and familiar tool row, squared or gently beveled UI, narrower dense rows.
- Aero: layered navigation and separate command bar, deliberate cool surface contrast.
- Modern: tab-first condensed navigation, roomy rows and rounded surfaces.
- The optional `[shell]` TOML table selects a shell identity and independently
  composes `top` and `bottom` regions from `menu-bar`, `tabs`, `navigation`
  and `command-bar` components. No component may render in both regions.
  Legacy themes without it use the established layout-family defaults.
- `[shell.sidebar]` selects left/right placement, width, independently
  expandable (`collapsible`) or flat section presentation, and ordered visible
  `tasks`, `places`, `favorites` and `volumes` sections. The XP Luna task shelf
  provides real file actions vertically; it is not a cosmetic duplicate.
- `[shell.preview]` selects the side and width of the on-demand preview pane.
  `[shell].status_bar` is optional; transfer/recovery warnings are not.
- Frutiger Aero intentionally places its command strip below the file area,
  while Windows 10 keeps its command strip in the upper ribbon position.
- Renderers select shared UI actions; filesystem mutations, open tabs,
  selection and running jobs never belong to a theme-specific implementation.
- Arbitrary multi-column docking, a visual composer and user-authored Rust
  components still need modular adapters. Regions currently accept only the
  four maintained chrome components; the file grid always remains central.
- Main directory area uses virtualized rows; width determines icon-grid column count.
- Sidebar can be collapsed and resized independently; it does not obscure the file list.
- Layout responds to small windows by shrinking search/path fields before losing controls.

## Elevation & Depth

Primary hierarchy comes from contrast between shell, sidebar and content—not drop shadows. Windows 98 uses strong borders and square corners. Windows 7 uses translucent-color *impression* without expensive runtime blur. ChromeOS uses bright paper and softer radius. Never blur filenames or metadata.

## Shapes

All major corners use the selected `metrics.rounding` value. Built-in radii span 0px (Windows 98) to 12px (ChromeOS), with dense list rows from 27px to 37px. File icons are painted vector primitives and take theme folder/file colors; the application logo is a scalable SVG with an abstract opened folder and blue explorer stripe.

## Components

Buttons have focus, hover, active and disabled states driven by the shared egui widget style; controls with no available action are disabled. Search includes a clear affordance. File rows show selected/hover feedback and tooltip paths. A confirmation window precedes trashing; failure details remain in the bottom status bar. Folder loading uses a spinner and empty/no-match/error messages. Keyboard and pointer act on the same operation methods. Filesystem logic never refers to visual themes.

The folder-tree disclosure buttons use universal `+`/`-` glyphs and the
existing Expand/Collapse tooltips with a 24×24 logical-point target, not
font-dependent Unicode triangles;
Linux fallback fonts must never make a navigation control appear blank.
Sort headers name their active direction with ASCII `asc` / `desc` text;
the bundled font must not turn an arrow into a missing-glyph box.

## Do's and Don'ts

- Do keep each rice's own color, density, structure and folder icon language.
- Do keep copy/move/Trash behavior identical across themes.
- Do let the desktop window manager handle native decorations and the RiceBowl session lifecycle.
- Don't silently overwrite files or permanently delete from the file list.
- Don't imitate proprietary OS icons by shipping extracted resources.
- Don't introduce an idle indexing daemon until measured usage requires one.
