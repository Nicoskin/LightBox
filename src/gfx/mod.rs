//! Direct3D 11 + DirectComposition rendering.
//!
//! [`Gpu`] (device, swap chain, pipeline state) needs no window and is built
//! on worker threads in parallel with window creation; the UI thread then
//! only binds its swap chain to the window through a [`Compositor`].

mod chrome;
mod compositor;
mod device;
mod inspector;
mod model;
mod overlay;
mod panel;
mod renderer;
mod texture;

pub use compositor::Compositor;
pub use device::{Adapter, Gpu};
pub use model::{GpuMesh, ModelDraw};
pub use overlay::{
    AdjustView, ButtonView, CaptionButtonView, CaptionView, CardView, CellView, CropView, DialView,
    HistogramView, Icon, InfoView, MenuItemView, MenuView, PanelView, SegmentView, SliderView,
    TextStyle, TextView, Thumb, Ui, VideoBarView,
};
pub use renderer::{Background, DrawParams, Filter, Layer, Part};
pub use texture::GpuImage;
