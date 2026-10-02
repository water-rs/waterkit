//! GF(2⁸) arithmetic and Reed–Solomon error correction for the QR field
//! (primitive polynomial `x⁸ + x⁴ + x³ + x² + 1`, `0x11D`, generator base 0).
//!
//! Polynomials are stored as `Vec<u8>` in *ascending* degree order:
//! `p[i]` is the coefficient of `xⁱ`. Leading zero coefficients are
//! trimmed before degree checks.

#![allow(
    clippy::cast_possible_truncation,
    clippy::many_single_char_names,
    clippy::missing_const_for_fn
)]

/// Exponential/logarithm tables for the field. `exp[i]` is `αⁱ` for
/// `i` in `0..510` (the table is doubled so `exp[log(a) + log(b)]`
/// never needs a modulo).
struct Tables {
    exp: [u8; 510],
    log: [u8; 256],
}

const fn build() -> Tables {
    let mut exp = [0u8; 510];
    let mut log = [0u8; 256];
    let mut a: u16 = 1;
    let mut i = 0usize;
    while i < 255 {
        exp[i] = a as u8;
        log[a as usize] = i as u8;
        a <<= 1;
        if a & 0x100 != 0 {
            a ^= 0x11D;
        }
        i += 1;
    }
    while i < 510 {
        exp[i] = exp[i - 255];
        i += 1;
    }
    Tables { exp, log }
}

static GF: Tables = build();

/// `αⁱ` (exponent may exceed 254 — the doubled table absorbs one wrap).
pub fn exp(i: u32) -> u8 {
    GF.exp[(i % 255) as usize]
}

/// `log_α(v)`; `v` must be nonzero.
pub fn log(v: u8) -> u8 {
    GF.log[v as usize]
}

/// Field multiplication.
pub fn mul(a: u8, b: u8) -> u8 {
    if a == 0 || b == 0 {
        0
    } else {
        GF.exp[GF.log[a as usize] as usize + GF.log[b as usize] as usize]
    }
}

/// Field division `a / b`; `b` must be nonzero.
pub fn div(a: u8, b: u8) -> u8 {
    debug_assert!(b != 0);
    if a == 0 {
        0
    } else {
        GF.exp[(GF.log[a as usize] as usize + 255 - GF.log[b as usize] as usize) % 255]
    }
}

/// Multiplicative inverse; `v` must be nonzero.
pub fn inv(v: u8) -> u8 {
    debug_assert!(v != 0);
    GF.exp[255 - GF.log[v as usize] as usize]
}

/// Drop trailing (highest-degree) zero coefficients.
fn trim(p: &mut Vec<u8>) {
    while p.len() > 1 && p[p.len() - 1] == 0 {
        p.pop();
    }
}

/// Degree of a polynomial (the zero polynomial has degree 0).
fn degree(p: &[u8]) -> usize {
    let mut d = p.len();
    while d > 1 && p[d - 1] == 0 {
        d -= 1;
    }
    d - 1
}

/// Evaluate `p(x)` with Horner's rule.
fn eval(p: &[u8], x: u8) -> u8 {
    let mut acc = 0u8;
    for &c in p.iter().rev() {
        acc = mul(acc, x) ^ c;
    }
    acc
}

/// `c(x) - scale · x^shift · b(x)` (subtraction is XOR in GF(2)).
fn sub_scaled(c: &mut Vec<u8>, b: &[u8], scale: u8, shift: usize) {
    for (i, &bb) in b.iter().enumerate() {
        let idx = i + shift;
        if idx >= c.len() {
            c.resize(idx + 1, 0);
        }
        c[idx] ^= mul(bb, scale);
    }
}

/// Polynomial product.
fn product(a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; a.len() + b.len() - 1];
    for (i, &aa) in a.iter().enumerate() {
        for (j, &bb) in b.iter().enumerate() {
            out[i + j] ^= mul(aa, bb);
        }
    }
    out
}

