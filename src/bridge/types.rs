/// PHP value type (stable across PHP versions, mapped from IS_* constants).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValType {
    Null = 0,
    False = 1,
    True = 2,
    Long = 3,
    Double = 4,
    String = 5,
    Array = 6,
    Object = 7,
    Resource = 8,
}

impl ValType {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => ValType::False,
            2 => ValType::True,
            3 => ValType::Long,
            4 => ValType::Double,
            5 => ValType::String,
            6 => ValType::Array,
            7 => ValType::Object,
            8 => ValType::Resource,
            _ => ValType::Null,
        }
    }

    pub fn is_bool(self) -> bool {
        matches!(self, ValType::True | ValType::False)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_val_type_from_u8() {
        assert_eq!(ValType::from_u8(0), ValType::Null);
        assert_eq!(ValType::from_u8(1), ValType::False);
        assert_eq!(ValType::from_u8(2), ValType::True);
        assert_eq!(ValType::from_u8(3), ValType::Long);
        assert_eq!(ValType::from_u8(4), ValType::Double);
        assert_eq!(ValType::from_u8(5), ValType::String);
        assert_eq!(ValType::from_u8(6), ValType::Array);
        assert_eq!(ValType::from_u8(7), ValType::Object);
        assert_eq!(ValType::from_u8(8), ValType::Resource);
        assert_eq!(ValType::from_u8(255), ValType::Null);
    }

    #[test]
    fn test_val_type_is_bool() {
        assert!(ValType::True.is_bool());
        assert!(ValType::False.is_bool());
        assert!(!ValType::Long.is_bool());
        assert!(!ValType::Null.is_bool());
    }
}
