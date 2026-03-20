// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// FirmwareVerifier (F12) — SW package signature verification (ISO 24089)
//
// Provides cryptographic integrity verification for firmware packages before
// activation. Follows ISO 24089 (Road vehicles — Software update engineering)
// requirement for signature verification prior to installation/activation.
//
// Implementations:
//   - Ed25519Verifier  (default, using ed25519-dalek)
//   - NoopVerifier     (passthrough for testing / unsigned packages)
//
// Integration point: called by `activate_software_package` handler to gate
// activation on a valid signature.
// ─────────────────────────────────────────────────────────────────────────────

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

/// Result of a firmware signature verification.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VerificationResult {
    /// Whether the signature is valid.
    pub valid: bool,
    /// SHA-256 digest of the firmware payload (hex-encoded).
    pub digest: String,
    /// Human-readable detail (e.g. "Ed25519 signature valid", or error reason).
    pub detail: String,
}

/// Trait for firmware signature verification.
///
/// Implementors verify that a firmware payload matches a given cryptographic
/// signature, ensuring integrity and authenticity before activation.
pub trait FirmwareVerifier: Send + Sync + 'static {
    /// Verify a firmware payload against a signature.
    ///
    /// - `payload`: raw firmware bytes
    /// - `signature`: detached signature bytes (algorithm-specific)
    ///
    /// Returns a `VerificationResult` with validity, digest, and detail.
    fn verify(&self, payload: &[u8], signature: &[u8]) -> VerificationResult;

    /// Name of the verification algorithm (e.g. "Ed25519", "Noop").
    fn algorithm(&self) -> &'static str;
}

// ─────────────────────────────────────────────────────────────────────────────
// Ed25519Verifier — production implementation
// ─────────────────────────────────────────────────────────────────────────────

/// Ed25519 signature verifier using a fixed public key.
///
/// The public key is loaded at construction time (from PEM or raw bytes).
/// Verification:
///   1. Compute SHA-256 digest of the payload
///   2. Verify the Ed25519 signature over the **raw payload** (not the digest)
///
/// This matches the ISO 24089 pattern where the OEM signs firmware with a
/// private key and the vehicle verifies with the corresponding public key.
pub struct Ed25519Verifier {
    verifying_key: VerifyingKey,
}

impl Ed25519Verifier {
    /// Create a verifier from raw 32-byte Ed25519 public key bytes.
    ///
    /// # Errors
    /// Returns an error if the bytes are not a valid Ed25519 public key.
    pub fn from_bytes(public_key_bytes: &[u8; 32]) -> Result<Self, String> {
        let verifying_key = VerifyingKey::from_bytes(public_key_bytes)
            .map_err(|e| format!("Invalid Ed25519 public key: {e}"))?;
        Ok(Self { verifying_key })
    }

    /// Create a verifier from a hex-encoded 32-byte Ed25519 public key.
    ///
    /// # Errors
    /// Returns an error if the hex string is invalid or not 32 bytes.
    pub fn from_hex(hex_key: &str) -> Result<Self, String> {
        let bytes = hex::decode(hex_key).map_err(|e| format!("Invalid hex: {e}"))?;
        if bytes.len() != 32 {
            return Err(format!(
                "Ed25519 public key must be 32 bytes, got {}",
                bytes.len()
            ));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Self::from_bytes(&arr)
    }
}

impl FirmwareVerifier for Ed25519Verifier {
    fn verify(&self, payload: &[u8], signature_bytes: &[u8]) -> VerificationResult {
        let digest = hex::encode(Sha256::digest(payload));

        // Parse signature (must be exactly 64 bytes)
        let signature = match Signature::from_slice(signature_bytes) {
            Ok(sig) => sig,
            Err(e) => {
                return VerificationResult {
                    valid: false,
                    digest,
                    detail: format!("Invalid signature format: {e}"),
                };
            }
        };

        match self.verifying_key.verify(payload, &signature) {
            Ok(()) => VerificationResult {
                valid: true,
                digest,
                detail: "Ed25519 signature valid".to_owned(),
            },
            Err(e) => VerificationResult {
                valid: false,
                digest,
                detail: format!("Ed25519 signature invalid: {e}"),
            },
        }
    }

