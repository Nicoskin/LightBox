//! Direct3D 11 + DirectComposition rendering.
//!
//! [`Gpu`] (device, swap chain, pipeline state) needs no window and is built
//! on worker threads in parallel with window creation; the UI thread then
//! only binds its swap chain to the window through a [`Compositor`].

mod compositor;
mod device;
mod overlay;
mod panel;
mod renderer;
mod texture;

pub use compositor::Compositor;
pub use device::{Adapter, Gpu};
pub use overlay::{ButtonView, CellView, Icon, PanelView, TextStyle, TextView, Thumb, Ui};
pub use renderer::{Background, DrawParams, Filter, Layer};
pub use texture::GpuImage;
