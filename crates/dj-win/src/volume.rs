//! Master volume of the default render endpoint (Core Audio), with change
//! notifications posted to the UI window.

use crate::window::{WM_DEFAULT_DEVICE_CHANGED, WM_VOLUME_CHANGED};
use std::ptr;
use windows::core::{implement, IUnknown, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, PROPERTYKEY, WPARAM};
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{
    eConsole, eMultimedia, eRender, EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient,
    IMMNotificationClient_Impl, MMDeviceEnumerator, AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// COM for this (UI) thread. Call once at startup.
pub fn init_com() {
    // SAFETY: plain initialization of the calling thread's COM apartment.
    // S_FALSE (already initialized) and RPC_E_CHANGED_MODE (another mode) are both fine.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
}

/// Posts `msg` to the window. Safe to call from any thread.
fn post(hwnd: isize, msg: u32) {
    // SAFETY: PostMessageW is thread-safe; a stale handle just makes the post fail.
    unsafe {
        let _ = PostMessageW(Some(HWND(hwnd as *mut _)), msg, WPARAM(0), LPARAM(0));
    }
}

/// Receives volume and mute changes on a COM thread. Only posts a message.
#[implement(IAudioEndpointVolumeCallback)]
struct VolumeCallback {
    hwnd: isize,
}

impl IAudioEndpointVolumeCallback_Impl for VolumeCallback_Impl {
    fn OnNotify(&self, _data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
        post(self.hwnd, WM_VOLUME_CHANGED);
        Ok(())
    }
}

/// Receives default-device changes on a COM thread. Only posts a message.
#[implement(IMMNotificationClient)]
struct DeviceCallback {
    hwnd: isize,
}

impl IMMNotificationClient_Impl for DeviceCallback_Impl {
    fn OnDeviceStateChanged(
        &self,
        _id: &PCWSTR,
        _state: DEVICE_STATE,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnDeviceAdded(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnDefaultDeviceChanged(
        &self,
        flow: EDataFlow,
        role: ERole,
        _id: &PCWSTR,
    ) -> windows::core::Result<()> {
        if flow == eRender && (role == eConsole || role == eMultimedia) {
            post(self.hwnd, WM_DEFAULT_DEVICE_CHANGED);
        }
        Ok(())
    }

    fn OnPropertyValueChanged(
        &self,
        _id: &PCWSTR,
        _key: &PROPERTYKEY,
    ) -> windows::core::Result<()> {
        Ok(())
    }
}

/// Clamps a level to 0..=1; NaN becomes 0.
fn clamp_level(level: f32) -> f32 {
    if level.is_nan() {
        0.0
    } else {
        level.clamp(0.0, 1.0)
    }
}

/// Master volume of the default render endpoint (console role), with change notifications.
pub struct Mixer {
    enumerator: IMMDeviceEnumerator,
    device_cb: IMMNotificationClient,
    volume_cb: IAudioEndpointVolumeCallback,
    endpoint: Option<IAudioEndpointVolume>,
}

impl Mixer {
    /// Acquires the default render endpoint and registers both callbacks.
    /// Notifications are posted to `hwnd`.
    pub fn new(hwnd: HWND) -> windows::core::Result<Self> {
        let hwnd = hwnd.0 as isize;
        // SAFETY: CLSID and null outer unknown are valid; the enumerator is released on drop.
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None::<&IUnknown>, CLSCTX_ALL)? };
        let device_cb: IMMNotificationClient = DeviceCallback { hwnd }.into();
        let volume_cb: IAudioEndpointVolumeCallback = VolumeCallback { hwnd }.into();
        // SAFETY: the callback object is kept alive by `device_cb` until unregistered in drop.
        unsafe { enumerator.RegisterEndpointNotificationCallback(&device_cb)? };
        // On error `mixer` is dropped here, which unregisters the device callback.
        let mut mixer = Self {
            enumerator,
            device_cb,
            volume_cb,
            endpoint: None,
        };
        mixer.endpoint = Some(mixer.attach()?);
        Ok(mixer)
    }

    /// Gets the default endpoint and registers the volume callback on it.
    fn attach(&self) -> windows::core::Result<IAudioEndpointVolume> {
        // SAFETY: COM calls on interfaces owned by this UI thread's apartment.
        unsafe {
            let device = self.enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let endpoint = device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)?;
            endpoint.RegisterControlChangeNotify(&self.volume_cb)?;
            Ok(endpoint)
        }
    }

    /// Unregisters the volume callback from the current endpoint, if any. Errors are ignored.
    fn detach(&mut self) {
        if let Some(endpoint) = self.endpoint.take() {
            // SAFETY: unregisters the same callback that `attach` registered.
            unsafe {
                let _ = endpoint.UnregisterControlChangeNotify(&self.volume_cb);
            }
        }
    }

    /// (level 0.0..=1.0, muted). None if the endpoint is gone.
    pub fn read(&self) -> Option<(f32, bool)> {
        let endpoint = self.endpoint.as_ref()?;
        // SAFETY: read-only COM calls on a live endpoint interface.
        unsafe {
            let level = endpoint.GetMasterVolumeLevelScalar().ok()?;
            let muted = endpoint.GetMute().ok()?.as_bool();
            Some((clamp_level(level), muted))
        }
    }

    /// Sets the master level (clamped to 0..=1). Synchronous.
    pub fn set_level(&self, level: f32) {
        if let Some(endpoint) = &self.endpoint {
            // SAFETY: a null event context is valid; errors (device gone) are ignored.
            unsafe {
                let _ = endpoint.SetMasterVolumeLevelScalar(clamp_level(level), ptr::null());
            }
        }
    }

    /// Windows' own volume step, with a null event context.
    pub fn step(&self, up: bool) {
        if let Some(endpoint) = &self.endpoint {
            // SAFETY: a null event context is valid; errors are ignored.
            unsafe {
                let _ = if up {
                    endpoint.VolumeStepUp(ptr::null())
                } else {
                    endpoint.VolumeStepDown(ptr::null())
                };
            }
        }
    }

    /// Flips the mute state. Does nothing if the endpoint is gone.
    pub fn toggle_mute(&self) {
        if let Some(endpoint) = &self.endpoint {
            // SAFETY: read then write of the mute flag on a live endpoint; errors are ignored.
            unsafe {
                if let Ok(muted) = endpoint.GetMute() {
                    let _ = endpoint.SetMute(!muted.as_bool(), ptr::null());
                }
            }
        }
    }

    /// Re-acquires the default endpoint after `WM_DEFAULT_DEVICE_CHANGED`.
    pub fn reacquire(&mut self) -> windows::core::Result<()> {
        self.detach();
        self.endpoint = Some(self.attach()?);
        Ok(())
    }
}

impl Drop for Mixer {
    fn drop(&mut self) {
        self.detach();
        // SAFETY: unregisters the device callback registered in `new`; errors are ignored.
        unsafe {
            let _ = self
                .enumerator
                .UnregisterEndpointNotificationCallback(&self.device_cb);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::clamp_level;

    #[test]
    fn clamp_level_stays_in_range() {
        assert_eq!(clamp_level(-0.5), 0.0);
        assert_eq!(clamp_level(0.25), 0.25);
        assert_eq!(clamp_level(1.5), 1.0);
        assert_eq!(clamp_level(f32::NAN), 0.0);
    }
}
