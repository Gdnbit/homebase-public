//! Bounded, credential-free downloads for the authenticated iLink media channel.
use super::*;
use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyInit};

pub(super) const DOWNLOAD: &str = "https://novac2c.cdn.weixin.qq.com/c2c/download";

pub(super) struct CdnReference {
    pub url: reqwest::Url,
    pub key: Zeroizing<[u8; 16]>,
}

pub(super) fn reference(media: &Value, label: &str) -> Result<CdnReference, String> {
    if media.get("encrypt_type").is_some_and(|n| n != 1) {
        return Err(format!("{label} 加密格式不受支持，未下载"));
    }
    let key = media["aes_key"]
        .as_str()
        .filter(|s| s.len() <= 48)
        .ok_or_else(|| format!("{label} 缺少有效解密信息"))?;
    let mut key_bytes = Zeroizing::new(
        STANDARD
            .decode(key)
            .map_err(|_| format!("{label} 解密信息无效"))?,
    );
    let key = match key_bytes.len() {
        16 => Zeroizing::new(<[u8; 16]>::try_from(key_bytes.as_slice()).unwrap()),
        32 if key_bytes.iter().all(u8::is_ascii_hexdigit) => {
            let mut decoded = Zeroizing::new([0u8; 16]);
            for (out, pair) in decoded.iter_mut().zip(key_bytes.chunks_exact_mut(2)) {
                let hex = std::str::from_utf8(pair).map_err(|_| format!("{label} 解密信息无效"))?;
                *out = u8::from_str_radix(hex, 16).map_err(|_| format!("{label} 解密信息无效"))?;
            }
            decoded
        }
        _ => return Err(format!("{label} 解密信息无效")),
    };
    let url = if let Some(full) = media["full_url"].as_str().filter(|s| !s.is_empty()) {
        let url = reqwest::Url::parse(full).map_err(|_| format!("{label} 下载地址无效"))?;
        let trusted = reqwest::Url::parse(DOWNLOAD).unwrap();
        if full.len() > 8192
            || url.origin() != trusted.origin()
            || url.path() != trusted.path()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || url.query().is_none_or(str::is_empty)
        {
            return Err(format!("{label} 下载地址超出已核实的官方 CDN 范围，未下载"));
        }
        url
    } else {
        let param = media["encrypt_query_param"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 6144 && !s.chars().any(char::is_control))
            .ok_or_else(|| format!("{label} 缺少下载参数"))?;
        let mut url = reqwest::Url::parse(DOWNLOAD).unwrap();
        url.query_pairs_mut()
            .append_pair("encrypted_query_param", param);
        url
    };

    Ok(CdnReference { url, key })
}

impl Api {
    pub(super) async fn download_cdn(
        &self,
        media: &CdnReference,
        limit: usize,
        expected_cipher_bytes: Option<usize>,
        label: &str,
    ) -> Result<Zeroizing<Vec<u8>>, String> {
        let mut response = self
            .client
            .get(self.target_url(media.url.to_string()))
            .timeout(Duration::from_secs(18))
            .send()
            .await
            .map_err(|_| format!("{label} 下载失败或超时"))?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(format!(
                "{label} 下载失败，CDN 返回 HTTP {}",
                response.status().as_u16()
            ));
        }
        let cipher_limit = limit + 16;
        let maximum = expected_cipher_bytes.unwrap_or(cipher_limit);
        if response
            .content_length()
            .is_some_and(|n| n > maximum as u64)
        {
            return Err(format!("{label} 下载长度超出上限或原声明"));
        }
        let mut data = Zeroizing::new(Vec::new());
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| format!("{label} 下载未完整结束"))?
        {
            if data.len() + chunk.len() > maximum {
                return Err(format!("{label} 下载长度超出上限或原声明"));
            }
            data.extend_from_slice(&chunk);
        }
        if expected_cipher_bytes.is_some_and(|size| data.len() != size) {
            return Err(format!("{label} 下载长度与原声明不一致"));
        }
        let size = ecb::Decryptor::<aes::Aes128>::new((&*media.key).into())
            .decrypt_padded_mut::<Pkcs7>(&mut data)
            .map_err(|_| format!("{label} 解密失败"))?
            .len();
        if size == 0 || size > limit {
            return Err(format!("{label} 解密后为空或超出上限"));
        }
        data.truncate(size);
        Ok(data)
    }
}
