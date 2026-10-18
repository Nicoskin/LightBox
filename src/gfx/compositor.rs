use anyhow::Result;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice2, IDCompositionDesktopDevice, IDCompositionTarget,
    IDCompositionVisual2,
};
use windows::Win32::Graphics::Dxgi::IDXGISwapChain1;
use windows::core::IUnknown;

/// Binds a composition swap chain to a window via DirectComposition, giving
/// a per-pixel transparent window (premultiplied alpha) over the desktop.
///
/// The DComp device is created without a rendering device: swap-chain
/// content does not need one, and it lets the content switch between a WARP
/// and a hardware swap chain at any time.
pub struct Compositor {
    device: IDCompositionDesktopDevice,
    _target: IDCompositionTarget,
    visual: IDCompositionVisual2,
}

impl Compositor {
    pub fn new(hwnd: HWND) -> Result<Self> {
        // SAFETY: hwnd is a live window owned by this thread; the window has
        // WS_EX_NOREDIRECTIONBITMAP so DComp content is all that is shown.
        unsafe {
            let device: IDCompositionDesktopDevice = DCompositionCreateDevice2(None::<&IUnknown>)?;
            let target = device.CreateTargetForHwnd(hwnd, true)?;
            let visual = device.CreateVisual()?;
            target.SetRoot(&visual)?;
            Ok(Self {
                device,
                _target: target,
                visual,
            })
        }
    }

    /// Shows `swap_chain` in the window (takes effect on [`Self::commit`]).
    pub fn set_content(&self, swap_chain: &IDXGISwapChain1) -> Result<()> {
        // SAFETY: valid visual and swap chain.
        unsafe { self.visual.SetContent(swap_chain)? };
        Ok(())
    }

    pub fn commit(&self) -> Result<()> {
        // SAFETY: valid device.
        unsafe { self.device.Commit()? };
        Ok(())
    }

    /// Blocks until DWM has consumed the last commit (used for timing only).
    pub fn wait(&self) -> Result<()> {
        // SAFETY: valid device.
        unsafe { self.device.WaitForCommitCompletion()? };
        Ok(())
    }
}
