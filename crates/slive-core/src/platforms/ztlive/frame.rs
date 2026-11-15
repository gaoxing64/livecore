//! link IM 帧格式：`12 字节头 + PacketHeader + 加密载荷`。
//!
//! - 头：magic `0xABCD`（大端）、version `1`、PacketHeader 长度、载荷长度；
//! - 载荷：`UpstreamPayload`（客户端）或 `DownstreamPayload`（服务端），
//!   按 header 的 `encryptionMode` 用服务 token 或会话密钥做 AES-CBC；
//! - `seqId` 由客户端递增，服务端响应回填同一个值。

use crate::error::SliveError;

use super::crypto;
use super::proto::{find, get_message, get_string, get_u64, parse_fields, Field, Writer};

/// 帧 magic（大端 `0xABCD`）。
pub const MAGIC: u16 = 0xABCD;
/// 帧版本。
pub const VERSION: u16 = 1;
/// 帧固定头长度。
pub const FRAME_HEADER_LEN: usize = 12;

/// 载荷加密方式（`PacketHeader.encryptionMode`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptionMode {
    /// 明文。
    None,
    /// 用服务 token 阶段的 `ssecurity` 加密（注册帧）。
    ServiceToken,
    /// 用 `RegisterResponse.sessKey` 加密（其余全部帧）。
    SessionKey,
}

impl EncryptionMode {
    fn from_u64(value: u64) -> Self {
        match value {
            1 => Self::ServiceToken,
            2 => Self::SessionKey,
            _ => Self::None,
        }
    }

    fn as_u64(self) -> u64 {
        match self {
            Self::None => 0,
            Self::ServiceToken => 1,
            Self::SessionKey => 2,
        }
    }
}

/// 客户端 PacketHeader。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacketHeader {
    /// 应用 id（AcFun=13）。
    pub app_id: i32,
    /// 用户 id（匿名 visitor 也有）。
    pub uid: u64,
    /// 实例 id（注册前为 0，之后取 RegisterResponse）。
    pub instance_id: u64,
    /// 明文载荷长度。
    pub decoded_payload_len: u32,
    /// 加密方式。
    pub encryption_mode: EncryptionMode,
    /// 序列号。
    pub seq_id: u64,
    /// 应用 key（`ACFUN_APP`）。
    pub kpn: String,
    /// 服务 token（仅 `ServiceToken` 阶段携带）。
    pub service_token: Option<Vec<u8>>,
}

/// 服务端 DownstreamPayload 解出的帧。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownstreamFrame {
    /// 帧头。
    pub header: PacketHeader,
    /// 业务命令（`Basic.Register` / `Global.ZtLiveInteractive.CsCmd` / `Push.*`）。
    pub command: String,
    /// 错误码（0 成功）。
    pub error_code: i32,
    /// 业务载荷。
    pub payload: Vec<u8>,
    /// 错误信息。
    pub error_message: String,
}

fn encode_header(header: &PacketHeader) -> Vec<u8> {
    let mut writer = Writer::new();
    writer.uint32(1, header.app_id as u32);
    writer.uint64(2, header.uid);
    writer.uint64(3, header.instance_id);
    writer.uint32(7, header.decoded_payload_len);
    writer.uint32(8, header.encryption_mode.as_u64() as u32);
    if let Some(token) = header.service_token.as_deref() {
        writer.message(9, |inner| {
            inner.uint32(1, 1); // kServiceToken
            inner.bytes(2, token);
        });
    }
    writer.uint64(10, header.seq_id);
    writer.string(12, &header.kpn);
    writer.finish()
}

fn decode_header(bytes: &[u8]) -> Result<PacketHeader, SliveError> {
    let fields = parse_fields(bytes)?;
    let mut header = PacketHeader {
        app_id: get_u64(&fields, 1).unwrap_or(0) as i32,
        uid: get_u64(&fields, 2).unwrap_or(0),
        instance_id: get_u64(&fields, 3).unwrap_or(0),
        decoded_payload_len: get_u64(&fields, 7).unwrap_or(0) as u32,
        encryption_mode: EncryptionMode::from_u64(get_u64(&fields, 8).unwrap_or(0)),
        seq_id: get_u64(&fields, 10).unwrap_or(0),
        kpn: get_string(&fields, 12).unwrap_or_default(),
        service_token: None,
    };
    if let Some(token_info) = get_message(&fields, 9) {
        header.service_token = find(&token_info, 2)
            .and_then(Field::as_bytes)
            .map(<[u8]>::to_vec);
    }
    Ok(header)
}

/// 编码一个客户端帧（UpstreamPayload）。
pub fn encode_upstream(
    header: &PacketHeader,
    key: &[u8],
    command: &str,
    seq_id: u64,
    sub_biz: &str,
    payload: &[u8],
) -> Result<Vec<u8>, SliveError> {
    let mut writer = Writer::new();
    writer.string(1, command);
    writer.uint64(2, seq_id);
    writer.uint32(3, 1); // retryCount
    writer.bytes(4, payload);
    writer.string(9, sub_biz);
    let plain = writer.finish();

    // 服务端校验 `decodedPayloadLen` == 解密后的 UpstreamPayload 长度
    // （不是业务 payload 的长度；RU 报文 10009「解包长度不一致」即此）。
    let mut header = header.clone();
    header.decoded_payload_len = plain.len() as u32;

    let sealed = match header.encryption_mode {
        EncryptionMode::None => plain,
        EncryptionMode::ServiceToken | EncryptionMode::SessionKey => crypto::encrypt(&plain, key)?,
    };
    let header_bytes = encode_header(&header);

    let mut frame = Vec::with_capacity(FRAME_HEADER_LEN + header_bytes.len() + sealed.len());
    frame.extend_from_slice(&MAGIC.to_be_bytes());
    frame.extend_from_slice(&VERSION.to_be_bytes());
    frame.extend_from_slice(&(header_bytes.len() as u32).to_be_bytes());
    frame.extend_from_slice(&(sealed.len() as u32).to_be_bytes());
    frame.extend_from_slice(&header_bytes);
    frame.extend_from_slice(&sealed);
    Ok(frame)
}

