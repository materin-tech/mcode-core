// MCode_Desktop — AI coding agent desktop app
// Copyright (c) 2026 Materin-tech. All rights reserved.
// SPDX-License-Identifier: MIT

//! 网络工具 — webfetch / websearch（对齐 CLI tools/webfetch.ts + websearch.ts）

use super::{Tool, ToolContext, ToolDefinition, ToolResult, clamp_output};
use anyhow::{Context as _, Result};
use serde_json::{Value, json};

const MAX_FETCH_CHARS: usize = 20_000;
const MAX_RESULTS: usize = 8;
const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36";

// ---------- webfetch ----------

pub struct WebFetch;

#[async_trait::async_trait]
impl Tool for WebFetch {
    fn def(&self) -> ToolDefinition {
        ToolDefinition {
            name: "webfetch".into(),
            description: "抓取 URL 的可读文本（HTML 转纯文本，默认截断 20k 字符）".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "max_chars": { "type": "integer", "description": "默认 20000" }
                },
                "required": ["url"]
            }),
            is_read_only: true,
        }
    }

    async fn execute(&self, input: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        let url = input
            .get("url")
            .and_then(|v| v.as_str())
            .context("缺少参数 url")?
            .to_string();
        let max = input
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .unwrap_or(MAX_FETCH_CHARS as u64) as usize;
        let max = max.min(100_000);
        let client = reqwest::Client::builder()
            .user_agent(UA)
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        let resp = client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("请求失败: {url}"))?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("HTTP {status}: {url}");
        }
        let text = html_to_text(&body);
        let truncated: String = text.chars().take(max).collect();
        Ok(ToolResult {
            success: true,
            output: clamp_output(truncated),
            display: Some(super::DisplayBlock::FileRef {
                path: url,
                lines: String::new(),
            }),
        })
    }
}

/// HTML → 可读文本：去 script/style、去标签、实体解码、压缩空白
fn html_to_text(html: &str) -> String {
    let mut s = html.to_string();
    // 去掉 script/style 块
    for tag in ["script", "style", "noscript"] {
        if let Some((start, _)) = find_tag(&s, tag)
            && let Some(end_rel) = s[start..].find(&format!("</{tag}>"))
        {
            let end = start + end_rel + tag.len() + 3;
            s.replace_range(start..end.min(s.len()), " ");
        }
    }
    // 块级标签换行
    for t in [
        "</p>", "</div>", "<br>", "<br/>", "<br />", "</h1>", "</h2>", "</h3>", "</li>", "</tr>",
    ] {
        s = s.replace(t, "\n");
    }
    // 去所有标签
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    // 常见实体
    let out = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    // 压缩空白
    out.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn find_tag(s: &str, tag: &str) -> Option<(usize, ())> {
    s.find(&format!("<{tag}")).map(|i| (i, ()))
}

// ---------- websearch ----------

pub struct WebSearch;

#[async_trait::async_trait]
impl Tool for WebSearch {
    fn def(&self) -> ToolDefinition {
        ToolDefinition {
            name: "websearch".into(),
            description: "网络搜索（DuckDuckGo，返回 标题 + URL + 摘要）".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "max_results": { "type": "integer", "description": "≤8" }
                },
                "required": ["query"]
            }),
            is_read_only: true,
        }
    }

    async fn execute(&self, input: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        let query = input
            .get("query")
            .and_then(|v| v.as_str())
            .context("缺少参数 query")?
            .to_string();
        let max = input
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .clamp(1, MAX_RESULTS as u64) as usize;
        let client = reqwest::Client::builder()
            .user_agent(UA)
            .timeout(std::time::Duration::from_secs(20))
            .build()?;
        let resp = client
            .post("https://html.duckduckgo.com/html/")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(format!("q={}", urlencode(&query)))
            .send()
            .await
            .context("搜索请求失败")?;
        let body = resp.text().await.unwrap_or_default();
        let results = parse_ddg(&body, max);
        if results.is_empty() {
            anyhow::bail!("无搜索结果（可能被限流），可稍后重试或用 webfetch 直接访问已知 URL");
        }
        let mut out = String::new();
        for (i, (title, url, snippet)) in results.iter().enumerate() {
            out.push_str(&format!("{}. {title}\n   {url}\n   {snippet}\n\n", i + 1));
        }
        Ok(ToolResult {
            success: true,
            output: clamp_output(out),
            display: Some(super::DisplayBlock::FileRef {
                path: format!("websearch:{query}"),
                lines: String::new(),
            }),
        })
    }
}

/// 解析 DuckDuckGo HTML 版结果：result__a 链接 + result__snippet
fn parse_ddg(html: &str, max: usize) -> Vec<(String, String, String)> {
    let mut out = vec![];
    let re_link =
        regex::Regex::new(r#"<a[^>]*class="result__a"[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#).unwrap();
    let re_snip = regex::Regex::new(r#"<a[^>]*class="result__snippet"[^>]*>(.*?)</a>"#).unwrap();
    let strip = |s: &str| html_to_text(s);
    let links: Vec<(String, String)> = re_link
        .captures_iter(html)
        .map(|c| (c[1].to_string(), strip(&c[2])))
        .collect();
    let snippets: Vec<String> = re_snip.captures_iter(html).map(|c| strip(&c[1])).collect();
    for (i, (url, title)) in links.into_iter().take(max).enumerate() {
        let snippet = snippets.get(i).cloned().unwrap_or_default();
        out.push((title, decode_ddg_url(&url), snippet));
    }
    out
}

/// DDG 的重定向 URL（//duckduckgo.com/l/?uddg=...）还原真实地址
fn decode_ddg_url(url: &str) -> String {
    if let Some(pos) = url.find("uddg=") {
        let rest = &url[pos + 5..];
        let end = rest.find('&').unwrap_or(rest.len());
        if let Ok(decoded) = urldecode(&rest[..end]) {
            return decoded;
        }
    }
    url.trim_start_matches("//").to_string()
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn urldecode(s: &str) -> Result<String> {
    let bytes = s.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3])?;
                out.push(u8::from_str_radix(hex, 16)?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    Ok(String::from_utf8(out)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_to_text_strips_tags() {
        let t = html_to_text(
            "<html><script>var x=1;</script><body><h1>Hi</h1><p>a&amp;b</p></body></html>",
        );
        assert!(t.contains("Hi"));
        assert!(t.contains("a&b"));
        assert!(!t.contains("var x"));
        assert!(!t.contains('<'));
    }

    #[test]
    fn ddg_url_decode() {
        assert_eq!(
            decode_ddg_url("//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa&rut=abc"),
            "https://example.com/a"
        );
    }
}
