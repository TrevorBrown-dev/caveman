# Widgets

Widgets are the see-through, frameless, always-on-top windows Caveman puts over the game for a stream: the board cam (the game's own board turning as it does under the skater) and the speed meter, with more to come. Each is a window of its own, so OBS captures each as its own window source and places them in the scene independently. Nothing is drawn in the game; what they show is read from outside it (the pose stream of `caveman-trainer pose`), so they need no FX DLL. Added 2026-11-01, when the board cam window (`board-cam.md`) became the first widget and the speed meter the second.

## What a widget is

- **A kind in the registry**, `app/src/lib/widgets.ts` (`WIDGETS`): its name, the window's title (what OBS lists; it never changes once a scene refers to it), a blurb, the window's size when none is saved, what it needs (`pose`), its options' defaults, the hint shown in the window for its first seconds, and two components loaded on demand: the widget itself and its settings pane.
- **A component** in `app/src/components/widgets/`, which gets `options` (flat values: strings, numbers, booleans) and `onstatus` (a line for the window's corner while it has nothing to show, null when all is well), and may give the frame menu items of its own (`onmenu`). It fills the window; the frame around it is `WidgetFrame.svelte`.
- **A settings pane**, also in `components/widgets/`, which gets the options and an `onchange(patch)`. The pane sits on the Plugins page (`Widgets.svelte`), a change is saved at once and sent to the open window, which applies it live (`widget-options-set`).
- **Saved settings** under `plugins.widgets.<kind>` in the app's config: `autostart` (open it with the game), `window` (its last place and size, logical pixels), `options`.

## The frame and the windows

`WidgetFrame.svelte` is the page of every widget window (`index.html?view=widget&kind=<kind>&<options>`; `main.ts` routes it, and the old `?view=boardcam` still opens the board cam). It draws the strip at the top that moves the window (Tauri's drag region), the right-click menu (show the window's bounds while placing it, the widget's items, Close), Escape, the hint and the status line. The windows resize by their edges. A widget with no clicks of its own can make its whole body the drag region as the speed meter does (`data-tauri-drag-region` on the panel, `pointer-events: none` on its children, since Tauri starts the drag only when the element under the pointer carries the attribute); the board cam cannot, because a drag over the board turns the view.

The back end (`app/src-tauri/src/widgets.rs`, the commands in `main.rs`) makes a window per kind, labelled `widget-<kind>`, with the flags that make it an overlay (no decorations, transparent, always on top, no taskbar entry, no shadow), and:

- opens it where it was last: it saves a window's place and size into the settings once the window has held still for 600 ms after a move or resize (`WidgetState`, the debounce), and a saved place off every screen falls back to the default spot near the top right of the primary monitor, each kind a step lower than the one before. The front end never sends a geometry back (it leaves `window` out when it saves; `null` resets it), so a save from the app cannot undo a move;
- tells the app when a widget window opens or closes (`widget-state`), so the cards show a dot and the pane's buttons know;
- closes every widget window with the main one (on their own they would keep the app alive, drawn over whatever is on the screen).

The capability `default` covers `main` and `widget-*`.

## Telemetry: one stream for all

`app/src/lib/telemetry.svelte.ts` is what a widget reads. A widget calls `start()` on mount and `stop()` when it goes; the first start in a window subscribes to the `board-pose` events and asks the back end for the stream, the last stop lets go. The back end counts the windows reading (`live.rs` `pose_users`, by window label; a window's end releases it too) and keeps one trainer process for all of them, stopped with the last. When the stream ends with its game, telemetry asks again three seconds later as long as a game runs, so a new game session comes through without reopening anything.

Besides the raw frames (`onFrame`, for the board cam's bones), telemetry keeps reactive state: `live` (frames come and the game has a board), `status` (why not), `t`, and the **speed**: the distance the board's position moved over the last 0.12 s divided by that time, eased by a third per frame, with `speedMax` since the start or the last reset. A jump of more than 60 m/s between two frames is a respawn or the menu, not skating, and clears the samples. The game's world unit is the metre, so km/h is 3.6 times the value and mph 2.237 times; the first ride with the meter open will say whether that holds (a reading in the thousands would mean centimetres).

## The two widgets

- **Board cam** (`BoardCamWidget.svelte`, `BoardCamSettings.svelte`): the replica from `board-cam.md`, unchanged in what it draws. Its viewpoint (elevation, azimuth, zoom), how the board turns (with the camera or the world), mirror and nose are its options; a drag or the wheel in the window changes the viewpoint and reports it back (`widget-options`), which the app saves without sending it back to the window. The window's size replaced the old Size slider. The picture on the board and the OBS link's video are still read by the widget itself.
- **Speed** (`SpeedWidget.svelte`, `SpeedSettings.svelte`): the number, its unit, a caption, and a bar under it showing the current speed against the session's top speed with that top speed as text. Options: units (km/h, mph, m/s), decimals, caption on or off, top speed and bar on or off, a dark panel behind it or the bare number. It scales with the window (container query units), and its menu has "Reset top speed".

## Adding a widget

1. A component in `components/widgets/` taking `WidgetProps`; read `telemetry` (or `onFrame`) and call `start()`/`stop()` if it needs the pose.
2. A settings component taking `SettingsProps`.
3. An entry in `WIDGETS` with a kind, a window title that will never change, defaults for every option (the defaults also type the options coming back from the query), and the two imports.

Nothing else: the Plugins page lists it, autostart and the window's place work, the frame gives it the menu and the hint.

## Open

- Not yet ridden with: the speed's units (metres assumed) and smoothing against real skating; the top-speed bar's feel.
- Next metrics the same stream gives cheaply: distance this session, air time (the board's height and the contact states the FX DLL tracks), grind time. Trick names would come from the scoring system (`knowledge/scoring.md`), which nothing reads yet.
- A widget needing the DLL (the bars' levels, say) would add `dll` to `needs` and a status line of its own; the frame does not know the DLL.
