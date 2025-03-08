//! BLS Signatures
//!
//! Implements Boneh–Lynn–Shacham (BLS) digital signatures using ronkathon's
//! existing curve and pairing primitives. This module demonstrates key generation,
//! signing, verification, and aggregation (for signatures on the same message).

use std::cmp::Ordering;

use rand::{rngs::StdRng, Rng, SeedableRng};

use crate::{
  algebra::{
    field::{
      extension::PlutoBaseFieldExtension,
      prime::{PlutoBaseField, PlutoScalarField, PrimeField},
      Field, FiniteField,
    },
    group::FiniteCyclicGroup,
    Finite,
  },
  curve::{
    pairing::pairing,
    pluto_curve::{PlutoBaseCurve, PlutoExtendedCurve},
    AffinePoint,
  },
  hashes::sha3::Sha3_256 as Sha256,
  hmac::hmac_sha256::hmac_sha256,
};

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
  /// Invalid point encountered.
  InvalidPoint,
}

/// BLS private key.
pub struct BlsPrivateKey {
  sk: PlutoScalarField,
}

/// BLS public key.
pub struct BlsPublicKey {
  pk: AffinePoint<PlutoBaseCurve>,
}

/// BLS signature.
pub struct BlsSignature {
  sig: AffinePoint<PlutoExtendedCurve>,
}

/// Proof of Possession (PoP) for a BLS public key.
/// This prevents rogue key attacks by requiring signers to prove knowledge of their secret key.
pub struct ProofOfPossession {
  pop: BlsSignature,
}

/// Converts a nonnegative integer to an octet string of a specified length using crypto-bigint.
///
/// I2OSP (Integer-to-Octet-String Primitive) converts a nonnegative integer `x`
/// into its big-endian representation, trimmed of any excess leading zeroes, and
/// left-padded with zeroes so that the result has exactly `length` bytes.
///
/// # Arguments
///
/// * `x` - A reference to a `usize` representing the nonnegative integer.
/// * `length` - The number of octets (bytes) the output string should have.
///
/// # Returns
///
/// * `Ok(Vec<u8>)` containing the octet string if the integer can be represented in the specified
///   length.
/// * `Err(String)` if the integer is too large to be encoded in the given number of octets.
///
/// # Example
///
/// ```

/// ```
pub fn i2osp(x: usize, length: usize) -> Result<Vec<u8>, String> {
  if x >= (1 << (8 * length)) {
    return Err(format!("Integer too large to encode in {} octets", length));
  }

  let mut result = vec![0u8; length];
  let mut val = x;

  // Fill from right to left
  for i in (0..length).rev() {
    result[i] = (val & 0xff) as u8;
    val >>= 8;
  }

  Ok(result)
}
/// Converts an octet string to a nonnegative integer as a U256 using crypto-bigint.
///
/// OS2IP (Octet-String-to-Integer Primitive) interprets an octet string as the big-endian
/// representation of a nonnegative integer. When the input slice is longer than 32 bytes, the
/// function verifies that the extra leading bytes are all zero (so that the value fits in 256
/// bits).
///
/// # Arguments
///
/// * `octets` - A slice of bytes representing the octet string.
///
/// # Returns
///
/// * `Ok(Usize)` corresponding to the nonnegative integer value of `octets`.
/// * `Err(String)` if the octet string represents a number that does not fit in 256 bits.
///
/// # Example
///
/// ```

/// ```
pub fn os2ip(octets: &[u8]) -> Result<usize, String> {
  let mut ret = 0usize;
  for &byte in octets {
    ret <<= 8;
    ret += byte as usize;
  }
  Ok(ret)
}

// HKDF

/// HKDF-Extract according to RFC 5869.
/// If no salt is provided (i.e., salt is empty), a zero-filled salt of 32-bytes (SHA-256 output
/// length) is used.
pub fn hkdf_extract(salt: &[u8], ikm: &[u8]) -> Vec<u8> {
  let salt = if salt.is_empty() {
    // For SHA-256, the hash length is 32 bytes.
    vec![0u8; 32]
  } else {
    salt.to_vec()
  };
  hmac_sha256(&salt, ikm).to_vec()
}

