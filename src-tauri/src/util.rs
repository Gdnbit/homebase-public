//! Cross-cutting 工具函数
//!
//! 当前唯一职责：剥 UTF-8 BOM。Windows 文本编辑器（VSCode 默认 UTF-8 with BOM、
//! Notepad 1809+ 保存 UTF-8 也会写 BOM）保存 JSON / TOML 配置文件时常带 EF BB BF
//! 头。`std::fs::read_to_string` 不会自动剥 BOM，导致后续 `serde_json::from_str`
//! 报 `expected value at line 1 column 1`。
//!
//! 所有读"用户可编辑配置文件"的 read_to_string + from_str 链都应该过 strip_bom。

/// 剥 UTF-8 BOM (`U+FEFF` / `EF BB BF`)。
///
/// 对没有 BOM 的字符串无副作用（直接返回原引用）。
#[inline]
pub(crate) fn strip_bom(s: &str) -> &str {
    s.strip_prefix('\u{FEFF}').unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_bom_removes_utf8_bom_prefix() {
        let with_bom = "\u{FEFF}{\"a\":1}";
        assert_eq!(strip_bom(with_bom), "{\"a\":1}");
    }

    #[test]
    fn strip_bom_passthrough_no_bom() {
        let plain = "{\"a\":1}";
        assert_eq!(strip_bom(plain), plain);
    }

    #[test]
    fn strip_bom_only_strips_first_bom() {
        // 只剥前缀；中间的 BOM-like 字符不应碰
        let s = "\u{FEFF}hello\u{FEFF}world";
        assert_eq!(strip_bom(s), "hello\u{FEFF}world");
    }

    #[test]
    fn strip_bom_empty_string() {
        assert_eq!(strip_bom(""), "");
    }

    #[test]
    fn strip_bom_just_bom() {
        assert_eq!(strip_bom("\u{FEFF}"), "");
    }

    #[test]
    fn serde_json_parses_bom_prefixed_string_after_strip() {
        // 模拟 read_claude_endpoint 场景：磁盘上 settings.json 头 3 字节是 EF BB BF
        let raw = "\u{FEFF}{\"env\":{\"ANTHROPIC_BASE_URL\":\"https://x.test\"}}";
        // 不剥 BOM 应失败
        assert!(serde_json::from_str::<serde_json::Value>(raw).is_err());
        // 剥 BOM 后应成功
        let v: serde_json::Value =
            serde_json::from_str(strip_bom(raw)).expect("strip_bom + from_str should succeed");
        assert_eq!(
            v["env"]["ANTHROPIC_BASE_URL"].as_str(),
            Some("https://x.test")
        );
    }

    #[test]
    fn toml_parses_bom_prefixed_string_after_strip() {
        // Codex config.toml 同样易带 BOM。
        // 注意：toml crate 自身已宽容 BOM 前缀（与 serde_json 不同），strip_bom 仍然
        // 正确——保证调用方代码风格统一，不依赖 toml 实现细节。
        let raw = "\u{FEFF}model = \"gpt-5\"\n";
        let v: toml::Value = toml::from_str(strip_bom(raw)).expect("ok");
        assert_eq!(v.get("model").and_then(|x| x.as_str()), Some("gpt-5"));
    }

    #[test]
    fn toml_edit_parses_bom_prefixed_string_after_strip() {
        // toml_edit::DocumentMut 用于 read_codex_endpoint / write_codex_endpoint
        let raw = "\u{FEFF}model_provider = \"homebase-sops\"\n";
        let stripped = strip_bom(raw);
        let doc: toml_edit::DocumentMut = stripped.parse().expect("parse ok");
        assert_eq!(
            doc.get("model_provider").and_then(|x| x.as_str()),
            Some("homebase-sops")
        );
    }
}
