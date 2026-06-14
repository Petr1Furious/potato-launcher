use std::sync::Arc;

use gpui::RenderImage;
use image::{Frame, ImageBuffer, Luma, Rgba};
use qrcode::QrCode;
use smallvec::SmallVec;

pub const QR_DISPLAY_SIZE: u32 = 200;

pub fn qr_image_for_url(url: &str) -> Option<Arc<RenderImage>> {
    let code = QrCode::new(url.as_bytes()).ok()?;

    let image = code
        .render::<Luma<u8>>()
        .quiet_zone(false)
        .min_dimensions(QR_DISPLAY_SIZE, QR_DISPLAY_SIZE)
        .build();
    let (width, height) = image.dimensions();
    let mut rgba = ImageBuffer::new(width, height);
    for (x, y, pixel) in image.enumerate_pixels() {
        let value = pixel[0];
        rgba.put_pixel(x, y, Rgba([value, value, value, 255]));
    }
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(SmallVec::from_elem(
        Frame::new(rgba),
        1,
    ))))
}