fn slice_u32(frame: &[u8], offset: usize) -> Result<u32, SliveError> {
    let bytes = frame
        .get(offset..offset + 4)
        .ok_or_else(|| SliveError::decode("link 帧头截断"))?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// 解码一个服务端帧（DownstreamPayload）。
pub fn decode_downstream(
    frame: &[u8],
    service_key: &[u8],
    session_key: Option<&[u8]>,
) -> Result<DownstreamFrame, SliveError> {
    let magic = frame
        .get(0..2)
        .map(|bytes| u16::from_be_bytes([bytes[0], bytes[1]]))
        .ok_or_else(|| SliveError::decode("link 帧截断"))?;
    if magic != MAGIC {
        return Err(SliveError::decode(format!(
            "link 帧 magic 非法: {magic:#06x}"
        )));
    }
    let version = frame
        .get(2..4)
        .map(|bytes| u16::from_be_bytes([bytes[0], bytes[1]]))
        .unwrap_or(0);
    if version != VERSION {
        return Err(SliveError::decode(format!("link 帧版本非法: {version}")));
    }
    let header_len = slice_u32(frame, 4)? as usize;
    let payload_len = slice_u32(frame, 8)? as usize;
    let expected_len = FRAME_HEADER_LEN
        .checked_add(header_len)
        .and_then(|len| len.checked_add(payload_len))
        .ok_or_else(|| SliveError::decode("link 帧长度溢出"))?;
    if expected_len != frame.len() {
        return Err(SliveError::decode("link 帧长度不一致"));
    }
    let header = decode_header(&frame[FRAME_HEADER_LEN..FRAME_HEADER_LEN + header_len])?;
    let body = &frame[FRAME_HEADER_LEN + header_len..];
    let plain = match header.encryption_mode {
        EncryptionMode::None => body.to_vec(),
        EncryptionMode::ServiceToken => crypto::decrypt(body, service_key)?,
        EncryptionMode::SessionKey => {
            let key = session_key.ok_or_else(|| SliveError::decode("会话密钥未就绪"))?;
            crypto::decrypt(body, key)?
        }
    };
    let fields = parse_fields(&plain)?;
    let error_code = get_u64(&fields, 3).unwrap_or(0) as i32;
    Ok(DownstreamFrame {
        header,
        command: get_string(&fields, 1).unwrap_or_default(),
        error_code,
        payload: find(&fields, 4)
            .and_then(Field::as_bytes)
            .map(<[u8]>::to_vec)
            .unwrap_or_default(),
        error_message: get_string(&fields, 5).unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_frame_roundtrips_through_plain_mode() {
        let header = PacketHeader {
            app_id: 13,
            uid: 1000000284842205,
            instance_id: 7,
            decoded_payload_len: 3,
            encryption_mode: EncryptionMode::None,
            seq_id: 9,
            kpn: "ACFUN_APP".to_string(),
            service_token: None,
        };
        let frame =
            encode_upstream(&header, &[], "Basic.Register", 9, "mainApp", b"abc").expect("编码");
        assert_eq!(&frame[0..2], &MAGIC.to_be_bytes());
        let decoded = decode_downstream(&frame, &[], None).expect("解码");
        assert_eq!(decoded.command, "Basic.Register");
        assert_eq!(decoded.payload, b"abc");
        assert_eq!(decoded.header.seq_id, 9);
        // decodedPayloadLen 必须是 UpstreamPayload 长度（不是业务载荷长度）。
        let mut upstream = Writer::new();
        upstream.string(1, "Basic.Register");
        upstream.uint64(2, 9);
        upstream.uint32(3, 1);
        upstream.bytes(4, b"abc");
        upstream.string(9, "mainApp");
        assert_eq!(
            decoded.header.decoded_payload_len as usize,
            upstream.finish().len()
        );
    }

    #[test]
    fn service_token_frame_roundtrips() {
        let key = [3u8; 16];
        let header = PacketHeader {
            app_id: 13,
            uid: 1,
            instance_id: 0,
            decoded_payload_len: 1,
            encryption_mode: EncryptionMode::ServiceToken,
            seq_id: 1,
            kpn: "ACFUN_APP".to_string(),
            service_token: Some(b"token".to_vec()),
        };
        let frame =
            encode_upstream(&header, &key, "Basic.Register", 1, "mainApp", b"x").expect("编码");
        let decoded = decode_downstream(&frame, &key, None).expect("解码");
        assert_eq!(decoded.header.service_token.as_deref(), Some(&b"token"[..]));
        assert_eq!(decoded.payload, b"x");
    }

    #[test]
    fn bad_magic_is_rejected() {
        let frame = [0u8; FRAME_HEADER_LEN];
        assert!(decode_downstream(&frame, &[], None).is_err());
    }

    #[test]
    fn mismatched_lengths_are_rejected() {
        let header = PacketHeader {
            app_id: 13,
            uid: 1,
            instance_id: 0,
            decoded_payload_len: 0,
            encryption_mode: EncryptionMode::None,
            seq_id: 1,
            kpn: "ACFUN_APP".to_string(),
            service_token: None,
        };
        let mut frame = encode_upstream(&header, &[], "x", 1, "mainApp", b"").expect("编码");
        frame.push(0);
        assert!(decode_downstream(&frame, &[], None).is_err());
    }
}
