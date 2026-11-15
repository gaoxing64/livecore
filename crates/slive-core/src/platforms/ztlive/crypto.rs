//! ZtLive link IM 载荷加密：AES-CBC + PKCS#7（与 SDK 的 SJCL 实现一致）。
//!
//! 线格式是 `IV(16) || ciphertext`；key 为 `ssecurity`（服务 token 阶段）或
//! `RegisterResponse.sessKey`（会话阶段），长度决定 AES-128/192/256。
//! 服务 token 字段本身是**明文**字符串，不进这个密钥。

use aes::{Aes128, Aes192, Aes256};
use block_padding::Pkcs7;
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};

use crate::error::SliveError;

const IV_LEN: usize = 16;

/// PKCS#7 填充的 AES-CBC 加密，返回 `IV || ciphertext`。
pub fn encrypt(plain: &[u8], key: &[u8]) -> Result<Vec<u8>, SliveError> {
    let mut iv = [0u8; IV_LEN];
    getrandom::getrandom(&mut iv)
        .map_err(|e| SliveError::invalid_input(format!("随机 IV 失败: {e}")))?;
    let body = match key.len() {
        16 => cbc::Encryptor::<Aes128>::new(key.into(), (&iv).into())
            .encrypt_padded_vec_mut::<Pkcs7>(plain),
        24 => cbc::Encryptor::<Aes192>::new(key.into(), (&iv).into())
            .encrypt_padded_vec_mut::<Pkcs7>(plain),
        32 => cbc::Encryptor::<Aes256>::new(key.into(), (&iv).into())
            .encrypt_padded_vec_mut::<Pkcs7>(plain),
        len => {
            return Err(SliveError::invalid_input(format!(
                "AES 密钥长度必须是 16/24/32 字节，实际 {len}"
            )))
        }
    };
    let mut out = Vec::with_capacity(IV_LEN + body.len());
    out.extend_from_slice(&iv);
    out.extend_from_slice(&body);
    Ok(out)
}

/// 解密 `IV || ciphertext`，校验并移除 PKCS#7 填充。
pub fn decrypt(data: &[u8], key: &[u8]) -> Result<Vec<u8>, SliveError> {
    if data.len() <= IV_LEN || !(data.len() - IV_LEN).is_multiple_of(16) {
        return Err(SliveError::decode("AES 载荷长度非法"));
    }
    let (iv, body) = data.split_at(IV_LEN);
    let plain = match key.len() {
        16 => cbc::Decryptor::<Aes128>::new(key.into(), iv.into())
            .decrypt_padded_vec_mut::<Pkcs7>(body),
        24 => cbc::Decryptor::<Aes192>::new(key.into(), iv.into())
            .decrypt_padded_vec_mut::<Pkcs7>(body),
        32 => cbc::Decryptor::<Aes256>::new(key.into(), iv.into())
            .decrypt_padded_vec_mut::<Pkcs7>(body),
        len => {
            return Err(SliveError::invalid_input(format!(
                "AES 密钥长度必须是 16/24/32 字节，实际 {len}"
            )))
        }
    }
    .map_err(|_| SliveError::decode("AES 解密失败（密钥或载荷不匹配）"))?;
    Ok(plain)
}

/// 标准 base64 解码（`ssecurity` 字段）。
pub fn decode_base64(text: &str) -> Result<Vec<u8>, SliveError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|e| SliveError::decode(format!("base64 解码失败: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aes128_roundtrips_with_pkcs7() {
        let key = [7u8; 16];
        let plain = b"aes-cbc payload with padding";
        let sealed = encrypt(plain, &key).expect("加密");
        assert_eq!(sealed.len() % 16, 0);
        assert_eq!(decrypt(&sealed, &key).expect("解密"), plain);
    }

    #[test]
    fn aes256_roundtrips() {
        let key = [9u8; 32];
        let plain = b"another key size";
        let sealed = encrypt(plain, &key).expect("加密");
        assert_eq!(decrypt(&sealed, &key).expect("解密"), plain);
    }

    #[test]
    fn wrong_key_fails() {
        let sealed = encrypt(b"secret", &[1u8; 16]).expect("加密");
        assert!(decrypt(&sealed, &[2u8; 16]).is_err());
    }

    #[test]
    fn invalid_key_length_is_reported() {
        assert!(encrypt(b"x", &[0u8; 10]).is_err());
    }

    #[test]
    fn base64_decodes_ssecurity() {
        assert_eq!(
            decode_base64("OVZnZ3cvaWFqdUVUT3RNOUFKUXdEUT09").unwrap(),
            b"9Vggw/iajuETOtM9AJQwDQ=="
        );
    }
}