    fn algorithm(&self) -> &'static str {
        "Ed25519"
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// NoopVerifier — passthrough (testing / unsigned packages)
// ─────────────────────────────────────────────────────────────────────────────

/// No-op verifier that always passes. Use only for testing or when
/// signature verification is disabled.
pub struct NoopVerifier;

impl FirmwareVerifier for NoopVerifier {
    fn verify(&self, payload: &[u8], _signature: &[u8]) -> VerificationResult {
        let digest = hex::encode(Sha256::digest(payload));
        VerificationResult {
            valid: true,
            digest,
            detail: "Signature verification disabled (NoopVerifier)".to_owned(),
        }
    }

    fn algorithm(&self) -> &'static str {
        "Noop"
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn generate_keypair() -> (SigningKey, VerifyingKey) {
        let signing_key = SigningKey::from_bytes(&[42u8; 32]);
        let verifying_key = signing_key.verifying_key();
        (signing_key, verifying_key)
    }

    #[test]
    fn ed25519_valid_signature() {
        let (signing_key, verifying_key) = generate_keypair();
        let payload = b"firmware image v2.1.0";
        let signature = signing_key.sign(payload);

        let verifier = Ed25519Verifier::from_bytes(&verifying_key.to_bytes()).unwrap();
        let result = verifier.verify(payload, &signature.to_bytes());

        assert!(result.valid);
        assert_eq!(result.detail, "Ed25519 signature valid");
        assert!(!result.digest.is_empty());
    }

    #[test]
    fn ed25519_invalid_signature() {
        let (_signing_key, verifying_key) = generate_keypair();
        let payload = b"firmware image v2.1.0";
        let bad_signature = [0u8; 64]; // wrong signature

        let verifier = Ed25519Verifier::from_bytes(&verifying_key.to_bytes()).unwrap();
        let result = verifier.verify(payload, &bad_signature);

        assert!(!result.valid);
        assert!(result.detail.contains("invalid"));
    }

    #[test]
    fn ed25519_tampered_payload() {
        let (signing_key, verifying_key) = generate_keypair();
        let payload = b"firmware image v2.1.0";
        let signature = signing_key.sign(payload);

        let verifier = Ed25519Verifier::from_bytes(&verifying_key.to_bytes()).unwrap();
        let tampered = b"firmware image v2.1.0-TAMPERED";
        let result = verifier.verify(tampered, &signature.to_bytes());

        assert!(!result.valid);
    }

    #[test]
    fn ed25519_wrong_key() {
        let (signing_key, _verifying_key) = generate_keypair();
        let payload = b"firmware image v2.1.0";
        let signature = signing_key.sign(payload);

        // Different key
        let other_key = SigningKey::from_bytes(&[99u8; 32]).verifying_key();
        let verifier = Ed25519Verifier::from_bytes(&other_key.to_bytes()).unwrap();
        let result = verifier.verify(payload, &signature.to_bytes());

        assert!(!result.valid);
    }

    #[test]
    fn ed25519_malformed_signature_bytes() {
        let (_signing_key, verifying_key) = generate_keypair();
        let payload = b"firmware image";
        let bad_sig = [0u8; 10]; // too short

        let verifier = Ed25519Verifier::from_bytes(&verifying_key.to_bytes()).unwrap();
        let result = verifier.verify(payload, &bad_sig);

        assert!(!result.valid);
        assert!(result.detail.contains("Invalid signature format"));
    }

    #[test]
    fn ed25519_from_hex() {
        let (_signing_key, verifying_key) = generate_keypair();
        let hex_key = hex::encode(verifying_key.to_bytes());
        let verifier = Ed25519Verifier::from_hex(&hex_key);
        assert!(verifier.is_ok());
        assert_eq!(verifier.unwrap().algorithm(), "Ed25519");
    }

    #[test]
    fn ed25519_from_hex_invalid() {
        assert!(Ed25519Verifier::from_hex("not-hex").is_err());
        assert!(Ed25519Verifier::from_hex("aabb").is_err()); // too short
    }

    #[test]
    fn noop_always_passes() {
        let verifier = NoopVerifier;
        let result = verifier.verify(b"anything", b"whatever");
        assert!(result.valid);
        assert!(result.detail.contains("disabled"));
        assert_eq!(verifier.algorithm(), "Noop");
    }

    #[test]
    fn digest_is_sha256_hex() {
        let verifier = NoopVerifier;
        let result = verifier.verify(b"test", b"");
        // SHA-256 of "test" is well-known
        assert_eq!(result.digest.len(), 64); // 32 bytes hex = 64 chars
        assert_eq!(
            result.digest,
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
        );
    }

    #[test]
    fn verification_result_serializes() {
        let result = VerificationResult {
            valid: true,
            digest: "abc123".to_owned(),
            detail: "ok".to_owned(),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"valid\":true"));
    }
}