/// Implements expand_message_xmd as specified in the standard
fn expand_message_xmd(msg: &[u8], dst: &[u8], len_in_bytes: usize) -> Vec<u8> {
  // Parameters for SHA-256
  const B_IN_BYTES: usize = 32; // hash digest size
  const R_IN_BYTES: usize = 64; // hash block size

  let ell = (len_in_bytes + B_IN_BYTES - 1) / B_IN_BYTES;
  assert!(ell <= 255 && len_in_bytes <= 65535 && dst.len() <= 255);

  // DST_prime = DST || I2OSP(len(DST), 1)
  let dst_prime = [dst, &[dst.len() as u8]].concat();

  // Z_pad = I2OSP(0, r_in_bytes)
  let z_pad = vec![0u8; R_IN_BYTES];

  // l_i_b_str = I2OSP(len_in_bytes, 2)
  let l_i_b_str = i2osp(len_in_bytes, 2).unwrap();

  // msg_prime = Z_pad || msg || l_i_b_str || I2OSP(0, 1) || DST_prime
  let mut msg_prime = Vec::new();
  msg_prime.extend_from_slice(&z_pad);
  msg_prime.extend_from_slice(msg);
  msg_prime.extend_from_slice(&l_i_b_str);
  msg_prime.push(0u8);
  msg_prime.extend_from_slice(&dst_prime);

  // b_0 = H(msg_prime)
  let mut hasher = Sha256::new();
  hasher.update(&msg_prime);
  let b_0 = hasher.finalize();

  // b_1 = H(b_0 || I2OSP(1, 1) || DST_prime)
  let mut hasher = Sha256::new();
  hasher.update(&b_0);
  hasher.update(&i2osp(1, 1).unwrap());
  hasher.update(&dst_prime);
  let b_1 = hasher.finalize();

  let mut uniform_bytes = b_1.to_vec();

  // Rest of b_vals: H(strxor(b_0, b_(i-1)) || I2OSP(i + 1, 1) || DST_prime)
  for i in 2..=ell {
    let mut hasher = Sha256::new();
    let prev_b = &uniform_bytes[(i - 2) * B_IN_BYTES..(i - 1) * B_IN_BYTES];
    let xored: Vec<u8> = b_0.iter().zip(prev_b).map(|(a, b)| a ^ b).collect();
    hasher.update(&xored);
    hasher.update(&i2osp(i, 1).unwrap());
    hasher.update(&dst_prime);
    uniform_bytes.extend_from_slice(&hasher.finalize());
  }

  uniform_bytes.truncate(len_in_bytes);
  uniform_bytes
}

/// Implements hash_to_field as specified in the standard
fn hash_to_field(msg: &[u8], count: usize) -> Vec<PlutoBaseFieldExtension> {
  const DST: &[u8] = b"BLS_SIG_PLUTO_RONKATHON_2024";
  let p = PlutoBaseField::ORDER; // modulus
  let degree = 2; // for GF(p²)
  let blen = 64; //

  let len_in_bytes = count * degree * blen;
  let uniform_bytes = expand_message_xmd(msg, DST, len_in_bytes);

  let mut result = Vec::with_capacity(count);
  for i in 0..count {
    let mut e_vals = [PrimeField::ZERO; 2];
    for j in 0..degree {
      let elm_offset = blen * (j + i * degree);
      let tv = &uniform_bytes[elm_offset..elm_offset + blen];

      // Convert bytes to integer mod p, using all bytes
      let mut val = 0usize;
      for byte in tv {
        val = (val * 256 + *byte as usize) % p;
      }
      e_vals[j] = PrimeField::new(val);
    }
    result.push(PlutoBaseFieldExtension::new(e_vals));
  }

  result
}

