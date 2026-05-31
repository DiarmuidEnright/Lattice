//! QR code support for meta-addresses

use crate::error::{StealthError, StealthResult};
use qrcode::QrCode;
use image::{DynamicImage, Luma};

/// QR code utilities for meta-addresses
pub struct QrCodeHandler;

impl QrCodeHandler {
    /// Encode meta-address to QR code format
    /// 
    /// Returns PNG image data as bytes
    pub fn encode(meta_address: &str) -> StealthResult<Vec<u8>> {
        // Validate meta-address format
        if !meta_address.starts_with("stealth:") {
            return Err(StealthError::InvalidMetaAddress(
                "Meta-address must start with 'stealth:'".to_string()
            ));
        }

        // Generate QR code
        let qr = QrCode::new(meta_address.as_bytes())
            .map_err(|e| StealthError::QrCodeError(format!("Failed to generate QR code: {}", e)))?;

        // Convert to image
        let image = qr.render::<Luma<u8>>().build();
        
        // Convert to PNG bytes
        let mut png_data = Vec::new();
        let dynamic_image = DynamicImage::ImageLuma8(image);
        dynamic_image.write_to(&mut std::io::Cursor::new(&mut png_data), image::ImageFormat::Png)
            .map_err(|e| StealthError::QrCodeError(format!("Failed to encode PNG: {}", e)))?;

        Ok(png_data)
    }

    /// Decode meta-address from QR code
    /// 
    /// Accepts PNG image data as bytes
    pub fn decode(qr_data: &[u8]) -> StealthResult<String> {
        // Load image from bytes
        let img = image::load_from_memory(qr_data)
            .map_err(|e| StealthError::QrCodeError(format!("Failed to load image: {}", e)))?;

        // Convert to grayscale
        let gray_img = img.to_luma8();

        // Use rqrr for decoding
        let mut img_for_decode = rqrr::PreparedImage::prepare(gray_img);
        let grids = img_for_decode.detect_grids();

        if grids.is_empty() {
            return Err(StealthError::QrCodeError("No QR code found in image".to_string()));
        }

        // Decode the first QR code found
        let (_, content) = grids[0].decode()
            .map_err(|e| StealthError::QrCodeError(format!("Failed to decode QR code: {:?}", e)))?;

        // content is already a String from rqrr
        let meta_address = content;

        // Validate meta-address format
        if !meta_address.starts_with("stealth:") {
            return Err(StealthError::InvalidMetaAddress(
                "Decoded data is not a valid meta-address".to_string()
            ));
        }

        Ok(meta_address)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageBuffer;

    #[test]
    fn test_encode_valid_meta_address() {
        let meta_address = "stealth:1:5ZWj7a1f8tWkjBESHKgrLmXshuXxqeY9SYcfbshpAqPG:3J98t1WpEZ73CNmYviecrnyiWrnqRhWNLy8F8qkgz3sP";
        
        let result = QrCodeHandler::encode(meta_address);
        assert!(result.is_ok());
        
        let png_data = result.unwrap();
        assert!(!png_data.is_empty());
        
        // Verify it's valid PNG data (starts with PNG signature)
        assert_eq!(&png_data[0..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
    }

    #[test]
    fn test_encode_invalid_meta_address() {
        let invalid_address = "not-a-stealth-address";
        
        let result = QrCodeHandler::encode(invalid_address);
        assert!(result.is_err());
        
        match result {
            Err(StealthError::InvalidMetaAddress(_)) => {},
            _ => panic!("Expected InvalidMetaAddress error"),
        }
    }

    #[test]
    fn test_encode_decode_round_trip() {
        let meta_address = "stealth:1:5ZWj7a1f8tWkjBESHKgrLmXshuXxqeY9SYcfbshpAqPG:3J98t1WpEZ73CNmYviecrnyiWrnqRhWNLy8F8qkgz3sP";
        
        // Encode
        let png_data = QrCodeHandler::encode(meta_address).unwrap();
        
        // Decode
        let decoded = QrCodeHandler::decode(&png_data).unwrap();
        
        // Verify round-trip
        assert_eq!(decoded, meta_address);
    }

    #[test]
    fn test_encode_decode_hybrid_meta_address() {
        let hybrid_meta_address = "stealth:2:5ZWj7a1f8tWkjBESHKgrLmXshuXxqeY9SYcfbshpAqPG:3J98t1WpEZ73CNmYviecrnyiWrnqRhWNLy8F8qkgz3sP:KyberPublicKeyBase58Encoded";
        
        // Encode
        let png_data = QrCodeHandler::encode(hybrid_meta_address).unwrap();
        
        // Decode
        let decoded = QrCodeHandler::decode(&png_data).unwrap();
        
        // Verify round-trip
        assert_eq!(decoded, hybrid_meta_address);
    }

    #[test]
    fn test_decode_invalid_image() {
        let invalid_data = vec![0u8; 100];
        
        let result = QrCodeHandler::decode(&invalid_data);
        assert!(result.is_err());
        
        match result {
            Err(StealthError::QrCodeError(_)) => {},
            _ => panic!("Expected QrCodeError"),
        }
    }

    #[test]
    fn test_decode_image_without_qr_code() {
        // Create a simple blank image
        let img: ImageBuffer<Luma<u8>, Vec<u8>> = ImageBuffer::new(100, 100);
        let dynamic_img = DynamicImage::ImageLuma8(img);
        
        let mut png_data = Vec::new();
        dynamic_img.write_to(&mut std::io::Cursor::new(&mut png_data), image::ImageFormat::Png).unwrap();
        
        let result = QrCodeHandler::decode(&png_data);
        assert!(result.is_err());
        
        match result {
            Err(StealthError::QrCodeError(msg)) => {
                assert!(msg.contains("No QR code found"));
            },
            _ => panic!("Expected QrCodeError with 'No QR code found'"),
        }
    }

    #[test]
    fn test_encode_long_meta_address() {
        // Test with a very long meta-address (should still work)
        let long_meta_address = format!(
            "stealth:2:{}:{}:{}",
            "A".repeat(44), // Base58 encoded key
            "B".repeat(44),
            "C".repeat(200) // Long Kyber key
        );
        
        let result = QrCodeHandler::encode(&long_meta_address);
        assert!(result.is_ok());
        
        let png_data = result.unwrap();
        let decoded = QrCodeHandler::decode(&png_data).unwrap();
        assert_eq!(decoded, long_meta_address);
    }

    #[test]
    fn test_decode_non_stealth_qr_code() {
        // Create QR code with non-stealth data
        let non_stealth_data = "https://example.com";
        let qr = QrCode::new(non_stealth_data.as_bytes()).unwrap();
        let image = qr.render::<Luma<u8>>().build();
        
        let mut png_data = Vec::new();
        let dynamic_image = DynamicImage::ImageLuma8(image);
        dynamic_image.write_to(&mut std::io::Cursor::new(&mut png_data), image::ImageFormat::Png).unwrap();
        
        let result = QrCodeHandler::decode(&png_data);
        assert!(result.is_err());
        
        match result {
            Err(StealthError::InvalidMetaAddress(_)) => {},
            _ => panic!("Expected InvalidMetaAddress error"),
        }
    }
}
