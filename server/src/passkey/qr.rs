#![allow(dead_code)]

use base64::{engine::general_purpose::STANDARD, Engine as _};
use qrcode::render::svg;
use qrcode::QrCode;

/// Generate a QR code as SVG string
pub fn generate_qr_code(data: &str, _size: u32) -> Result<String, String> {
    let code = QrCode::new(data).map_err(|e| format!("Failed to generate QR code: {}", e))?;

    let svg_string = code.render::<svg::Color>().min_dimensions(256, 256).build();

    Ok(svg_string)
}

pub fn generate_qr_code_simple(data: &str) -> Result<String, String> {
    let svg = generate_qr_code(data, 256)?;
    let base64 = STANDARD.encode(svg.as_bytes());
    Ok(format!("data:image/svg+xml;base64,{}", base64))
}

pub fn generate_qr_svg(data: &str) -> Result<String, String> {
    generate_qr_code(data, 256)
}
