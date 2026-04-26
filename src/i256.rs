/// 256-bit signed integer for overflow-free rational arithmetic.
/// Represented as sign + two u128 limbs.
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct i256 {
    pub(crate) negative: bool,
    pub(crate) limbs: [u128; 2], // [0] = lower, [1] = upper
}

impl i256 {
    pub(crate) fn zero() -> Self {
        i256 {
            negative: false,
            limbs: [0, 0],
        }
    }

    pub(crate) fn from_i128(n: i128) -> Self {
        i256 {
            negative: n < 0,
            limbs: [n.unsigned_abs(), 0],
        }
    }

    pub(crate) fn is_zero(&self) -> bool {
        self.limbs[0] == 0 && self.limbs[1] == 0
    }

    pub(crate) fn to_i128(self) -> Option<i128> {
        if self.limbs[1] != 0 || self.limbs[0] > i128::MAX as u128 {
            return None;
        }
        let val = self.limbs[0] as i128;
        Some(if self.negative { -val } else { val })
    }

    pub(crate) fn abs(&self) -> Self {
        i256 {
            negative: false,
            ..*self
        }
    }

    pub(crate) fn negate(&self) -> Self {
        if self.is_zero() {
            return *self;
        }
        i256 {
            negative: !self.negative,
            ..*self
        }
    }

    pub(crate) fn add(&self, rhs: &i256) -> i256 {
        if self.negative == rhs.negative {
            let (lo, carry) = self.limbs[0].overflowing_add(rhs.limbs[0]);
            let hi = self.limbs[1] + rhs.limbs[1] + u128::from(carry);
            i256 {
                negative: self.negative,
                limbs: [lo, hi],
            }
        } else {
            match cmp_mag(self.limbs, rhs.limbs) {
                std::cmp::Ordering::Equal => i256::zero(),
                std::cmp::Ordering::Greater => {
                    let (lo, borrow) = self.limbs[0].overflowing_sub(rhs.limbs[0]);
                    let hi = self.limbs[1] - rhs.limbs[1] - u128::from(borrow);
                    i256 {
                        negative: self.negative,
                        limbs: [lo, hi],
                    }
                }
                std::cmp::Ordering::Less => {
                    let (lo, borrow) = rhs.limbs[0].overflowing_sub(self.limbs[0]);
                    let hi = rhs.limbs[1] - self.limbs[1] - u128::from(borrow);
                    i256 {
                        negative: rhs.negative,
                        limbs: [lo, hi],
                    }
                }
            }
        }
    }

    /// Full i256 × i256 → i256 multiplication.
    /// Handles operands with non-zero upper limbs.
    pub(crate) fn mul(&self, rhs: &i256) -> i256 {
        let neg = self.negative != rhs.negative;

        // a = a1*2^128 + a0, b = b1*2^128 + b0
        // a*b = (a1*b0 + a0*b1)*2^128 + a0*b0
        // (a1*b1*2^256 is overflow — only possible with very large values)
        let p00 = mul_u128(self.limbs[0], rhs.limbs[0]);
        let cross = self.limbs[0]
            .wrapping_mul(rhs.limbs[1])
            .wrapping_add(self.limbs[1].wrapping_mul(rhs.limbs[0]));

        let limbs = [p00[0], p00[1].wrapping_add(cross)];

        i256 {
            negative: neg && (limbs[0] != 0 || limbs[1] != 0),
            limbs,
        }
    }

    pub(crate) fn divmod(&self, rhs: &i256) -> (i256, i256) {
        assert!(!rhs.is_zero(), "division by zero");
        if self.is_zero() {
            return (i256::zero(), i256::zero());
        }

        let neg_q = self.negative != rhs.negative;
        let neg_r = self.negative;
        let (q, r) = divmod_u256(self.limbs, rhs.limbs);

        (
            i256 {
                negative: neg_q && (q[0] != 0 || q[1] != 0),
                limbs: q,
            },
            i256 {
                negative: neg_r && (r[0] != 0 || r[1] != 0),
                limbs: r,
            },
        )
    }

    pub(crate) fn cmp_signed(&self, other: &Self) -> std::cmp::Ordering {
        match (self.negative, other.negative) {
            (true, false) if !self.is_zero() || !other.is_zero() => std::cmp::Ordering::Less,
            (false, true) if !self.is_zero() || !other.is_zero() => std::cmp::Ordering::Greater,
            (false, false) => cmp_mag(self.limbs, other.limbs),
            (true, true) => cmp_mag(other.limbs, self.limbs),
            _ => std::cmp::Ordering::Equal,
        }
    }

}

