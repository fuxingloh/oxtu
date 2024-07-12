use std::fmt;
use std::ops::{AddAssign, SubAssign};

use bigdecimal::{BigDecimal, ToPrimitive};
use serde::{Deserialize, Serialize};

/// Unsigned 256-bit integer used to store 32 bytes of data. (e.g. hash, txid)
#[derive(Serialize, Deserialize, Hash, Eq, PartialEq, Clone, Copy)]
pub struct U256([u8; 32]);

impl U256 {
    /// To Hex String without 0x prefix
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn from_hex(hex: &str) -> Self {
        let bytes = hex::decode(hex).unwrap();
        Self(bytes.try_into().unwrap())
    }

    pub fn zero() -> Self {
        const ZERO: [u8; 32] = [0; 32];
        Self(ZERO)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl From<U256> for [u8; 32] {
    fn from(value: U256) -> Self {
        value.0
    }
}

impl From<[u8; 32]> for U256 {
    fn from(value: [u8; 32]) -> Self {
        Self(value)
    }
}

impl fmt::Debug for U256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

impl fmt::Display for U256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

impl Default for U256 {
    fn default() -> Self {
        Self::zero()
    }
}

/// To make this implementation compatible with forked bitcoin-like chains,
/// we store value as (u128, exponent as u8) to avoid precision loss and ensure space efficiency.
/// This should be "compatible enough" with most chains without introducing another data type.
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct U128Decimal(pub u128, pub u8);

impl From<BigDecimal> for U128Decimal {
    fn from(value: BigDecimal) -> Self {
        let (number, exponent) = value.into_bigint_and_exponent();
        Self(number.to_u128().unwrap(), exponent.to_u8().unwrap())
    }
}

impl From<U128Decimal> for BigDecimal {
    fn from(value: U128Decimal) -> Self {
        BigDecimal::new(value.0.into(), value.1.into())
    }
}

impl U128Decimal {
    pub fn is_zero(&self) -> bool {
        self.0 == 0
    }

    pub const fn zero() -> Self {
        Self(0, 0)
    }
}

impl AddAssign for U128Decimal {
    fn add_assign(&mut self, rhs: Self) {
        let scale = self.1.max(rhs.1);
        if self.1 < scale {
            // Current scale is smaller than the target scale
            self.0 *= 10u128.pow((scale - self.1) as u32);
            self.0 += rhs.0;
        } else if rhs.1 < scale {
            // Current scale is larger than the target scale
            let rhs = rhs.0 * 10u128.pow((scale - rhs.1) as u32);
            self.0 += rhs;
        } else {
            // Both scales are equal
            self.0 += rhs.0;
        }
        self.1 = scale;
    }
}

impl SubAssign for U128Decimal {
    fn sub_assign(&mut self, rhs: Self) {
        let scale = self.1.max(rhs.1);
        if self.1 < scale {
            // Current scale is smaller than the target scale
            self.0 *= 10u128.pow((scale - self.1) as u32);
            self.0 -= rhs.0;
        } else if rhs.1 < scale {
            // Current scale is larger than the target scale
            let rhs = rhs.0 * 10u128.pow((scale - rhs.1) as u32);
            self.0 -= rhs;
        } else {
            // Both scales are equal
            self.0 -= rhs.0;
        }
        self.1 = scale;
    }
}
