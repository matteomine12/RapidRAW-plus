//! HDR display state and nit handling.
//!
//! Model (same as Lightroom / LightCraft):
//! * SDR white is 1.0 in the linear HDR render. Everything above it is HDR headroom.
//! * The display's headroom is `peak_nits / sdr_white_nits`, measured in stops (EV) above SDR white.
//! * Peak and SDR white nits are read from the OS by default and can be overridden in the settings.
//! * The swapchain is scRGB (linear `Rgba16Float`, 1.0 = 80 nits), so a value of 1.0 in the HDR
//!   render is shown at `sdr_white_nits` by multiplying with `sdr_white_nits / 80`.

use crate::app::settings::AppSettings;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub const SCRGB_REFERENCE_NITS: f32 = 80.0;
pub const DEFAULT_PEAK_NITS: f32 = 1000.0;
pub const DEFAULT_SDR_WHITE_NITS: f32 = 203.0;
pub const MIN_PEAK_NITS: f32 = 100.0;
pub const MAX_PEAK_NITS: f32 = 10000.0;
pub const MIN_SDR_WHITE_NITS: f32 = 80.0;
pub const MAX_SDR_WHITE_NITS: f32 = 1000.0;

/// Default HDR edit headroom in stops above SDR white (LightCraft uses 3, the limit is 5).
pub const DEFAULT_EDIT_MAX_EV: f64 = 3.0;
pub const EDIT_MAX_EV_LIMIT: f64 = 5.0;

/// What the operating system reports about the display the window is on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedHdr {
    /// HDR (advanced colour) is switched on for the display.
    pub hdr_on: bool,
    pub peak_nits: Option<f32>,
    pub sdr_white_nits: Option<f32>,
}

static ACTIVE: AtomicBool = AtomicBool::new(false);
static SURFACE_CAPABLE: AtomicBool = AtomicBool::new(false);
static PEAK_NITS: AtomicU32 = AtomicU32::new(0);
static SDR_WHITE_NITS: AtomicU32 = AtomicU32::new(0);

fn load(a: &AtomicU32, default: f32) -> f32 {
    let v = f32::from_bits(a.load(Ordering::Relaxed));
    if v.is_finite() && v > 0.0 { v } else { default }
}

/// True when the display is rendering in HDR (float surface and HDR enabled on the monitor or forced).
pub fn display_active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

pub fn peak_nits() -> f32 {
    load(&PEAK_NITS, DEFAULT_PEAK_NITS)
}

pub fn sdr_white_nits() -> f32 {
    load(&SDR_WHITE_NITS, DEFAULT_SDR_WHITE_NITS)
}

/// Peak over SDR white as a linear factor, at least 1.
pub fn headroom() -> f32 {
    (peak_nits() / sdr_white_nits()).max(1.0)
}

pub fn headroom_ev() -> f32 {
    headroom().log2()
}

/// Factor that turns "1.0 = SDR white" into scRGB (1.0 = 80 nits).
pub fn scrgb_scale() -> f32 {
    sdr_white_nits() / SCRGB_REFERENCE_NITS
}

/// Records whether the window surface can present `Rgba16Float` (scRGB) at all.
pub fn set_surface_capable(capable: bool) {
    SURFACE_CAPABLE.store(capable, Ordering::Release);
}

pub fn surface_capable() -> bool {
    SURFACE_CAPABLE.load(Ordering::Acquire)
}

/// The values to use, from the settings and what the OS reports.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Resolved {
    pub active: bool,
    pub peak_nits: f32,
    pub sdr_white_nits: f32,
}

