use super::error::LLUdpError;
use math::{Quat, Vec3};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Frequency {
    High,
    Medium,
    Low,
    Fixed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MessageId {
    pub frequency: Frequency,
    pub number: u32,
}

impl MessageId {
    pub const fn high(number: u32) -> Self {
        Self { frequency: Frequency::High, number }
    }

    pub const fn medium(number: u32) -> Self {
        Self { frequency: Frequency::Medium, number }
    }

    pub const fn low(number: u32) -> Self {
        Self { frequency: Frequency::Low, number }
    }

    pub const fn fixed(number: u32) -> Self {
        Self { frequency: Frequency::Fixed, number }
    }

    pub(crate) fn encode(self, output: &mut Vec<u8>) -> Result<(), LLUdpError> {
        match self.frequency {
            Frequency::High if (1..=0xfe).contains(&self.number) => {
                output.push(self.number as u8);
            }
            Frequency::Medium if (1..=0xfe).contains(&self.number) => {
                output.extend_from_slice(&[0xff, self.number as u8]);
            }
            Frequency::Low if (1..=0xfff9).contains(&self.number) => {
                output.extend_from_slice(&[0xff, 0xff]);
                output.extend_from_slice(&(self.number as u16).to_be_bytes());
            }
            Frequency::Fixed if (0xffff_fffa..=0xffff_ffff).contains(&self.number) => {
                output.extend_from_slice(&self.number.to_be_bytes());
            }
            _ => return Err(LLUdpError::InvalidMessageId),
        }
        Ok(())
    }

    pub(crate) fn decode(input: &[u8]) -> Result<(Self, usize), LLUdpError> {
        let first = *input.first().ok_or(LLUdpError::TruncatedPacket)?;
        if first != 0xff {
            if first == 0 {
                return Err(LLUdpError::InvalidMessageId);
            }
            return Ok((Self::high(u32::from(first)), 1));
        }

        let second = *input.get(1).ok_or(LLUdpError::TruncatedPacket)?;
        if second != 0xff {
            if second == 0 {
                return Err(LLUdpError::InvalidMessageId);
            }
            return Ok((Self::medium(u32::from(second)), 2));
        }

        let third = *input.get(2).ok_or(LLUdpError::TruncatedPacket)?;
        let fourth = *input.get(3).ok_or(LLUdpError::TruncatedPacket)?;
        let number = u32::from_be_bytes([first, second, third, fourth]);
        if (0xffff_fffa..=0xffff_ffff).contains(&number) {
            return Ok((Self::fixed(number), 4));
        }

        let number = u32::from(u16::from_be_bytes([third, fourth]));
        if !(1..=0xfff9).contains(&number) {
            return Err(LLUdpError::InvalidMessageId);
        }
        Ok((Self::low(number), 4))
    }
}

pub(crate) fn quaternion_from_xyz(value: Vec3) -> Result<Quat, LLUdpError> {
    if !value.is_finite() {
        return Err(LLUdpError::InvalidMessage);
    }

    let squared = value.x * value.x + value.y * value.y + value.z * value.z;
    let remainder = 1.0 - squared;
    let w = if remainder > 0.0 { remainder.sqrt() } else { 0.0 };
    Ok(Quat::from_xyzw(value.x, value.y, value.z, w))
}

#[cfg(test)]
mod tests {
    use super::*;
    use math::{Quat, Vec3};

    #[test]
    fn quaternion_default_is_identity() {
        assert_eq!(Quat::default(), Quat::IDENTITY);
    }

    #[test]
    fn quaternion_w_uses_second_life_xyz_unpack_rule() {
        let identity = quaternion_from_xyz(Vec3::ZERO).expect("finite XYZ components");
        assert_eq!(identity, Quat::IDENTITY);

        let positive_w = quaternion_from_xyz(Vec3::new(0.6, 0.0, 0.0)).expect("finite XYZ components");
        assert!((positive_w.w - 0.8).abs() < f32::EPSILON);

        let outside_unit_sphere = quaternion_from_xyz(Vec3::new(0.6, 0.8, 0.0)).expect("finite XYZ components");
        assert_eq!(outside_unit_sphere.w, 0.0);
    }

    #[test]
    fn message_ids_use_frequency_specific_wire_format() {
        let cases = [
            (MessageId::high(4), vec![0x04]),
            (MessageId::medium(5), vec![0xff, 0x05]),
            (MessageId::low(0x0123), vec![0xff, 0xff, 0x01, 0x23]),
            (MessageId::low(0xfff9), vec![0xff, 0xff, 0xff, 0xf9]),
            (MessageId::fixed(0xffff_fffb), vec![0xff, 0xff, 0xff, 0xfb]),
        ];
        for (id, expected) in cases {
            let mut encoded = Vec::new();
            id.encode(&mut encoded).expect("valid message ID");
            assert_eq!(encoded, expected);
            assert_eq!(MessageId::decode(&expected), Ok((id, expected.len())));
        }
    }

    #[test]
    fn reserved_ids_are_rejected() {
        for id in [
            MessageId::high(0),
            MessageId::high(0xff),
            MessageId::medium(0),
            MessageId::medium(0xff),
            MessageId::low(0),
            MessageId::low(0xfffa),
            MessageId::fixed(0xffff_fff9),
        ] {
            let mut encoded = Vec::new();
            assert_eq!(id.encode(&mut encoded), Err(LLUdpError::InvalidMessageId));
        }
        assert_eq!(MessageId::decode(&[0xff]), Err(LLUdpError::TruncatedPacket));
        assert_eq!(MessageId::decode(&[0xff, 0xff, 0x00]), Err(LLUdpError::TruncatedPacket));
        assert_eq!(MessageId::decode(&[0xff, 0xff, 0x00, 0x00]), Err(LLUdpError::InvalidMessageId));
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChatType {
    Whisper,
    Normal,
    Shout,
    Say,
    StartTyping,
    StopTyping,
    Debug,
    OwnerSay,
    Unknown(u8),
}

impl ChatType {
    pub const fn from_wire(value: u8) -> Self {
        match value {
            0 => Self::Whisper,
            1 => Self::Normal,
            2 => Self::Shout,
            3 => Self::Say,
            4 => Self::StartTyping,
            5 => Self::StopTyping,
            6 => Self::Debug,
            8 => Self::OwnerSay,
            value => Self::Unknown(value),
        }
    }

    pub const fn to_wire(self) -> u8 {
        match self {
            Self::Whisper => 0,
            Self::Normal => 1,
            Self::Shout => 2,
            Self::Say => 3,
            Self::StartTyping => 4,
            Self::StopTyping => 5,
            Self::Debug => 6,
            Self::OwnerSay => 8,
            Self::Unknown(value) => value,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChatSourceType {
    System,
    Agent,
    Object,
    Unknown(u8),
}

impl ChatSourceType {
    pub const fn from_wire(value: u8) -> Self {
        match value {
            0 => Self::System,
            1 => Self::Agent,
            2 => Self::Object,
            value => Self::Unknown(value),
        }
    }

    pub const fn to_wire(self) -> u8 {
        match self {
            Self::System => 0,
            Self::Agent => 1,
            Self::Object => 2,
            Self::Unknown(value) => value,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Audible {
    Not,
    Barely,
    Fully,
    Unknown(u8),
}

impl Audible {
    pub const fn from_wire(value: u8) -> Self {
        match value {
            u8::MAX => Self::Not,
            0 => Self::Barely,
            1 => Self::Fully,
            value => Self::Unknown(value),
        }
    }

    pub const fn to_wire(self) -> u8 {
        match self {
            Self::Not => u8::MAX,
            Self::Barely => 0,
            Self::Fully => 1,
            Self::Unknown(value) => value,
        }
    }
}
