//! BLS Signatures
//!
//! Implements Boneh–Lynn–Shacham (BLS) digital signatures using ronkathon's
//! existing curve and pairing primitives. This module demonstrates key generation,
//! signing, verification, and aggregation (for signatures on the same message).
//!
//! Security Note: The hash-to-curve function below (using a try-and-increment
//! strategy) is provided for educational purposes. In a secure production system,
//! please follow updated RFCs (e.g. RFC 9380) and optimized constant‑time algorithms.

use rand::Rng;
use crate::algebra::field::Field;
use crate::algebra::group::FiniteCyclicGroup;
use crate::algebra::Finite;
use crate::curve;
use crate::hashes::sha3::Sha3_256 as Sha256;
use crate::algebra::field::prime::PlutoScalarField;
use crate::curve::{AffinePoint, pluto_curve::PlutoExtendedCurve};
use crate::curve::pairing::pairing;

/// Errors that can occur during BLS signature operations.
#[derive(Debug)]
pub enum BlsError {
    /// The provided public key is invalid.
    InvalidPublicKey,
    /// The signature is invalid.
    InvalidSignature,
    /// Hash-to-curve failed to find a valid point on the curve.
    HashToCurveFailed,
    /// Signature verification failed.
    VerificationFailed,
    /// Other error with a descriptive message.
    Other(String),
}

/// BLS private key.
pub struct BlsPrivateKey {
    sk: PlutoScalarField,
}

/// BLS public key.
pub struct BlsPublicKey {
    pk: AffinePoint<PlutoExtendedCurve>,
}

/// BLS signature.
pub struct BlsSignature {
    sig: AffinePoint<PlutoExtendedCurve>,
}

impl BlsPrivateKey {
    /// Generates a new BLS private key using the provided random number generator.
    pub fn generate<R: Rng>(rng: &mut R) -> Self {
        // Generate a random scalar in the range [1, ORDER]
        let sk = PlutoScalarField::new(rng.gen_range(1..=PlutoScalarField::ORDER));
        BlsPrivateKey { sk }
    }

    /// Returns the corresponding BLS public key.
    pub fn public_key(&self) -> BlsPublicKey {
        // Calculate public key as sk * G, where G is the generator of the subgroup.
        let pk = AffinePoint::<PlutoExtendedCurve>::GENERATOR * self.sk;
        BlsPublicKey { pk }
    }

    /// Signs a message using the BLS private key.
    ///
    /// The signature is computed as sk * H(m), where H is a hash-to-curve function.
    pub fn sign(&self, msg: &[u8]) -> Result<BlsSignature, BlsError> {
        let hash_point = hash_to_curve(msg)?;
        let sig_point = hash_point * self.sk;
        Ok(BlsSignature { sig: sig_point })
    }
}

impl BlsPublicKey {
    /// Verifies a BLS signature against the given message.
    ///
    /// The verification check uses the bilinear pairing:
    ///   e(signature, G) == e(H(message), public_key)
    pub fn verify(&self, msg: &[u8], signature: &BlsSignature) -> Result<(), BlsError> {
        let hash_point = hash_to_curve(msg)?;
        let left = pairing::<PlutoExtendedCurve, 17>(signature.sig, AffinePoint::<PlutoExtendedCurve>::GENERATOR);
        let right = pairing::<PlutoExtendedCurve, 17>(hash_point, self.pk);
        if left == right {
            Ok(())
        } else {
            Err(BlsError::VerificationFailed)
        }
    }
}

impl BlsSignature {
    /// Aggregates multiple BLS signatures into a single signature.
    ///
    /// This function sums the individual signature points. All signatures must be on the same message.
    pub fn aggregate(signatures: &[BlsSignature]) -> Result<BlsSignature, BlsError> {
        if signatures.is_empty() {
            return Err(BlsError::Other("No signatures to aggregate".into()));
        }
        let mut agg = signatures[0].sig.clone();
        for sig in signatures.iter().skip(1) {
            agg = agg + sig.sig;
        }
        Ok(BlsSignature { sig: agg })
    }
}

/// Verifies an aggregated BLS signature for a single common message.
///
/// For aggregated signatures the verification check is:
///   e(aggregated_signature, G) == e(H(message), ∑ public_keys)
pub fn verify_aggregated_signature(
    msg: &[u8],
    pks: &[BlsPublicKey],
    aggregated_sig: &BlsSignature,
) -> Result<(), BlsError> {
    if pks.is_empty() {
        return Err(BlsError::Other("No public keys provided".into()));
    }
    let mut agg_pk = pks[0].pk.clone();
    for pk in pks.iter().skip(1) {
        agg_pk = agg_pk + pk.pk;
    }
    let hash_point = hash_to_curve(msg)?;
    let left = pairing::<PlutoExtendedCurve, 17>(
        aggregated_sig.sig,
        AffinePoint::<PlutoExtendedCurve>::GENERATOR,
    );
    let right = pairing::<PlutoExtendedCurve, 17>(hash_point, agg_pk);
    if left == right {
        Ok(())
    } else {
        Err(BlsError::VerificationFailed)
    }
}

/// Converts a message to a point on the curve using a basic try-and-increment hash-to-curve method.
///
/// This simplistic implementation is for educational purposes and is not optimized for production use.
fn hash_to_curve(msg: &[u8]) -> Result<AffinePoint<PlutoExtendedCurve>, BlsError> {
    let mut counter = 0u32;
    loop {
        let mut hasher = Sha256::new();
        hasher.update(msg);
        hasher.update(&counter.to_be_bytes());
        let hash_result = hasher.finalize();
        if hash_result.len() < 8 {
            return Err(BlsError::Other("Hash output too short".into()));
        }
        let x_bytes: [u8; 8] = hash_result[0..8].try_into().unwrap();
        let x_val = u64::from_be_bytes(x_bytes);
        // Convert the integer into a field element.
        let candidate_x = <curve::pluto_curve::PlutoExtendedCurve as curve::EllipticCurve>::BaseField::from(x_val);
        // Compute y² = x³ + 3 using the curve equation: y² = x³ + 3.
        let x3 = candidate_x * candidate_x * candidate_x;
        let y2 = x3 + <curve::pluto_curve::PlutoExtendedCurve as curve::EllipticCurve>::BaseField::from(3u64);
        if let Some(candidate_y) = y2.sqrt() {
            let point = AffinePoint::<PlutoExtendedCurve>::new(candidate_x, candidate_y.0);
            // Verify that the point is in the correct subgroup by checking
            // that multiplying by the subgroup order yields the identity.
            let subgroup_order = 17; // For Pluto curve, as defined in this example.
            if (point * PlutoScalarField::new(subgroup_order)) == AffinePoint::<PlutoExtendedCurve>::Infinity {
                return Ok(point);
            }
        }
        counter += 1;
        if counter > 1000 {
            return Err(BlsError::HashToCurveFailed);
        }
    }
} 