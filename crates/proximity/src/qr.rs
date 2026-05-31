use crate::error::{ProximityError, Result};
use image::{DynamicImage, Luma};
use qrcode::QrCode;

/// QR code service for generating and handling wallet address QR codes
pub struct QrCodeService;

impl QrCodeService {
    /// Generate a QR code for a wallet address
    /// 
    /// Returns the QR code as a PNG image in a DynamicImage format
    pub fn generate_qr_code(wallet_address: &str) -> Result<DynamicImage> {
        // Validate wallet address format (Solana addresses are base58 encoded, 32-44 chars)
        if wallet_address.is_empty() {
            return Err(ProximityError::InvalidInput(
                "Wallet address cannot be empty".to_string(),
            ));
        }

        // Validate base58 format
        if let Err(e) = bs58::decode(wallet_address).into_vec() {
            return Err(ProximityError::InvalidInput(format!(
                "Invalid wallet address format: {}",
                e
            )));
        }

        // Generate QR code
        let qr = QrCode::new(wallet_address.as_bytes()).map_err(|e| {
            ProximityError::QrCodeError(format!("Failed to generate QR code: {}", e))
        })?;

        // Convert to image
        let image = qr.render::<Luma<u8>>().build();
        
        Ok(DynamicImage::ImageLuma8(image))
    }

    /// Generate a QR code as a data URL (base64 encoded PNG)
    /// 
    /// This is useful for web applications that need to display QR codes inline
    pub fn generate_qr_code_data_url(wallet_address: &str) -> Result<String> {
        let image = Self::generate_qr_code(wallet_address)?;
        
        // Convert to PNG bytes
        let mut png_bytes = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            )
            .map_err(|e| {
                ProximityError::QrCodeError(format!("Failed to encode PNG: {}", e))
            })?;

        // Encode as base64 data URL
        let base64_data = base64_encode(&png_bytes);
        Ok(format!("data:image/png;base64,{}", base64_data))
    }

    /// Generate a QR code as raw PNG bytes
    /// 
    /// This is useful for saving to files or sending over network
    pub fn generate_qr_code_png(wallet_address: &str) -> Result<Vec<u8>> {
        let image = Self::generate_qr_code(wallet_address)?;
        
        let mut png_bytes = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            )
            .map_err(|e| {
                ProximityError::QrCodeError(format!("Failed to encode PNG: {}", e))
            })?;

        Ok(png_bytes)
    }

    /// Scan a QR code from image data and extract the wallet address
    /// 
    /// This method takes raw image data and attempts to decode a QR code from it.
    /// Platform-specific camera integration should capture the image and pass it here.
    pub fn scan_qr_code(image: &DynamicImage) -> Result<String> {
        // Convert to grayscale for QR code detection
        let gray_image = image.to_luma8();
        
        // Prepare image for rqrr
        let mut img = rqrr::PreparedImage::prepare(gray_image);
        
        // Find and decode QR codes
        let grids = img.detect_grids();
        
        if grids.is_empty() {
            return Err(ProximityError::QrCodeError(
                "No QR code found in image".to_string(),
            ));
        }

        // Decode the first QR code found
        let (_, content) = grids[0]
            .decode()
            .map_err(|e| ProximityError::QrCodeError(format!("Failed to decode QR code: {:?}", e)))?;

        // content is already a String from rqrr
        let wallet_address = content;

        // Validate the extracted wallet address
        Self::validate_wallet_address(&wallet_address)?;

        Ok(wallet_address)
    }

    /// Scan a QR code from PNG image bytes
    /// 
    /// Convenience method for scanning from PNG data
    pub fn scan_qr_code_from_png(png_bytes: &[u8]) -> Result<String> {
        let image = image::load_from_memory_with_format(png_bytes, image::ImageFormat::Png)
            .map_err(|e| ProximityError::QrCodeError(format!("Failed to load PNG: {}", e)))?;

        Self::scan_qr_code(&image)
    }

    /// Validate a wallet address format
    /// 
    /// Ensures the address is valid base58 and within expected length range
    pub fn validate_wallet_address(address: &str) -> Result<()> {
        if address.is_empty() {
            return Err(ProximityError::InvalidInput(
                "Wallet address cannot be empty".to_string(),
            ));
        }

        // Validate base58 format
        let decoded = bs58::decode(address).into_vec().map_err(|e| {
            ProximityError::InvalidInput(format!("Invalid wallet address format: {}", e))
        })?;

        // Solana public keys are 32 bytes
        if decoded.len() != 32 {
            return Err(ProximityError::InvalidInput(format!(
                "Invalid wallet address length: expected 32 bytes, got {}",
                decoded.len()
            )));
        }

        Ok(())
    }
}

