use image::{ImageBuffer, Rgb};
use ratatui::style::Color;

pub type RgbImage = ImageBuffer<Rgb<u8>, Vec<u8>>;

pub fn buffer_to_rgb_image(buffer: &ratatui::buffer::Buffer) -> RgbImage {
    let width = buffer.area.width;
    let height = buffer.area.height;
    let mut image = ImageBuffer::new(width as u32, height as u32);
    for y in 0..height {
        for x in 0..width {
            let cell = &buffer[(x, y)];
            let bg = color_to_rgb(cell.bg);
            let fg = color_to_rgb(cell.fg);
            let rgb = if cell.symbol() == " " { bg } else { fg };
            image.put_pixel(x as u32, y as u32, Rgb(rgb));
        }
    }
    image
}

fn color_to_rgb(color: Color) -> [u8; 3] {
    match color {
        Color::Reset => [15, 15, 15],
        Color::Black => [0, 0, 0],
        Color::Red => [205, 49, 49],
        Color::Green => [13, 188, 121],
        Color::Yellow => [229, 229, 16],
        Color::Blue => [36, 114, 200],
        Color::Magenta => [188, 63, 188],
        Color::Cyan => [17, 168, 205],
        Color::Gray => [128, 128, 128],
        Color::DarkGray => [80, 80, 80],
        Color::LightRed => [241, 76, 76],
        Color::LightGreen => [35, 209, 139],
        Color::LightYellow => [245, 245, 67],
        Color::LightBlue => [59, 142, 234],
        Color::LightMagenta => [214, 112, 214],
        Color::LightCyan => [41, 184, 219],
        Color::White => [229, 229, 229],
        Color::Rgb(r, g, b) => [r, g, b],
        Color::Indexed(index) => indexed_color(index),
    }
}

fn indexed_color(index: u8) -> [u8; 3] {
    const PALETTE: [[u8; 3]; 16] = [
        [0, 0, 0],
        [128, 0, 0],
        [0, 128, 0],
        [128, 128, 0],
        [0, 0, 128],
        [128, 0, 128],
        [0, 128, 128],
        [192, 192, 192],
        [128, 128, 128],
        [255, 0, 0],
        [0, 255, 0],
        [255, 255, 0],
        [0, 0, 255],
        [255, 0, 255],
        [0, 255, 255],
        [255, 255, 255],
    ];
    if index < 16 {
        PALETTE[index as usize]
    } else {
        [15, 15, 15]
    }
}