fn cmp_mag(a: [u128; 2], b: [u128; 2]) -> std::cmp::Ordering {
    match a[1].cmp(&b[1]) {
        std::cmp::Ordering::Equal => a[0].cmp(&b[0]),
        other => other,
    }
}

/// Multiply two u128 → [lo, hi].
fn mul_u128(a: u128, b: u128) -> [u128; 2] {
    let a0 = a & 0xFFFFFFFFFFFFFFFF;
    let a1 = a >> 64;
    let b0 = b & 0xFFFFFFFFFFFFFFFF;
    let b1 = b >> 64;

    let p00 = a0 * b0;
    let p01 = a0 * b1;
    let p10 = a1 * b0;
    let p11 = a1 * b1;

    let mid = p01 + (p00 >> 64);
    let (mid, carry) = mid.overflowing_add(p10);

    let lo = (p00 & 0xFFFFFFFFFFFFFFFF) | (mid << 64);
    let hi = p11 + (mid >> 64) + if carry { 1_u128 << 64 } else { 0 };

    [lo, hi]
}

/// Binary long division for unsigned 256-bit values.
fn divmod_u256(a: [u128; 2], b: [u128; 2]) -> ([u128; 2], [u128; 2]) {
    if a[1] == 0 && b[1] == 0 {
        return ([a[0] / b[0], 0], [a[0] % b[0], 0]);
    }

    let mut q = [0_u128; 2];
    let mut r = [0_u128; 2];

    for i in (0..256).rev() {
        r[1] = (r[1] << 1) | (r[0] >> 127);
        r[0] <<= 1;

        let bit = if i >= 128 {
            (a[1] >> (i - 128)) & 1
        } else {
            (a[0] >> i) & 1
        };
        r[0] |= bit;

        if r[1] > b[1] || (r[1] == b[1] && r[0] >= b[0]) {
            let (new_lo, borrow) = r[0].overflowing_sub(b[0]);
            r[1] = r[1] - b[1] - u128::from(borrow);
            r[0] = new_lo;

            if i >= 128 {
                q[1] |= 1 << (i - 128);
            } else {
                q[0] |= 1 << i;
            }
        }
    }

    (q, r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip() {
        assert_eq!(i256::from_i128(42).to_i128(), Some(42));
        assert_eq!(i256::from_i128(-100).to_i128(), Some(-100));
        assert_eq!(i256::from_i128(0).to_i128(), Some(0));
    }

    #[test]
    fn test_add() {
        let c = i256::from_i128(100).add(&i256::from_i128(200));
        assert_eq!(c.to_i128(), Some(300));
    }

    #[test]
    fn test_add_different_signs() {
        assert_eq!(
            i256::from_i128(100).add(&i256::from_i128(-300)).to_i128(),
            Some(-200)
        );
    }

    #[test]
    fn test_mul_small() {
        assert_eq!(
            i256::from_i128(123).mul(&i256::from_i128(456)).to_i128(),
            Some(56088)
        );
    }

    #[test]
    fn test_mul_overflow_i128() {
        let a = i256::from_i128(i128::MAX / 2);
        let c = a.mul(&i256::from_i128(4));
        assert!(c.to_i128().is_none());
        assert!(!c.is_zero());
    }

    #[test]
    fn test_mul_with_upper_limb() {
        // Construct a value with upper limb directly
        let big = i256 {
            negative: false,
            limbs: [42, 1],
        };
        let result = big.mul(&i256::from_i128(1));
        assert_eq!(result, big);
    }

    #[test]
    fn test_divmod() {
        let (q, r) = i256::from_i128(17).divmod(&i256::from_i128(5));
        assert_eq!(q.to_i128(), Some(3));
        assert_eq!(r.to_i128(), Some(2));
    }

    #[test]
    fn test_negate() {
        assert_eq!(i256::from_i128(5).negate().to_i128(), Some(-5));
        assert_eq!(i256::from_i128(-5).negate().to_i128(), Some(5));
        assert_eq!(i256::zero().negate(), i256::zero());
    }

    #[test]
    fn test_cmp_signed() {
        let pos = i256::from_i128(10);
        let neg = i256::from_i128(-10);
        let zero = i256::zero();
        assert_eq!(pos.cmp_signed(&neg), std::cmp::Ordering::Greater);
        assert_eq!(neg.cmp_signed(&pos), std::cmp::Ordering::Less);
        assert_eq!(zero.cmp_signed(&zero), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_mul_u128_roundtrip() {
        let a = 100_000_000_000_000_000_000_u128;
        let product = mul_u128(a, a);
        assert!(product[1] > 0);
        let (q, r) = divmod_u256(product, [a, 0]);
        assert_eq!(q, [a, 0]);
        assert_eq!(r, [0, 0]);
    }
}
