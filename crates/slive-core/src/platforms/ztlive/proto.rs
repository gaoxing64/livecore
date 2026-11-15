//! 最小 protobuf 编解码（无 build.rs / 无 protoc）。
//!
//! 只覆盖 ZtLive link IM 用到的线格式：varint、长度前缀（string/bytes/嵌套消息）。
//! 解码丢弃未知字段，编码不写默认值 —— 对齐 protobufjs 生成代码的行为。

use crate::error::SliveError;

/// 递增写 protobuf 字段。
#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    /// 空写入器。
    pub fn new() -> Self {
        Self::default()
    }

    fn varint(&mut self, mut value: u64) {
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            self.buf.push(byte);
            if value == 0 {
                break;
            }
        }
    }

    fn key(&mut self, field: u32, wire: u8) {
        self.varint((u64::from(field) << 3) | u64::from(wire));
    }

    /// 字符串字段（wire type 2）。
    pub fn string(&mut self, field: u32, value: &str) {
        self.bytes(field, value.as_bytes());
    }

    /// bytes 字段（wire type 2）。
    pub fn bytes(&mut self, field: u32, value: &[u8]) {
        self.key(field, 2);
        self.varint(value.len() as u64);
        self.buf.extend_from_slice(value);
    }

    /// uint64 / int64 字段（wire type 0）。
    pub fn uint64(&mut self, field: u32, value: u64) {
        self.key(field, 0);
        self.varint(value);
    }

    /// uint32 字段（wire type 0）。
    pub fn uint32(&mut self, field: u32, value: u32) {
        self.uint64(field, u64::from(value));
    }

    /// bool 字段（wire type 0）。
    pub fn boolean(&mut self, field: u32, value: bool) {
        self.uint64(field, u64::from(value));
    }

    /// 嵌套消息字段：`build` 写入子消息体。
    pub fn message(&mut self, field: u32, build: impl FnOnce(&mut Writer)) {
        let mut inner = Writer::new();
        build(&mut inner);
        self.bytes(field, &inner.buf);
    }

    /// 取出编码结果。
    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

/// 字段值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value<'a> {
    /// wire type 0。
    Varint(u64),
    /// wire type 2（string / bytes / 嵌套消息）。
    Bytes(&'a [u8]),
    /// wire type 5。
    Fixed32(&'a [u8]),
    /// wire type 1。
    Fixed64(&'a [u8]),
}

/// 解码出的单个字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Field<'a> {
    /// 字段号。
    pub number: u32,
    /// 字段值。
    pub value: Value<'a>,
}

impl<'a> Field<'a> {
    /// varint 值。
    pub fn as_u64(&self) -> Option<u64> {
        match self.value {
            Value::Varint(value) => Some(value),
            _ => None,
        }
    }

    /// 长度前缀字节。
    pub fn as_bytes(&self) -> Option<&'a [u8]> {
        match self.value {
            Value::Bytes(value) => Some(value),
            _ => None,
        }
    }

    /// UTF-8 字符串（非法 UTF-8 返回 `None`）。
    pub fn as_str(&self) -> Option<&'a str> {
        self.as_bytes()
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
    }

    /// 非空 UTF-8 字符串。
    pub fn as_string(&self) -> Option<String> {
        self.as_str().map(str::to_string)
    }
}