impl ProofOfPossession {
  /// Verifies the proof of possession for a BLS public key.
  pub fn verify(&self, pk: &BlsPublicKey) -> Result<(), BlsError> {
    pk.validate()?;
    // Build the properly twisted generator G from the base-curve generator.
    let g = if let AffinePoint::<PlutoBaseCurve>::Point(x, y) =
      AffinePoint::<PlutoBaseCurve>::GENERATOR
    {
      let cube_root = PlutoBaseFieldExtension::primitive_root_of_unity(3);
      AffinePoint::<PlutoExtendedCurve>::new(
        cube_root * PlutoBaseFieldExtension::from(x),
        PlutoBaseFieldExtension::from(y),
      )
    } else {
      return Err(BlsError::InvalidPoint);
    };

    let pk_ext = convert_to_extended(pk.pk);
    let left = pairing::<PlutoExtendedCurve, 17>(self.pop.sig, g);
    let right = pairing::<PlutoExtendedCurve, 17>(pk_ext, pk_ext);
    if canonicalize_extension(left) == canonicalize_extension(right) {
      Ok(())
    } else {
      Err(BlsError::VerificationFailed)
    }
  }
}
impl BlsPrivateKey {
  /// Returns the corresponding BLS secret key. subject to a lot of issues due to local caching
  pub fn generate_random<R: Rng>(rng: &mut R) -> Self {
    let sk = PlutoScalarField::new(rng.gen_range(1..=PlutoScalarField::ORDER));
    BlsPrivateKey { sk }
  }

  /// Returns the corresponding BLS secret key.
  pub fn generate_deterministic(seed: u64) -> Self {
    let mut rng = StdRng::seed_from_u64(seed);
    Self::generate_random(&mut rng)
  }

  /// Returns the corresponding BLS public key.
  pub fn public_key(&self) -> BlsPublicKey {
    // Calculate public key as sk * G, where G is the generator of the subgroup.
    let pk = AffinePoint::<PlutoBaseCurve>::GENERATOR * self.sk;
    BlsPublicKey { pk }
  }

  /// Signs a message using the BLS private key.
  ///
  /// The signature is computed as sk * H(m), where H is a hash-to-curve function.
  pub fn sign(&self, msg: &[u8]) -> Result<BlsSignature, BlsError> {
    let hash_point = hash_to_curve(msg)?;

    // Sign
    let sig_point = hash_point * self.sk;

    Ok(BlsSignature { sig: canonicalize(sig_point) })
  }

  /// Generates a proof of possession for the private key.
  /// The proof is a signature on the public key bytes.
  pub fn generate_proof_of_possession(&self) -> Result<ProofOfPossession, BlsError> {
    let pk = self.public_key();

    // Sign the public key bytes
    let pop = BlsSignature { sig: convert_to_extended(pk.pk) * self.sk };
    Ok(ProofOfPossession { pop })
  }
}
impl BlsPublicKey {
  /// Verifies a BLS signature against the given message.
  ///
  /// The verification check uses the bilinear pairing:
  ///   e(signature, G) == e(H(message), public_key)
  pub fn verify(&self, msg: &[u8], signature: &BlsSignature) -> Result<(), BlsError> {
    self.validate()?;
    // Hash the message to a point on the extended curve.
    let hash_point = hash_to_curve(msg)?;

    // Build the properly twisted generator G from the base-curve generator.
    let g = if let AffinePoint::<PlutoBaseCurve>::Point(x, y) =
      AffinePoint::<PlutoBaseCurve>::GENERATOR
    {
      let cube_root = PlutoBaseFieldExtension::primitive_root_of_unity(3);
      AffinePoint::<PlutoExtendedCurve>::new(
        cube_root * PlutoBaseFieldExtension::from(x),
        PlutoBaseFieldExtension::from(y),
      )
    } else {
      return Err(BlsError::InvalidPoint);
    };

    // Convert the public key into the extended group and canonicalize.
    let pk = convert_to_extended(self.pk);

    // Compute the pairing outputs.
    let left = pairing::<PlutoExtendedCurve, 17>(signature.sig, g);
    let right = pairing::<PlutoExtendedCurve, 17>(hash_point, pk);

    // Compare the canonical representations of each pairing output.
    if left == right {
      Ok(())
    } else {
      Err(BlsError::VerificationFailed)
    }
  }

