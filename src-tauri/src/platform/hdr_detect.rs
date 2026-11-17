//! What the OS says about HDR on the display the main window is on: whether HDR is switched on,
//! the display's peak luminance and the level SDR white is shown at. All values are in nits.
//!
//! * Windows: DXGI (`IDXGIOutput6::GetDesc1`) for the colour space and peak luminance, and the
//!   DisplayConfig API for the "SDR content brightness" slider.
//! * Linux: no portable API exists; KDE Plasma is read through `kscreen-doctor`. Elsewhere nothing is
//!   detected and the user switches HDR on (and sets the nits) in the settings.

use crate::render::hdr::DetectedHdr;

#[cfg(target_os = "windows")]
pub fn detect(window: &tauri::WebviewWindow) -> Option<DetectedHdr> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020;
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput6};
    use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromWindow};
    use windows::core::Interface;

    let raw = window.hwnd().ok()?.0 as isize;
    let hwnd = HWND(raw as *mut core::ffi::c_void);
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.ok()?;

    let mut adapter_index = 0;
    while let Ok(adapter) = unsafe { factory.EnumAdapters1(adapter_index) } {
        adapter_index += 1;
        let mut output_index = 0;
        while let Ok(output) = unsafe { adapter.EnumOutputs(output_index) } {
            output_index += 1;
            let Ok(output6) = output.cast::<IDXGIOutput6>() else {
                continue;
            };
            let Ok(desc) = (unsafe { output6.GetDesc1() }) else {
                continue;
            };
            if desc.Monitor != monitor {
                continue;
            }
            let hdr_on = desc.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020;
            let peak = (desc.MaxLuminance.is_finite() && desc.MaxLuminance > 0.0)
                .then_some(desc.MaxLuminance);
            let sdr_white = if hdr_on {
                sdr_white_nits(&desc.DeviceName)
            } else {
                None
            };
            return Some(DetectedHdr {
                hdr_on,
                peak_nits: peak,
                sdr_white_nits: sdr_white,
            });
        }
    }
    None
}

/// The "SDR content brightness" of the display with this GDI name, in nits.
#[cfg(target_os = "windows")]
fn sdr_white_nits(device_name: &[u16; 32]) -> Option<f32> {
    use windows::Win32::Devices::Display::{
        DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
        DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SDR_WHITE_LEVEL,
        DISPLAYCONFIG_SOURCE_DEVICE_NAME, DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes,
        QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
    };

    unsafe {
        let mut path_count = 0u32;
        let mut mode_count = 0u32;
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count).0
            != 0
        {
            return None;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut path_count,
            paths.as_mut_ptr(),
            &mut mode_count,
            modes.as_mut_ptr(),
            None,
        )
        .0 != 0
        {
            return None;
        }

        for path in paths.iter().take(path_count as usize) {
            let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            source.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            source.header.size = size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
            source.header.adapterId = path.sourceInfo.adapterId;
            source.header.id = path.sourceInfo.id;
            if DisplayConfigGetDeviceInfo(&mut source.header) != 0
                || source.viewGdiDeviceName != *device_name
            {
                continue;
            }

            let mut white = DISPLAYCONFIG_SDR_WHITE_LEVEL::default();
            white.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL;
            white.header.size = size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32;
            white.header.adapterId = path.targetInfo.adapterId;
            white.header.id = path.targetInfo.id;
            if DisplayConfigGetDeviceInfo(&mut white.header) == 0 && white.SDRWhiteLevel > 0 {
                // 1000 corresponds to 80 nits
                return Some(white.SDRWhiteLevel as f32 / 1000.0 * 80.0);
            }
            return None;
        }
    }
    None
}

#[cfg(target_os = "linux")]
pub fn detect(_window: &tauri::WebviewWindow) -> Option<DetectedHdr> {
    detect_kscreen().or_else(detect_gnome)
}

/// GNOME 48+: Mutter reports each monitor's `color-mode` (0 = default, 1 = BT.2100 / HDR) on D-Bus.
/// It does not report luminance, so the nits come from the defaults or the settings.
#[cfg(target_os = "linux")]
fn detect_gnome() -> Option<DetectedHdr> {
    let output = std::process::Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.gnome.Mutter.DisplayConfig",
            "--object-path",
            "/org/gnome/Mutter/DisplayConfig",
            "--method",
            "org.gnome.Mutter.DisplayConfig.GetCurrentState",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(parse_gnome(&String::from_utf8_lossy(&output.stdout)))
}

#[cfg(any(target_os = "linux", test))]
fn parse_gnome(state: &str) -> DetectedHdr {
    DetectedHdr {
        hdr_on: state.contains("'color-mode': <uint32 1>"),
        peak_nits: None,
        sdr_white_nits: None,
    }
}

/// KDE Plasma 6: `kscreen-doctor -j` lists every output with its HDR state and brightness.
#[cfg(target_os = "linux")]
fn detect_kscreen() -> Option<DetectedHdr> {
    let output = std::process::Command::new("kscreen-doctor")
        .arg("-j")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    parse_kscreen(&json)
}

#[cfg(any(target_os = "linux", test))]
fn parse_kscreen(json: &serde_json::Value) -> Option<DetectedHdr> {
    let outputs = json.get("outputs")?.as_array()?;
    let number = |o: &serde_json::Value, keys: &[&str]| {
        keys.iter()
            .find_map(|k| o.get(*k).and_then(|v| v.as_f64()))
            .map(|v| v as f32)
    };
    outputs
        .iter()
        .filter(|o| o.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true))
        .find(|o| o.get("hdr").and_then(|v| v.as_bool()).unwrap_or(false))
        .map(|o| DetectedHdr {
            hdr_on: true,
            peak_nits: number(o, &["maxPeakBrightness", "peakBrightness", "maxPeakBrightnessOverride"])
                .filter(|v| *v > 0.0),
            sdr_white_nits: number(o, &["sdrBrightness", "sdr-brightness"]).filter(|v| *v > 0.0),
        })
        .or(Some(DetectedHdr::default()))
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn detect(_window: &tauri::WebviewWindow) -> Option<DetectedHdr> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kscreen_hdr_output_is_read() {
        let json = serde_json::json!({"outputs": [
            {"enabled": true, "hdr": false},
            {"enabled": true, "hdr": true, "sdrBrightness": 300, "maxPeakBrightness": 1000}
        ]});
        let d = parse_kscreen(&json).unwrap();
        assert!(d.hdr_on);
        assert_eq!(d.peak_nits, Some(1000.0));
        assert_eq!(d.sdr_white_nits, Some(300.0));
    }

    #[test]
    fn gnome_bt2100_monitor_is_hdr() {
        let hdr = "(1, [(('DP-1', 'X', 'Y', 'Z'), [('3840x2160@60', 3840, 2160, 60.0, 1.0, [1.0], {})], {'color-mode': <uint32 1>, 'supported-color-modes': <[uint32 0, 1]>})], [], {})";
        let sdr = "(1, [(('DP-1', 'X', 'Y', 'Z'), [], {'color-mode': <uint32 0>, 'supported-color-modes': <[uint32 0, 1]>})], [], {})";
        assert!(parse_gnome(hdr).hdr_on);
        assert!(!parse_gnome(sdr).hdr_on);
    }

    #[test]
    fn kscreen_without_hdr_is_sdr() {
        let json = serde_json::json!({"outputs": [{"enabled": true, "hdr": false}]});
        assert!(!parse_kscreen(&json).unwrap().hdr_on);
    }
}