/// Correct `block` in place against `ec_len` trailing check symbols.
///
/// Returns the number of corrected symbols, or `None` when the damage
/// exceeds the block's correction capacity.
pub fn correct(block: &mut [u8], ec_len: usize) -> Option<u32> {
    let n = block.len();
    // r(x) in ascending-degree form: block[p] is the coefficient of x^{n-1-p}.
    let mut r: Vec<u8> = block.iter().rev().copied().collect();

    // Syndromes S_j = r(α^j), j = 0..ec_len.
    let syndromes: Vec<u8> = (0..ec_len as u32).map(|j| eval(&r, exp(j))).collect();
    if syndromes.iter().all(|&s| s == 0) {
        return Some(0);
    }

    // Berlekamp–Massey: find the error locator σ(x).
    let mut sigma = vec![1u8];
    let mut prev = vec![1u8];
    let (mut l, mut m, mut b) = (0usize, 1usize, 1u8);
    for (idx, &s_n) in syndromes.iter().enumerate() {
        let mut d = s_n;
        for i in 1..=l.min(sigma.len() - 1) {
            // Coefficients beyond sigma's length are zero.
            d ^= mul(sigma[i], syndromes[idx - i]);
        }
        if d == 0 {
            m += 1;
        } else if 2 * l <= idx {
            let t = sigma.clone();
            sub_scaled(&mut sigma, &prev, div(d, b), m);
            l = idx + 1 - l;
            prev = t;
            b = d;
            m = 1;
        } else {
            sub_scaled(&mut sigma, &prev, div(d, b), m);
            m += 1;
        }
    }
    trim(&mut sigma);
    let errors = degree(&sigma);
    if errors == 0 || errors > ec_len / 2 {
        return None;
    }

    // Error evaluator Ω(x) = σ(x)·S(x) mod x^{ec_len}.
    let mut omega = product(&sigma, &syndromes);
    omega.truncate(ec_len);

    // Chien search: roots of σ are the inverse locators X⁻¹ = α^{-e}
    // where e is the exponent of x for the errored coefficient.
    let mut roots: Vec<u8> = Vec::with_capacity(errors);
    for i in 0..255u32 {
        let x = exp(i);
        if eval(&sigma, x) == 0 {
            roots.push(x);
        }
    }
    if roots.len() != errors {
        return None;
    }

    // Forney: Y_i = Ω(X⁻¹) / Π_{j≠i}(1 - X_j·X⁻¹)  (generator base 0).
    for &r_i in &roots {
        let x_i = inv(r_i);
        let mut denom = 1u8;
        for &r_j in &roots {
            if r_j != r_i {
                denom = mul(denom, 1 ^ mul(inv(r_j), r_i));
            }
        }
        if denom == 0 {
            return None;
        }
        let magnitude = div(eval(&omega, r_i), denom);
        let e = log(x_i) as usize; // exponent of x for this error
        if e >= n {
            return None;
        }
        r[e] ^= magnitude;
    }

    // Verify the correction: recomputed syndromes must vanish.
    if (0..ec_len as u32).any(|j| eval(&r, exp(j)) != 0) {
        return None;
    }

    block.copy_from_slice(&r.iter().rev().copied().collect::<Vec<u8>>());
    Some(errors as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generator polynomial with roots α⁰..α^{ec-1} (ascending-degree form).
    fn generator(ec_len: usize) -> Vec<u8> {
        let mut g = vec![1u8];
        for i in 0..ec_len as u32 {
            g = product(&g, &[exp(i), 1]); // (x - α^i) == (x + α^i)
        }
        g
    }

    /// Encode `data` appending `ec_len` check symbols (systematic encoder).
    fn encode(data: &[u8], ec_len: usize) -> Vec<u8> {
        let g = generator(ec_len);
        let mut msg = data.to_vec();
        msg.resize(data.len() + ec_len, 0);
        // Long division of msg·x^{ec} by g: `msg[i]` is the coefficient
        // of x^{n-1-i}, so g is walked descending (leading term first).
        for i in 0..data.len() {
            let coef = msg[i];
            if coef != 0 {
                for (j, &gc) in g.iter().rev().enumerate() {
                    msg[i + j] ^= mul(coef, gc);
                }
            }
        }
        let mut out = data.to_vec();
        out.extend_from_slice(&msg[data.len()..]);
        out
    }

    #[test]
    fn field_tables_are_consistent() {
        for i in 0..255u32 {
            assert_eq!(log(exp(i)), i as u8);
        }
        for v in 1..=255u8 {
            assert_eq!(mul(v, inv(v)), 1, "inverse failed for {v:#04x}");
            assert_eq!(div(v, v), 1);
        }
    }

    #[test]
    fn corrects_within_capacity() {
        // v1-L block: 19 data + 7 ec codewords.
        let data: Vec<u8> = (0u8..19)
            .map(|i| i.wrapping_mul(37).wrapping_add(11))
            .collect();
        let mut block = encode(&data, 7);
        let clean = block.clone();
        // Corrupt three codewords (capacity is ⌊7/2⌋ = 3).
        block[2] ^= 0xAB;
        block[9] ^= 0x14;
        block[25] ^= 0x77;
        assert_eq!(correct(&mut block, 7), Some(3));
        assert_eq!(block, clean);
    }

    #[test]
    fn rejects_beyond_capacity() {
        let data: Vec<u8> = (0u8..19)
            .map(|i| i.wrapping_mul(11).wrapping_add(3))
            .collect();
        let mut block = encode(&data, 7);
        for i in [1, 5, 9, 13] {
            block[i] ^= 0xC3;
        }
        assert!(correct(&mut block, 7).is_none());
    }

    #[test]
    fn clean_block_needs_no_correction() {
        let data: Vec<u8> = (0u8..16)
            .map(|i| i.wrapping_mul(7).wrapping_add(1))
            .collect();
        let mut block = encode(&data, 10);
        assert_eq!(correct(&mut block, 10), Some(0));
    }
}