  /// Validates a BLS public key according to the spec
  pub fn validate(&self) -> Result<(), BlsError> {
    // Check if point is valid (implicitly done by AffinePoint type)

    // Check if point is identity element
    if self.pk == AffinePoint::<PlutoBaseCurve>::Infinity {
      return Err(BlsError::InvalidPublicKey);
    }

    // Check if point is in the correct subgroup
    let subgroup_order = 17;
    if (self.pk * PlutoScalarField::new(subgroup_order)) != AffinePoint::<PlutoBaseCurve>::Infinity
    {
      return Err(BlsError::InvalidPublicKey);
    }

    Ok(())
  }
}

impl BlsSignature {
  /// Aggregates multiple BLS signatures into a single signature.
  ///
  /// This function sums the individual signature points. All signatures must be on the same
  /// message.
  pub fn aggregate(signatures: &[BlsSignature]) -> Result<BlsSignature, BlsError> {
    if signatures.is_empty() {
      return Err(BlsError::Other("No signatures to aggregate".into()));
    }
    let mut agg = signatures[0].sig;
    for sig in signatures.iter().skip(1) {
      agg += sig.sig;
    }
    Ok(BlsSignature { sig: agg })
  }
}

/// Verifies an aggregated BLS signature for a single common message:
///   e(aggregated_signature, G) == ∏ e(H(m), pk_i)
pub fn verify_aggregated_signature(
  pks: &[BlsPublicKey],
  messages: &[&[u8]],
  aggregated_sig: &BlsSignature,
) -> Result<(), BlsError> {
  if pks.is_empty() || messages.is_empty() || pks.len() != messages.len() {
    return Err(BlsError::Other("Invalid input lengths".to_string()));
  }

  // Build the same properly twisted generator G.
  let g =
    if let AffinePoint::<PlutoBaseCurve>::Point(x, y) = AffinePoint::<PlutoBaseCurve>::GENERATOR {
      let cube_root = PlutoBaseFieldExtension::primitive_root_of_unity(3);
      AffinePoint::<PlutoExtendedCurve>::new(
        cube_root * PlutoBaseFieldExtension::from(x),
        PlutoBaseFieldExtension::from(y),
      )
    } else {
      return Err(BlsError::InvalidPoint);
    };

  // Verification: e(aggregated_sig, G) must equal the product over all pairings.
  let left = pairing::<PlutoExtendedCurve, 17>(aggregated_sig.sig, g);

  let mut right = PlutoBaseFieldExtension::ONE;
  for (pk, msg) in pks.iter().zip(messages.iter()) {
    pk.validate()?;
    let hash_point = hash_to_curve(msg)?;
    let pk_extended = convert_to_extended(pk.pk);
    right *= pairing::<PlutoExtendedCurve, 17>(hash_point, pk_extended);
  }

  if canonicalize_extension(left) == canonicalize_extension(right) {
    Ok(())
  } else {
    Err(BlsError::VerificationFailed)
  }
}

fn convert_to_extended(point: AffinePoint<PlutoBaseCurve>) -> AffinePoint<PlutoExtendedCurve> {
  match point {
    AffinePoint::Point(x, y) => {
      let cube_root = PlutoBaseFieldExtension::primitive_root_of_unity(3);
      AffinePoint::<PlutoExtendedCurve>::new(
        cube_root * PlutoBaseFieldExtension::from(x),
        PlutoBaseFieldExtension::from(y),
      )
    },
    AffinePoint::Infinity => AffinePoint::<PlutoExtendedCurve>::Infinity,
  }
}
/// Implements map_to_curve as specified in the standard

