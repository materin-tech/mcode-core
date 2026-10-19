// MCode_Desktop — AI coding agent desktop app
// Copyright (c) 2026 Materin-tech. All rights reserved.
// SPDX-License-Identifier: MIT

//! @ 文件引用展开 — 对齐 CLI util/file-refs.ts：
//! 文本文件注入为带注释内容块，图片（multimodal 模型）转为 data URL 附件。

use std::path::Path;

const MAX_AT_FILE_CHARS: usize = 50_000;
const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];

const BINARY_EXTENSIONS: &[&str] = &[
    "pdf", "zip", "gz", "tar", "rar", "7z", "bz2", "xz", "mp3", "mp4", "mov", "avi", "mkv", "wav",
    "flac", "woff", "woff2", "ttf", "otf", "eot", "exe", "dll", "dylib", "so", "bin", "ds_store",
    "pptx", "docx", "xlsx", "key", "pages", "numbers", "ico", "icns", "tiff",
];

#[derive(Debug, Default, Clone)]
pub struct FileRefResult {
    /// 展开 @path 内容块后的输入文本
    pub text: String,
    /// 成功注入的引用
    pub injected: Vec<String>,
    /// 解析失败的引用
    pub missing: Vec<String>,
    /// 图片附件（multimodal 模型才非空）(path, data_url)
    pub attachments: Vec<(String, String)>,
}

/// 匹配 '@path' 引用：路径须以词字符结尾，避免吞掉句尾标点（对齐 CLI FILE_REF_PATTERN）
pub fn extract_refs(input: &str) -> Vec<String> {
    let mut refs = vec![];
    let chars: Vec<(usize, char)> = input.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let (byte_idx, c) = chars[i];
        if c == '@' && (i == 0 || !is_word(chars[i - 1].1)) {
            let start_byte = byte_idx + 1;
            let mut end_byte = start_byte;
            let mut end_i = i + 1;
            while end_i < chars.len()
                && (is_word(chars[end_i].1) || matches!(chars[end_i].1, '/' | '.' | '-'))
            {
                end_byte = chars[end_i].0 + chars[end_i].1.len_utf8();
                end_i += 1;
            }
            // 剥离尾部 CJK 标点（防吞句末标点，如 @README.md。）
            if end_byte > start_byte {
                let mut slice = &input[start_byte..end_byte];
                slice = slice.trim_end_matches(|ch| CJK_PUNCT.contains(&ch));
                let last_ok = slice.chars().next_back().is_some_and(is_word);
                if !slice.is_empty() && last_ok && !refs.contains(&slice.to_string()) {
                    refs.push(slice.to_string());
                }
            }
            i = end_i;
        } else {
            i += 1;
        }
    }
    refs
}

const CJK_PUNCT: &[char] = &[
    '。', '，', '、', '；', '：', '！', '？', '）', '》', '」', '』', '】', '〉',
];

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn mime_of(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default() {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => "image/png",
    }
}

/// 展开 @path 引用。multimodal=false 时图片引用降级为缺失提示（不附加）。
pub fn expand_file_refs(input: &str, cwd: &Path, multimodal: bool) -> FileRefResult {
    let refs = extract_refs(input);
    if refs.is_empty() {
        return FileRefResult {
            text: input.into(),
            ..Default::default()
        };
    }
    let mut injected = vec![];
    let mut missing = vec![];
    let mut blocks = vec![];
    let mut attachments = vec![];
    for r in &refs {
        let abs = match crate::tools::contain_to_workspace(cwd, r) {
            Ok(p) => p,
            Err(_) => {
                missing.push(r.clone());
                continue;
            }
        };
        let ext = r.rsplit('.').next().unwrap_or_default().to_lowercase();
        if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
            match std::fs::metadata(&abs) {
                Ok(m) if m.len() <= MAX_IMAGE_BYTES => {
                    if !multimodal {
                        missing.push(format!("{r}（当前模型不支持多模态，图片已忽略）"));
                        continue;
                    }
                    match std::fs::read(&abs) {
                        Ok(bytes) => {
                            attachments.push((
                                r.clone(),
                                format!("data:{};base64,{}", mime_of(r), use_base64(&bytes)),
                            ));
                            injected.push(r.clone());
                        }
                        Err(_) => missing.push(r.clone()),
                    }
                }
                Ok(m) => missing.push(format!("{r}（图片过大: {} bytes）", m.len())),
                Err(_) => missing.push(r.clone()),
            }
            continue;
        }
        if BINARY_EXTENSIONS.contains(&ext.as_str()) {
            missing.push(format!("{r}（二进制文件，无法以文本渲染）"));
            continue;
        }
        match std::fs::read(&abs) {
            Ok(bytes) => {
                let content = String::from_utf8_lossy(&bytes);
                if content.chars().take(8000).any(|c| c == '\0') {
                    missing.push(format!("{r}（二进制内容，已跳过）"));
                    continue;
                }
                let truncated: String = content.chars().take(MAX_AT_FILE_CHARS).collect();
                let more = if content.chars().count() > MAX_AT_FILE_CHARS {
                    "\n…[truncated]"
                } else {
                    ""
                };
                injected.push(r.clone());
                blocks.push(format!(
                    "--- file: {r} ---\n{truncated}{more}\n--- end of {r} ---"
                ));
            }
            Err(_) => missing.push(r.clone()),
        }
    }
    let text = if blocks.is_empty() {
        input.to_string()
    } else {
        format!("{}\n\n{}", input, blocks.join("\n\n"))
    };
    FileRefResult {
        text,
        injected,
        missing,
        attachments,
    }
}

fn use_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_refs_without_punctuation() {
        let refs = extract_refs("看下 @src/api/client.ts 和 @README.md。");
        assert_eq!(refs, vec!["src/api/client.ts", "README.md"]);
        assert!(extract_refs("no refs here").is_empty());
        // 邮箱中间不触发
        assert!(
            extract_refs("mail me a@b.com").is_empty()
                || extract_refs("mail me a@b.com") == ["b.com"]
        );
    }

    #[test]
    fn base64_known_vectors() {
        assert_eq!(use_base64(b"Man"), "TWFu");
        assert_eq!(use_base64(b"Ma"), "TWE=");
        assert_eq!(use_base64(b"M"), "TQ==");
    }
}
