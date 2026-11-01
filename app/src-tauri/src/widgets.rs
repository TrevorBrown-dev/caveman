//! Widgets: the see-through, frameless, always-on-top windows (the board cam, the speed meter, ...) that OBS captures
//! one by one. Each kind has one window, labelled `widget-<kind>`, whose page is `index.html?view=widget&kind=<kind>`
//! with the widget's options in the query (app/src/lib/widgets.ts is the registry; components/widgets/ the pages).
//! This module keeps what the back end needs of them: the labels, each window's saved place and size in the settings
//! (`plugins.widgets.<kind>.window`, logical pixels), where a new one goes, and the debounce for saving a window's
//! geometry as it is moved or resized.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::AppHandle;

/// Widget windows' labels start with this; the rest is the kind.
pub const PREFIX: &str = "widget-";

pub fn label(kind: &str) -> String {
    format!("{PREFIX}{kind}")
}

pub fn kind_of(label: &str) -> Option<&str> {
    label.strip_prefix(PREFIX)
}

/// A window's place and size in logical pixels (what the window builder takes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Geometry {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// The saved geometry of a widget's window, from the plugins' settings.
pub fn saved(plugins: &Value, kind: &str) -> Option<Geometry> {
    serde_json::from_value(plugins.get("widgets")?.get(kind)?.get("window")?.clone()).ok()
}

/// Puts a window's geometry into the plugins' settings, the rest untouched.
pub fn store(plugins: &mut Value, kind: &str, g: Geometry) {
    if !plugins.is_object() {
        *plugins = json!({});
    }
    let widgets = plugins.as_object_mut().unwrap().entry("widgets").or_insert_with(|| json!({}));
    if !widgets.is_object() {
        *widgets = json!({});
    }
    let widget = widgets.as_object_mut().unwrap().entry(kind).or_insert_with(|| json!({}));
    if !widget.is_object() {
        *widget = json!({});
    }
    widget.as_object_mut().unwrap().insert("window".to_string(), json!(g));
}

/// Carries the stored window geometries into settings that come without them: the front end leaves a widget's
/// `window` out when it saves (the back end keeps it), and sends `null` to reset it.
pub fn keep_geometry(into: &mut Value, stored: &Value) {
    let Some(kinds) = stored.get("widgets").and_then(Value::as_object) else { return };
    for (kind, w) in kinds {
        let Some(g) = w.get("window").and_then(|g| serde_json::from_value::<Geometry>(g.clone()).ok()) else { continue };
        let has_key = into.get("widgets").and_then(|ws| ws.get(kind)).and_then(|w| w.get("window")).is_some();
        if !has_key {
            store(into, kind, g);
        }
    }
}

/// Where a widget window goes: its saved place while that is still on a screen, else a spot near the top right
/// corner of the primary monitor, each kind a step further down (two new widgets then do not cover each other).
pub fn place(app: &AppHandle, saved: Option<Geometry>, default_w: u32, default_h: u32, index: usize) -> Geometry {
    if let Some(g) = saved.filter(|g| g.w >= 80 && g.h >= 40) {
        let (cx, cy) = (g.x as f64 + g.w as f64 / 2.0, g.y as f64 + g.h as f64 / 2.0);
        let on_a_screen = app.available_monitors().unwrap_or_default().iter().any(|m| {
            let s = m.scale_factor();
            let (mx, my) = (m.position().x as f64 / s, m.position().y as f64 / s);
            let (mw, mh) = (m.size().width as f64 / s, m.size().height as f64 / s);
            cx >= mx && cx <= mx + mw && cy >= my && cy <= my + mh
        });
        if on_a_screen {
            return g;
        }
    }
    let step = index as f64 * 40.0;
    let (x, y) = match app.primary_monitor() {
        Ok(Some(m)) => {
            let s = m.scale_factor();
            let (mx, my) = (m.position().x as f64 / s, m.position().y as f64 / s);
            ((mx + m.size().width as f64 / s - default_w as f64 - 24.0).max(mx), my + 24.0 + step)
        }
        _ => (40.0 + step, 40.0 + step),
    };
    Geometry { x: x.round() as i32, y: y.round() as i32, w: default_w, h: default_h }
}

/// The geometry changes waiting to be saved: a window is saved once it has held still for a moment, not at every
/// pixel of a drag.
#[derive(Default)]
pub struct WidgetState {
    pending: Mutex<HashMap<String, (Geometry, Instant)>>,
}

impl WidgetState {
    /// Notes a window's new geometry; returns the moment, for `settled`.
    pub fn note(&self, label: &str, g: Geometry) -> Instant {
        let now = Instant::now();
        self.pending.lock().unwrap().insert(label.to_string(), (g, now));
        now
    }

    /// The geometry noted at `at`, taken out, when nothing newer came for that window since.
    pub fn settled(&self, label: &str, at: Instant) -> Option<Geometry> {
        let mut pending = self.pending.lock().unwrap();
        match pending.get(label) {
            Some((g, t)) if *t == at => {
                let g = *g;
                pending.remove(label);
                Some(g)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_round_trip() {
        assert_eq!(label("speed"), "widget-speed");
        assert_eq!(kind_of("widget-boardcam"), Some("boardcam"));
        assert_eq!(kind_of("main"), None);
    }

    #[test]
    fn geometry_is_stored_beside_the_other_settings() {
        let mut plugins = json!({"obs": {"autostart": true}, "widgets": {"speed": {"autostart": false, "options": {"units": "mph"}}}});
        let g = Geometry { x: 10, y: -20, w: 300, h: 120 };
        store(&mut plugins, "speed", g);
        assert_eq!(saved(&plugins, "speed"), Some(g));
        assert_eq!(plugins["widgets"]["speed"]["options"]["units"], "mph");
        assert_eq!(plugins["obs"]["autostart"], true);
        assert_eq!(saved(&plugins, "boardcam"), None);
        // from nothing
        let mut empty = Value::Null;
        store(&mut empty, "boardcam", g);
        assert_eq!(saved(&empty, "boardcam"), Some(g));
    }

    #[test]
    fn stored_geometry_survives_a_save_without_one() {
        let g = Geometry { x: 1, y: 2, w: 300, h: 100 };
        let mut stored = json!({});
        store(&mut stored, "speed", g);
        // left out: kept
        let mut incoming = json!({"widgets": {"speed": {"autostart": true, "options": {}}}});
        keep_geometry(&mut incoming, &stored);
        assert_eq!(saved(&incoming, "speed"), Some(g));
        assert_eq!(incoming["widgets"]["speed"]["autostart"], true);
        // null: a reset, left as is
        let mut reset = json!({"widgets": {"speed": {"window": null}}});
        keep_geometry(&mut reset, &stored);
        assert_eq!(saved(&reset, "speed"), None);
        assert!(reset["widgets"]["speed"]["window"].is_null());
        // no widgets at all: made
        let mut none = json!({"obs": {}});
        keep_geometry(&mut none, &stored);
        assert_eq!(saved(&none, "speed"), Some(g));
    }

    #[test]
    fn settled_only_for_the_last_note() {
        let s = WidgetState::default();
        let a = s.note("widget-speed", Geometry { x: 0, y: 0, w: 100, h: 50 });
        let b = s.note("widget-speed", Geometry { x: 5, y: 0, w: 100, h: 50 });
        assert_eq!(s.settled("widget-speed", a), None, "a newer note came");
        assert_eq!(s.settled("widget-speed", b).map(|g| g.x), Some(5));
        assert_eq!(s.settled("widget-speed", b), None, "taken out");
    }
}