/// Simple base64 encoding helper
fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    
    for chunk in data.chunks(3) {
        let mut buf = [0u8; 3];
        for (i, &byte) in chunk.iter().enumerate() {
            buf[i] = byte;
        }
        
        let b1 = (buf[0] >> 2) as usize;
        let b2 = (((buf[0] & 0x03) << 4) | (buf[1] >> 4)) as usize;
        let b3 = (((buf[1] & 0x0f) << 2) | (buf[2] >> 6)) as usize;
        let b4 = (buf[2] & 0x3f) as usize;
        
        result.push(CHARS[b1] as char);
        result.push(CHARS[b2] as char);
        result.push(if chunk.len() > 1 { CHARS[b3] as char } else { '=' });
        result.push(if chunk.len() > 2 { CHARS[b4] as char } else { '=' });
    }
    
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_qr_code_valid_address() {
        let wallet_address = "DYw8jCTfwHNRJhhmFcbXvVDTqWMEVFBX6ZKUmG5CNSKK";
        let result = QrCodeService::generate_qr_code(wallet_address);
        assert!(result.is_ok());
    }

    #[test]
    fn test_generate_qr_code_empty_address() {
        let result = QrCodeService::generate_qr_code("");
        assert!(result.is_err());
        match result {
            Err(ProximityError::InvalidInput(msg)) => {
                assert!(msg.contains("cannot be empty"));
            }
            _ => panic!("Expected InvalidInput error"),
        }
    }

    #[test]
    fn test_generate_qr_code_invalid_base58() {
        let result = QrCodeService::generate_qr_code("invalid0OIl");
        assert!(result.is_err());
        match result {
            Err(ProximityError::InvalidInput(msg)) => {
                assert!(msg.contains("Invalid wallet address format"));
            }
            _ => panic!("Expected InvalidInput error"),
        }
    }

    #[test]
    fn test_generate_qr_code_data_url() {
        let wallet_address = "DYw8jCTfwHNRJhhmFcbXvVDTqWMEVFBX6ZKUmG5CNSKK";
        let result = QrCodeService::generate_qr_code_data_url(wallet_address);
        assert!(result.is_ok());
        let data_url = result.unwrap();
        assert!(data_url.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn test_generate_qr_code_png() {
        let wallet_address = "DYw8jCTfwHNRJhhmFcbXvVDTqWMEVFBX6ZKUmG5CNSKK";
        let result = QrCodeService::generate_qr_code_png(wallet_address);
        assert!(result.is_ok());
        let png_bytes = result.unwrap();
        assert!(!png_bytes.is_empty());
        // PNG files start with specific magic bytes
        assert_eq!(&png_bytes[0..4], &[0x89, 0x50, 0x4E, 0x47]);
    }

    #[test]
    fn test_scan_qr_code_round_trip() {
        // Generate a QR code
        let wallet_address = "DYw8jCTfwHNRJhhmFcbXvVDTqWMEVFBX6ZKUmG5CNSKK";
        let image = QrCodeService::generate_qr_code(wallet_address).unwrap();

        // Scan it back
        let scanned_address = QrCodeService::scan_qr_code(&image).unwrap();

        // Should match original
        assert_eq!(wallet_address, scanned_address);
    }

    #[test]
    fn test_scan_qr_code_from_png_round_trip() {
        // Generate a QR code as PNG
        let wallet_address = "DYw8jCTfwHNRJhhmFcbXvVDTqWMEVFBX6ZKUmG5CNSKK";
        let png_bytes = QrCodeService::generate_qr_code_png(wallet_address).unwrap();

        // Scan it back from PNG bytes
        let scanned_address = QrCodeService::scan_qr_code_from_png(&png_bytes).unwrap();

        // Should match original
        assert_eq!(wallet_address, scanned_address);
    }

    #[test]
    fn test_scan_qr_code_no_qr_in_image() {
        // Create a blank image with no QR code
        let blank_image = DynamicImage::new_luma8(100, 100);
        let result = QrCodeService::scan_qr_code(&blank_image);

        assert!(result.is_err());
        match result {
            Err(ProximityError::QrCodeError(msg)) => {
                assert!(msg.contains("No QR code found"));
            }
            _ => panic!("Expected QrCodeError"),
        }
    }

    #[test]
    fn test_validate_wallet_address_valid() {
        let address = "DYw8jCTfwHNRJhhmFcbXvVDTqWMEVFBX6ZKUmG5CNSKK";
        let result = QrCodeService::validate_wallet_address(address);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_wallet_address_empty() {
        let result = QrCodeService::validate_wallet_address("");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_wallet_address_invalid_base58() {
        let result = QrCodeService::validate_wallet_address("invalid0OIl");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_wallet_address_wrong_length() {
        // Valid base58 but wrong length (not 32 bytes)
        let result = QrCodeService::validate_wallet_address("111111111111111111111111111111");
        assert!(result.is_err());
        match result {
            Err(ProximityError::InvalidInput(msg)) => {
                assert!(msg.contains("Invalid wallet address length"));
            }
            _ => panic!("Expected InvalidInput error"),
        }
    }
}
