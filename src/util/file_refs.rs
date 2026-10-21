// MCode_Desktop — AI coding agent desktop app
// Copyright (c) 2026 Materin-tech. All rights reserved.
// SPDX-License-Identifier: MIT

//! @ 文件引用展开 — 对齐 CLI util/file-refs.ts：
//! 文本文件注入为带注释内容块，图片（multimodal 模型）转为 data URL 附件。

use std::path::Path;

const MAX_AT_FILE_CHARS: usize = 50_000;
const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;
/// @目录 展开的浅层列目录上限（只列立即子项，不递归——深挖交给模型的 glob/read_file）
const MAX_DIR_ENTRIES: usize = 200;

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
///
/// 附件语义（对齐 Claude Code 的 @ 提交处理）：
/// - 用户正文原样保留，@path 即引用出处；
/// - @文件 → 文件内容、@目录 → 浅层目录列表，统一包进
///   `<mcode-attachments>` 结构化块附在正文之后，并把"如何使用这些内容"
///   的裁量权显式交给模型（可继续用 read_file/glob/grep 深挖）。
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
        let Ok(meta) = std::fs::metadata(&abs) else {
            missing.push(r.clone());
            continue;
        };
        if meta.is_dir() {
            match list_dir_shallow(&abs) {
                Ok((listing, omitted)) => {
                    let more = if omitted > 0 {
                        format!("\n…[另有 {omitted} 项未列出]")
                    } else {
                        String::new()
                    };
                    injected.push(r.clone());
                    blocks.push(format!(
                        "<attachment path=\"{r}\" type=\"directory\">\n{listing}{more}\n</attachment>"
                    ));
                }
                Err(_) => missing.push(r.clone()),
            }
            continue;
        }
        let ext = r.rsplit('.').next().unwrap_or_default().to_lowercase();
        if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
            if meta.len() <= MAX_IMAGE_BYTES {
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
            } else {
                missing.push(format!("{r}（图片过大: {} bytes）", meta.len()));
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
                    "<attachment path=\"{r}\" type=\"file\">\n{truncated}{more}\n</attachment>"
                ));
            }
            Err(_) => missing.push(r.clone()),
        }
    }
    let text = if blocks.is_empty() {
        input.to_string()
    } else {
        // 结构化附件块：正文之外单独封套，使用方式交给模型裁量
        format!(
            "{}\n\n<mcode-attachments>\n以下是用户在消息中通过 @路径 引用的附件。\
             请自行判断如何使用这些内容；需要更多上下文时用 read_file/glob/grep 工具继续查看。\n{}\n</mcode-attachments>",
            input,
            blocks.join("\n")
        )
    };
    FileRefResult {
        text,
        injected,
        missing,
        attachments,
    }
}

/// 目录浅层列表：立即子项（目录带 `/` 后缀、排前），超出 MAX_DIR_ENTRIES 截断。
fn list_dir_shallow(dir: &Path) -> std::io::Result<(String, usize)> {
    let mut dirs = vec![];
    let mut files = vec![];
    for e in std::fs::read_dir(dir)?.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue; // 与补全树同口径：默认不带隐藏项
        }
        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            dirs.push(name);
        } else {
            files.push(name);
        }
    }
    dirs.sort();
    files.sort();
    let total = dirs.len() + files.len();
    let omitted = total.saturating_sub(MAX_DIR_ENTRIES);
    let mut lines = vec![];
    for name in dirs.iter().chain(files.iter()).take(MAX_DIR_ENTRIES) {
        let is_dir = dirs.contains(name);
        lines.push(if is_dir { format!("{name}/") } else { name.clone() });
    }
    Ok((lines.join("\n"), omitted))
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

    /// @目录 → 浅层列表附件（不递归）；@文件 → 内容附件；统一封进
    /// <mcode-attachments>，正文原样保留——对齐 Claude Code 的 @ 提交语义
    #[test]
    fn expands_dir_and_file_refs_into_structured_attachments() {
        let base = std::env::temp_dir().join(format!("mcode-refs-{}", std::process::id()));
        let dir = base.join("sub");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("inner.txt"), "hello").unwrap();
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        std::fs::write(base.join("README.md"), "# doc").unwrap();

        let input = "看下 @sub 和 @README.md。";
        let r = expand_file_refs(input, &base, false);

        // 正文原样保留，附件单独封套且把裁量权交给模型
        assert!(r.text.starts_with(input), "用户正文不得被改写");
        assert!(r.text.contains("<mcode-attachments>"), "缺附件封套");
        assert!(r.text.contains("read_file/glob/grep"), "缺模型裁量提示");

        // 目录：浅层列表，目录项带 / 且排前，不递归读 nested 内容
        assert!(r
            .text
            .contains("<attachment path=\"sub\" type=\"directory\">"));
        let dir_block = r
            .text
            .split("<attachment path=\"sub\" type=\"directory\">")
            .nth(1)
            .unwrap()
            .split("</attachment>")
            .next()
            .unwrap();
        assert!(dir_block.contains("inner.txt") && dir_block.contains("nested/"));
        assert!(!dir_block.contains("hello"), "目录列表不得递归读文件内容");

        // 文件：内容附件
        assert!(r.text.contains("<attachment path=\"README.md\" type=\"file\">"));
        assert!(r.text.contains("# doc"));

        assert_eq!(r.injected, vec!["sub", "README.md"]);
        assert!(r.missing.is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// @不存在的路径 → missing 提示，正文不动
    #[test]
    fn missing_refs_keep_text_intact() {
        let base = std::env::temp_dir().join(format!("mcode-refs-miss-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let input = "看下 @ghost.txt";
        let r = expand_file_refs(input, &base, false);
        assert_eq!(r.text, input, "无附件时正文不得被追加任何块");
        assert_eq!(r.missing.len(), 1);
        let _ = std::fs::remove_dir_all(&base);
    }
}
