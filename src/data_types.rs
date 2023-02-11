//! This contains the code and structure for some of the fundamental
//! data types common to other components.
//!
use std::io::{Read, Write};

use num_derive::FromPrimitive;

use crate::error::TdmsError;

/// The DataTypeRaw enum's values match the binary representation of that
/// type in tdms files.
#[derive(Clone, Copy, Debug, FromPrimitive, PartialEq, Eq)]
#[repr(u32)]
pub enum DataType {
    Void = 0,
    I8 = 1,
    I16 = 2,
    I32 = 3,
    I64 = 4,
    U8 = 5,
    U16 = 6,
    U32 = 7,
    U64 = 8,
    SingleFloat = 9,
    DoubleFloat = 10,
    ExtendedFloat = 11,
    SingleFloatWithUnit = 0x19,
    DoubleFloatWithUnit = 12,
    ExtendedFloatWithUnit = 13,
    TdmsString = 0x20,
    Boolean = 0x21,
    TimeStamp = 0x44,
    FixedPoint = 0x4F,
    ComplexSingleFloat = 0x0008_000c,
    ComplexDoubleFloat = 0x0010_000d,
    DAQmxRawData = 0xFFFF_FFFF,
}

type StorageResult<T> = std::result::Result<T, TdmsError>;

pub trait TdmsStorageType: Sized {
    /// The [`DataType`] that can be read as this storage type.
    const SUPPORTED_TYPES: &'static [DataType];
    /// The [`DataType`] that this storage type is naturally written as.
    const NATURAL_TYPE: DataType;
    fn read_le(reader: &mut impl Read) -> StorageResult<Self>;
    fn read_be(reader: &mut impl Read) -> StorageResult<Self>;
    /// Write the value as little endian.
    fn write_le(&self, writer: &mut impl Write) -> StorageResult<()>;
    /// Write the value as big endian.
    fn write_be(&self, writer: &mut impl Write) -> StorageResult<()>;
    /// Report the size of the type to allow for planning of writes.
    fn size(&self) -> usize;

    fn supports_data_type(data_type: &DataType) -> bool {
        Self::SUPPORTED_TYPES.contains(&data_type)
    }
}

/// Macro for scripting the wrapping of the different read methods.
///
/// Should provide the type which has a from_le_bytes and from_be_bytes
/// Then the natural type for the storage type.
/// and then a slice of supported [`DataType`] values.
macro_rules! numeric_type {
    ($type:ty, $natural:expr, $supported:expr) => {
        impl TdmsStorageType for $type {
            const NATURAL_TYPE: DataType = $natural;
            const SUPPORTED_TYPES: &'static [DataType] = $supported;
            fn read_le(reader: &mut impl Read) -> StorageResult<$type> {
                let mut buf = [0u8; std::mem::size_of::<$type>()];
                reader.read_exact(&mut buf)?;
                Ok(<$type>::from_le_bytes(buf))
            }
            fn read_be(reader: &mut impl Read) -> StorageResult<$type> {
                let mut buf = [0u8; std::mem::size_of::<$type>()];
                reader.read_exact(&mut buf)?;
                Ok(<$type>::from_be_bytes(buf))
            }
            fn write_le(&self, writer: &mut impl Write) -> StorageResult<()> {
                writer.write_all(&self.to_le_bytes())?;
                Ok(())
            }
            fn write_be(&self, writer: &mut impl Write) -> StorageResult<()> {
                writer.write_all(&self.to_be_bytes())?;
                Ok(())
            }
            fn size(&self) -> usize {
                std::mem::size_of::<$type>()
            }
        }
    };
}

numeric_type!(u8, DataType::U8, &[DataType::U8]);
numeric_type!(i32, DataType::I32, &[DataType::I32]);
numeric_type!(u32, DataType::U32, &[DataType::U32]);
numeric_type!(u64, DataType::U64, &[DataType::U64]);
numeric_type!(
    f64,
    DataType::DoubleFloat,
    &[DataType::DoubleFloat, DataType::DoubleFloatWithUnit]
);
numeric_type!(
    f32,
    DataType::SingleFloat,
    &[DataType::SingleFloat, DataType::SingleFloatWithUnit]
);

fn read_string_with_length(reader: &mut impl Read, length: u32) -> Result<String, TdmsError> {
    let mut buffer = vec![0; length as usize];
    reader.read_exact(&mut buffer[..])?;
    let value = String::from_utf8(buffer)?;
    Ok(value)
}

impl TdmsStorageType for String {
    const SUPPORTED_TYPES: &'static [DataType] = &[DataType::TdmsString];

    fn read_le(reader: &mut impl Read) -> Result<Self, TdmsError> {
        let length = u32::read_le(reader)?;
        read_string_with_length(reader, length)
    }

    fn read_be(reader: &mut impl Read) -> Result<Self, TdmsError> {
        let length = u32::read_be(reader)?;
        read_string_with_length(reader, length)
    }

    const NATURAL_TYPE: DataType = DataType::TdmsString;

    fn write_le(&self, writer: &mut impl Write) -> StorageResult<()> {
        writer.write_all(&(self.len() as u32).to_le_bytes())?;
        writer.write_all(self.as_bytes())?;
        Ok(())
    }

    fn write_be(&self, writer: &mut impl Write) -> StorageResult<()> {
        writer.write_all(&(self.len() as u32).to_be_bytes())?;
        writer.write_all(self.as_bytes())?;
        Ok(())
    }

    fn size(&self) -> usize {
        self.len() + std::mem::size_of::<u32>()
    }
}

#[cfg(test)]
mod tests {
    use crate::reader::{BigEndianReader, LittleEndianReader, TdmsReader};
    use crate::writer::{BigEndianWriter, LittleEndianWriter, TdmsWriter};
    use std::io::Cursor;
    /// Tests the conversion against the le and be version for the value specified.
    macro_rules! test_formatting {
        ($type:ty, $test_value:literal) => {
            paste::item! {
                #[test]
                fn [< test_ $type _le >] () {
                    let original_value: $type = $test_value;
                    let bytes = original_value.to_le_bytes();
                    let mut reader = Cursor::new(bytes);
                    let mut tdms_reader = LittleEndianReader::from_reader(&mut reader);
                    let read_value: $type = tdms_reader.read_value().unwrap();
                    assert_eq!(read_value, original_value);

                    let mut output_bytes = [0u8; std::mem::size_of::<$type>()];
                    // block to limit writer lifetime.
                    {
                        let mut writer = LittleEndianWriter::from_writer(&mut output_bytes[..]);
                        writer.write_value(&original_value).unwrap();
                    }
                    assert_eq!(bytes, output_bytes);
                }

                #[test]
                fn [< test_ $type _be >] () {
                    let original_value: $type = $test_value;
                    let bytes = original_value.to_be_bytes();
                    let mut reader = Cursor::new(bytes);
                    let mut tdms_reader = BigEndianReader::from_reader(&mut reader);
                    let read_value: $type = tdms_reader.read_value().unwrap();
                    assert_eq!(read_value, original_value);

                    let mut output_bytes = [0u8; std::mem::size_of::<$type>()];
                    //block to limit writer lifetime.
                    {
                    let mut writer = BigEndianWriter::from_writer(&mut output_bytes[..]);
                    writer.write_value(&original_value).unwrap();
                    }
                    assert_eq!(bytes, output_bytes);
                }
            }
        };
    }

    test_formatting!(i32, -12345);
    test_formatting!(u32, 12345);
    test_formatting!(f64, 1234.1245);
}