pub fn resolve(settings: &AppSettings, detected: Option<DetectedHdr>, surface_capable: bool) -> Resolved {
    let mode = settings.hdr_display_mode.as_deref().unwrap_or("auto");
    let os_hdr = detected.is_some_and(|d| d.hdr_on);
    let active = match mode {
        "off" => false,
        "on" => surface_capable,
        _ => surface_capable && os_hdr,
    };

    let clean = |v: Option<f32>, min: f32, max: f32| v.filter(|v| v.is_finite()).map(|v| v.clamp(min, max));
    let sdr_white_nits = clean(settings.hdr_sdr_white_nits, MIN_SDR_WHITE_NITS, MAX_SDR_WHITE_NITS)
        .or_else(|| clean(detected.and_then(|d| d.sdr_white_nits), MIN_SDR_WHITE_NITS, MAX_SDR_WHITE_NITS))
        .unwrap_or(DEFAULT_SDR_WHITE_NITS);
    let peak_nits = clean(settings.hdr_peak_nits, MIN_PEAK_NITS, MAX_PEAK_NITS)
        .or_else(|| clean(detected.and_then(|d| d.peak_nits), MIN_PEAK_NITS, MAX_PEAK_NITS))
        .unwrap_or(DEFAULT_PEAK_NITS)
        .max(sdr_white_nits);

    Resolved {
        active,
        peak_nits,
        sdr_white_nits,
    }
}

pub fn apply(resolved: Resolved) {
    PEAK_NITS.store(resolved.peak_nits.to_bits(), Ordering::Relaxed);
    SDR_WHITE_NITS.store(resolved.sdr_white_nits.to_bits(), Ordering::Relaxed);
    ACTIVE.store(resolved.active, Ordering::Release);
}

static DETECTED: std::sync::Mutex<Option<DetectedHdr>> = std::sync::Mutex::new(None);

fn last_detected() -> Option<DetectedHdr> {
    *DETECTED.lock().unwrap_or_else(|e| e.into_inner())
}

/// Reads the settings and the OS state again and updates the live values.
#[cfg(not(target_os = "android"))]
pub fn refresh(app_handle: &tauri::AppHandle) -> HdrDisplayInfo {
    use tauri::Manager;
    let settings = crate::app::settings::load_settings(app_handle.clone()).unwrap_or_default();
    let detected = app_handle
        .get_webview_window("main")
        .and_then(|window| crate::platform::hdr_detect::detect(&window));
    *DETECTED.lock().unwrap_or_else(|e| e.into_inner()) = detected;
    apply(resolve(&settings, detected, surface_capable()));
    info(detected)
}

#[cfg(target_os = "android")]
pub fn refresh(_app_handle: &tauri::AppHandle) -> HdrDisplayInfo {
    info(None)
}

/// [`refresh`] with the app handle registered at startup (used while the GPU context is being built).
pub fn refresh_global() -> Option<HdrDisplayInfo> {
    crate::render::gpu::context::app_handle().map(refresh)
}

/// Re-reads everything and moves the window surfaces between SDR and HDR output when that changed.
/// Returns true when a surface changed (the image has to be rendered again).
pub fn reconcile(app_handle: &tauri::AppHandle) -> bool {
    use tauri::Manager;
    let was_active = display_active();
    let (was_peak, was_white) = (peak_nits(), sdr_white_nits());
    refresh(app_handle);
    let want = display_active();
    let state = app_handle.state::<crate::app::state::AppState>();
    let Some(context) = crate::render::gpu::context::current_gpu_context(&state) else {
        return false;
    };
    let mut changed = false;
    for display in [&context.display, &context.viewer_display] {
        let mut lock = display.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(display) = lock.as_mut() {
            changed |= display.set_hdr_output(&context.device, &context.adapter, want);
        }
    }
    changed
        || was_active != want
        || (want && (was_peak != peak_nits() || was_white != sdr_white_nits()))
}

