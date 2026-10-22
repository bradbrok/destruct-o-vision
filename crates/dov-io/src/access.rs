//! Capture permissions. macOS asks the user once per app, and the answer
//! sticks until they change it in System Settings.

#[derive(Clone, Copy, Debug)]
pub(crate) enum Device {
    Camera,
    Microphone,
}

/// Whether access has been granted, without asking.
#[cfg(target_os = "macos")]
pub(crate) fn granted(device: Device) -> bool {
    use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice};
    let Some(media) = media_type(device) else {
        return false;
    };
    // SAFETY: a plain class-method call with a valid media type.
    unsafe { AVCaptureDevice::authorizationStatusForMediaType(media) == AVAuthorizationStatus::Authorized }
}

/// Asks for access if the user hasn't been asked yet. Blocks the calling
/// thread (never the engine's) until they answer.
#[cfg(target_os = "macos")]
pub(crate) fn authorized(device: Device) -> bool {
    use std::sync::mpsc;
    use std::time::Duration;

    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice};

    let Some(media) = media_type(device) else {
        return false;
    };
    // SAFETY: plain class-method calls with a valid media type.
    match unsafe { AVCaptureDevice::authorizationStatusForMediaType(media) } {
        AVAuthorizationStatus::Authorized => true,
        AVAuthorizationStatus::NotDetermined => {
            let (tx, rx) = mpsc::channel();
            let block = RcBlock::new(move |granted: Bool| {
                let _ = tx.send(granted.as_bool());
            });
            // SAFETY: the block is retained by AVFoundation until it runs.
            unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(media, &block) };
            rx.recv_timeout(Duration::from_secs(300)).unwrap_or(false)
        }
        _ => false,
    }
}

#[cfg(target_os = "macos")]
fn media_type(device: Device) -> Option<&'static objc2_av_foundation::AVMediaType> {
    // SAFETY: framework constants.
    unsafe {
        match device {
            Device::Camera => objc2_av_foundation::AVMediaTypeVideo,
            Device::Microphone => objc2_av_foundation::AVMediaTypeAudio,
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn granted(_device: Device) -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn authorized(_device: Device) -> bool {
    true
}