fn read_varint(buf: &[u8], pos: &mut usize) -> Result<u64, SliveError> {
    let mut value = 0u64;
    let mut shift = 0u32;
    loop {
        let Some(&byte) = buf.get(*pos) else {
            return Err(SliveError::decode("protobuf: varint 截断"));
        };
        *pos += 1;
        if shift > 63 {
            return Err(SliveError::decode("protobuf: varint 溢出"));
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
    }
}

fn read_len(buf: &[u8], pos: &mut usize) -> Result<usize, SliveError> {
    let len = read_varint(buf, pos)?;
    usize::try_from(len).map_err(|_| SliveError::decode("protobuf: 长度溢出"))
}

/// 解码一段消息体为字段列表。
pub fn parse_fields(buf: &[u8]) -> Result<Vec<Field<'_>>, SliveError> {
    let mut fields = Vec::new();
    let mut pos = 0usize;
    while pos < buf.len() {
        let key = read_varint(buf, &mut pos)?;
        let number =
            u32::try_from(key >> 3).map_err(|_| SliveError::decode("protobuf: 字段号溢出"))?;
        if number == 0 {
            return Err(SliveError::decode("protobuf: 字段号为 0"));
        }
        let value = match key & 0x7 {
            0 => Value::Varint(read_varint(buf, &mut pos)?),
            2 => {
                let len = read_len(buf, &mut pos)?;
                let end = pos
                    .checked_add(len)
                    .filter(|end| *end <= buf.len())
                    .ok_or_else(|| SliveError::decode("protobuf: 长度前缀越界"))?;
                let slice = &buf[pos..end];
                pos = end;
                Value::Bytes(slice)
            }
            5 => {
                let end = pos
                    .checked_add(4)
                    .filter(|end| *end <= buf.len())
                    .ok_or_else(|| SliveError::decode("protobuf: fixed32 越界"))?;
                let slice = &buf[pos..end];
                pos = end;
                Value::Fixed32(slice)
            }
            1 => {
                let end = pos
                    .checked_add(8)
                    .filter(|end| *end <= buf.len())
                    .ok_or_else(|| SliveError::decode("protobuf: fixed64 越界"))?;
                let slice = &buf[pos..end];
                pos = end;
                Value::Fixed64(slice)
            }
            wire => {
                return Err(SliveError::decode(format!(
                    "protobuf: 不支持的线格式 {wire}"
                )))
            }
        };
        fields.push(Field { number, value });
    }
    Ok(fields)
}

/// 首个指定字段。
pub fn find<'a>(fields: &'a [Field<'a>], number: u32) -> Option<&'a Field<'a>> {
    fields.iter().find(|field| field.number == number)
}

/// 指定字段的全部出现。
pub fn find_all<'a>(fields: &'a [Field<'a>], number: u32) -> impl Iterator<Item = &'a Field<'a>> {
    fields.iter().filter(move |field| field.number == number)
}

/// 字符串字段。
pub fn get_string(fields: &[Field<'_>], number: u32) -> Option<String> {
    find(fields, number).and_then(Field::as_string)
}

/// varint 字段。
pub fn get_u64(fields: &[Field<'_>], number: u32) -> Option<u64> {
    find(fields, number).and_then(Field::as_u64)
}

/// 嵌套消息字段。
pub fn get_message<'a>(fields: &'a [Field<'a>], number: u32) -> Option<Vec<Field<'a>>> {
    let bytes = find(fields, number).and_then(Field::as_bytes)?;
    parse_fields(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_roundtrips_nested_scalars_and_bytes() {
        let mut writer = Writer::new();
        writer.string(1, "hello");
        writer.uint64(2, 42);
        writer.boolean(3, true);
        writer.message(4, |inner| {
            inner.string(1, "nested");
            inner.uint32(2, 7);
        });
        let bytes = writer.finish();
        let fields = parse_fields(&bytes).expect("解析");
        assert_eq!(get_string(&fields, 1).as_deref(), Some("hello"));
        assert_eq!(get_u64(&fields, 2), Some(42));
        assert_eq!(get_u64(&fields, 3), Some(1));
        let nested = get_message(&fields, 4).expect("嵌套消息");
        assert_eq!(get_string(&nested, 1).as_deref(), Some("nested"));
        assert_eq!(get_u64(&nested, 2), Some(7));
    }

    #[test]
    fn unknown_fields_are_skipped() {
        let mut writer = Writer::new();
        writer.uint64(9, 5);
        writer.string(1, "value");
        let bytes = writer.finish();
        let fields = parse_fields(&bytes).expect("解析");
        assert_eq!(get_string(&fields, 1).as_deref(), Some("value"));
    }

    #[test]
    fn truncated_length_prefix_is_rejected() {
        let bytes = [0x0a, 0x05, b'a'];
        assert!(parse_fields(&bytes).is_err());
    }

    #[test]
    fn zero_field_number_is_rejected() {
        assert!(parse_fields(&[0x00]).is_err());
    }

    #[test]
    fn multibyte_varint_roundtrips() {
        let mut writer = Writer::new();
        writer.uint64(1, u64::MAX);
        writer.uint64(2, 300);
        let bytes = writer.finish();
        let fields = parse_fields(&bytes).expect("解析");
        assert_eq!(get_u64(&fields, 1), Some(u64::MAX));
        assert_eq!(get_u64(&fields, 2), Some(300));
    }
}