/// [`reconcile`] on a worker thread, at most once a second; tells the frontend when the output changed.
pub fn reconcile_in_background(app_handle: &tauri::AppHandle) {
    use std::sync::atomic::AtomicBool;
    use tauri::Emitter;
    static BUSY: AtomicBool = AtomicBool::new(false);
    if display_info_unavailable() || BUSY.swap(true, Ordering::AcqRel) {
        return;
    }
    let app_handle = app_handle.clone();
    std::thread::spawn(move || {
        if reconcile(&app_handle) {
            let _ = app_handle.emit("hdr-display-changed", info(last_detected()));
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
        BUSY.store(false, Ordering::Release);
    });
}

/// Nothing to follow until the GPU surface has been created.
fn display_info_unavailable() -> bool {
    !surface_capable()
}

#[tauri::command]
pub fn get_hdr_display_info(app_handle: tauri::AppHandle) -> HdrDisplayInfo {
    refresh(&app_handle);
    info(last_detected())
}

/// Applies the HDR display settings that were just saved. Emits `hdr-display-changed` so the editor renders again.
#[tauri::command]
pub fn apply_hdr_display_settings(app_handle: tauri::AppHandle) -> HdrDisplayInfo {
    use tauri::Emitter;
    reconcile(&app_handle);
    let info = info(last_detected());
    let _ = app_handle.emit("hdr-display-changed", &info);
    info
}

/// What the settings screen shows.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HdrDisplayInfo {
    /// The window surface can present HDR (float scRGB) on this system.
    pub supported: bool,
    pub detected: Option<DetectedHdr>,
    pub active: bool,
    pub peak_nits: f32,
    pub sdr_white_nits: f32,
    pub headroom_ev: f32,
}

pub fn info(detected: Option<DetectedHdr>) -> HdrDisplayInfo {
    HdrDisplayInfo {
        supported: surface_capable(),
        detected,
        active: display_active(),
        peak_nits: peak_nits(),
        sdr_white_nits: sdr_white_nits(),
        headroom_ev: headroom_ev(),
    }
}

/// Scene-linear window in which the HDR lift happens, in stops, for the shader.
///
/// `peak` is linear (SDR white = 1). Below `ev0` the HDR render equals the SDR one; the lift
/// reaches `peak` at `ev1`. The AgX tonemapper has a long shoulder, so its lift starts earlier.
pub fn lift_window(peak: f32, agx: bool) -> (f32, f32) {
    let white_ev = if agx { 2.53_f32 } else { 0.0 };
    let ev0 = white_ev - if agx { 1.5 } else { 0.0 };
    let ev1 = white_ev + peak.max(1.0).log2() + if agx { 1.0 } else { 0.75 };
    (ev0, ev1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> AppSettings {
        AppSettings::default()
    }

    #[test]
    fn auto_follows_the_os() {
        let s = settings();
        let hdr = DetectedHdr {
            hdr_on: true,
            peak_nits: Some(1400.0),
            sdr_white_nits: Some(240.0),
        };
        let r = resolve(&s, Some(hdr), true);
        assert!(r.active);
        assert_eq!((r.peak_nits, r.sdr_white_nits), (1400.0, 240.0));
        assert!(!resolve(&s, Some(DetectedHdr::default()), true).active);
        assert!(!resolve(&s, None, true).active);
        assert!(!resolve(&s, Some(hdr), false).active);
    }

    #[test]
    fn settings_override_the_os_values() {
        let mut s = settings();
        s.hdr_peak_nits = Some(600.0);
        s.hdr_sdr_white_nits = Some(150.0);
        let hdr = DetectedHdr {
            hdr_on: true,
            peak_nits: Some(1400.0),
            sdr_white_nits: Some(240.0),
        };
        let r = resolve(&s, Some(hdr), true);
        assert_eq!((r.peak_nits, r.sdr_white_nits), (600.0, 150.0));
    }

    #[test]
    fn forced_on_needs_only_the_surface() {
        let mut s = settings();
        s.hdr_display_mode = Some("on".into());
        let r = resolve(&s, None, true);
        assert!(r.active);
        assert_eq!((r.peak_nits, r.sdr_white_nits), (DEFAULT_PEAK_NITS, DEFAULT_SDR_WHITE_NITS));
        s.hdr_display_mode = Some("off".into());
        assert!(!resolve(&s, None, true).active);
    }

    #[test]
    fn peak_never_below_sdr_white() {
        let mut s = settings();
        s.hdr_peak_nits = Some(100.0);
        s.hdr_sdr_white_nits = Some(400.0);
        let r = resolve(&s, None, true);
        assert!(r.peak_nits >= r.sdr_white_nits);
    }

    #[test]
    fn lift_window_is_ordered() {
        for agx in [false, true] {
            for peak in [1.0, 2.0, 8.0, 32.0] {
                let (a, b) = lift_window(peak, agx);
                assert!(b > a);
            }
        }
    }
}