/// Implements clear_cofactor as specified in the standard
fn clear_cofactor(point: AffinePoint<PlutoExtendedCurve>) -> AffinePoint<PlutoExtendedCurve> {
  let p = PlutoBaseField::ORDER; // 101
  let cofactor = (p * p - 1) / 17;

  let mut cleared = point * PlutoScalarField::new(cofactor);

  // Check if we need to adjust the point
  let mut sum = cleared;
  for _ in 0..17 {
    sum += cleared;
  }

  if sum != cleared {
    // If point doesn't have the property, multiply x by cube root
    if let AffinePoint::Point(x, y) = cleared {
      let cube_root = PlutoBaseFieldExtension::primitive_root_of_unity(3);
      cleared = AffinePoint::new(cube_root * x, y);
    }
  }

  cleared
}

/// Compares two extended field elements lexicographically.
pub fn lex_cmp_extension(a: &PlutoBaseFieldExtension, b: &PlutoBaseFieldExtension) -> Ordering {
  match a.coeffs[0].value.cmp(&b.coeffs[0].value) {
    Ordering::Equal => a.coeffs[1].value.cmp(&b.coeffs[1].value),
    ord => ord,
  }
}

/// Returns the canonical representation of an extension field element.
/// It returns the lexicographically smaller element between the given element and its negation.
pub fn canonicalize_extension(x: PlutoBaseFieldExtension) -> PlutoBaseFieldExtension {
  if lex_cmp_extension(&x, &(-x)) == Ordering::Greater {
    -x
  } else {
    x
  }
}

/// Updates the canonicalization for a point: it forces its y-coordinate to be in
/// the unique (canonical) form.
fn canonicalize(point: AffinePoint<PlutoExtendedCurve>) -> AffinePoint<PlutoExtendedCurve> {
  match point {
    AffinePoint::Infinity => point,
    AffinePoint::Point(x, y) => {
      // Instead of using is_negative we now use our lexicographic method.
      AffinePoint::Point(x, canonicalize_extension(y))
    },
  }
}

/// Returns the canonical square root of a field element in PlutoBaseFieldExtension.
/// such that alternative = -candidate.
pub fn sqrt_canonical(x: &PlutoBaseFieldExtension) -> Option<PlutoBaseFieldExtension> {
  x.sqrt().map(|(candidate, _alternative)| {
    // Choose the lexicographically smaller candidate: candidate or -candidate.
    if lex_cmp_extension(&candidate, &(-candidate)) == Ordering::Less {
      candidate
    } else {
      -candidate
    }
  })
}

/// Implements hash_to_curve as specified in the standard
fn hash_to_curve(msg: &[u8]) -> Result<AffinePoint<PlutoExtendedCurve>, BlsError> {
  let field_elems = hash_to_field(msg, 1);
  let mut x = field_elems[0];

  for _ in 0..100 {
    let x3 = x * x * x;
    let y2 = x3 + PlutoBaseFieldExtension::from(3u64);

    if y2.euler_criterion() {
      // Use the canonical square root.
      let y = sqrt_canonical(&y2).ok_or(BlsError::HashToCurveFailed)?;
      let point = AffinePoint::<PlutoExtendedCurve>::new(x, y);

      // Clear cofactor and verify point is in correct subgroup
      let cofactored = clear_cofactor(point);
      if (cofactored * PlutoScalarField::new(17)) == AffinePoint::<PlutoExtendedCurve>::Infinity {
        return Ok(cofactored);
      }
    }
    x += PlutoBaseFieldExtension::ONE;
  }

  Err(BlsError::HashToCurveFailed)
}

