use base64::{Engine as _, engine::general_purpose::STANDARD};
use qrcode::QrCode;

pub fn generate_qr_code(data: &str, size: u32) -> Result<Vec<u8>, String> {
    let code = QrCode::new(data)
        .map_err(|e| format!("Failed to generate QR code: {}", e))?;
    
    // Use the render method to get a Vec<u8> (PNG format)
    let image = code
        .render()
        .min_dimensions(size as u32, size as u32)
        .build();
    
    Ok(image)
}

pub fn generate_qr_code_simple(data: &str) -> Result<String, String> {
    let bytes = generate_qr_code(data, 256)?;
    let base64 = STANDARD.encode(bytes);
    Ok(format!("data:image/png;base64,{}", base64))
}

pub fn generate_qr_svg(data: &str) -> Result<String, String> {
    let code = QrCode::new(data)
        .map_err(|e| format!("Failed to generate QR code: {}", e))?;
    
    let svg = code
        .render()
        .min_dimensions(256, 256)
        .build();
    
    Ok(svg)
}