/// Verifies an aggregated BLS signature for a single common message by checking that the pairing of
/// the aggregated signature with the twisted generator equals the pairing of the message hash with
/// the aggregated public key.
///
/// # Arguments
///
/// * `pks` - A slice of BLS public keys.
/// * `msg` - The message to which the signatures correspond.
/// * `aggregated_sig` - The aggregated BLS signature.
///
/// # Returns
///
/// * `Ok(())` if the signature is valid, or a corresponding `BlsError` otherwise.
pub fn verify_aggregated_signature_single_message(
  pks: &[BlsPublicKey],
  msg: &[u8],
  aggregated_sig: &BlsSignature,
) -> Result<(), BlsError> {
  if pks.is_empty() {
    return Err(BlsError::Other("No public keys provided".to_string()));
  }

  // Build the twisted generator G₁.
  let g =
    if let AffinePoint::<PlutoBaseCurve>::Point(x, y) = AffinePoint::<PlutoBaseCurve>::GENERATOR {
      let cube_root = PlutoBaseFieldExtension::primitive_root_of_unity(3);
      AffinePoint::<PlutoExtendedCurve>::new(
        cube_root * PlutoBaseFieldExtension::from(x),
        PlutoBaseFieldExtension::from(y),
      )
    } else {
      return Err(BlsError::InvalidPoint);
    };

  // Convert and aggregate the public keys in the extended group.
  let mut aggregated_pk_ext: AffinePoint<PlutoExtendedCurve> =
    AffinePoint::<PlutoExtendedCurve>::Infinity;
  for pk in pks {
    pk.validate()?;
    let pk_ext = canonicalize(convert_to_extended(pk.pk));
    aggregated_pk_ext += pk_ext;
  }

  // Hash the common message to a point.
  let hash_point = hash_to_curve(msg)?;

  // Compute the pairings.
  let left = pairing::<PlutoExtendedCurve, 17>(aggregated_sig.sig, g);
  let right = pairing::<PlutoExtendedCurve, 17>(hash_point, aggregated_pk_ext);

  // Compare the canonical representation of both pairing outputs.
  if canonicalize_extension(left) == canonicalize_extension(right) {
    Ok(())
  } else {
    Err(BlsError::VerificationFailed)
  }
}

#[cfg(test)]
mod tests {

  use super::*;

  /// Creates a deterministic private key for testing using seed
  fn create_test_private_key(seed: u64) -> BlsPrivateKey {
    BlsPrivateKey::generate_deterministic(seed)
  }

  #[test]
  fn test_sign_and_verify() {
    let msg = b"Hello, BLS!";
    let sk = create_test_private_key(1234);
    let pk = sk.public_key();
    let sig = sk.sign(msg).expect("Signing should succeed");
    assert!(pk.verify(msg, &sig).is_ok(), "Valid signature should verify correctly");
  }

  #[test]
  fn test_invalid_signature() {
    let msg = b"Hello, BLS!";
    let sk = create_test_private_key(1234);
    let pk = sk.public_key();
    let tampered_sig = BlsSignature { sig: AffinePoint::<PlutoBaseCurve>::GENERATOR.into() };
    assert!(pk.verify(msg, &tampered_sig).is_err(), "Tampered signature should fail verification");
  }

  #[test]
  fn test_aggregate_signatures() {
    let msg = b"Hello, BLS!";
    let mut signatures = Vec::new();
    let mut public_keys = Vec::new();

    // Generate several keypairs with fixed seeds
    let test_seeds = [1234, 1234, 1234, 1234];
    for seed in test_seeds {
      let sk = create_test_private_key(seed);
      public_keys.push(sk.public_key());
      signatures.push(sk.sign(msg).expect("Signing should succeed"));
    }

    let aggregated_signature =
      BlsSignature::aggregate(&signatures).expect("Aggregation should succeed");

    assert!(
      verify_aggregated_signature_single_message(&public_keys, msg, &aggregated_signature).is_ok(),
      "Aggregated signature should verify correctly"
    );
  }

  #[test]
  fn test_verify_aggregated_empty_public_keys() {
    let msg = b"Aggregate with Empty Public Keys";
    let sk = create_test_private_key(1111);
    let sig = sk.sign(msg).expect("Signing should succeed");

    let res = verify_aggregated_signature_single_message(&[], &[], &sig);
    assert!(res.is_err(), "Verification with empty public key list should fail");
  }
}
