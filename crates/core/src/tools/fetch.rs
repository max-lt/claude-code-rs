use std::path::Path;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_TYPE, USER_AGENT};

use super::{ToolDef, ToolOutput};

pub struct FetchTool {
    client: reqwest::Client,
}

impl Default for FetchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl FetchTool {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .expect("failed to build HTTP client");
        Self { client }
    }
}

impl ToolDef for FetchTool {
    fn name(&self) -> &'static str {
        "Fetch"
    }

    fn description(&self) -> &'static str {
        "Make HTTP requests. Supports GET, POST, PUT, PATCH, DELETE with headers and body. \
         Returns status code, response headers, and body. \
         Use this instead of curl/wget via Bash."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch"
                },
                "method": {
                    "type": "string",
                    "enum": ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"],
                    "description": "HTTP method (default: GET)"
                },
                "headers": {
                    "type": "object",
                    "description": "HTTP headers as key-value pairs",
                    "additionalProperties": { "type": "string" }
                },
                "body": {
                    "type": "string",
                    "description": "Request body (for POST/PUT/PATCH)"
                },
                "max_bytes": {
                    "type": "integer",
                    "description": "Max response body size in bytes (default: 1048576 = 1MB)"
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, input: &serde_json::Value, _cwd: &Path) -> ToolOutput {
        let url = match input.get("url").and_then(|v| v.as_str()) {
            Some(u) => u,
            None => return ToolOutput::error("Missing required parameter: url"),
        };

        let method_str = input
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("GET");

        let method = match method_str.to_uppercase().as_str() {
            "GET" => reqwest::Method::GET,
            "POST" => reqwest::Method::POST,
            "PUT" => reqwest::Method::PUT,
            "PATCH" => reqwest::Method::PATCH,
            "DELETE" => reqwest::Method::DELETE,
            "HEAD" => reqwest::Method::HEAD,
            other => return ToolOutput::error(format!("Unsupported HTTP method: {other}")),
        };

        let max_bytes = input
            .get("max_bytes")
            .and_then(|v| v.as_u64())
            .unwrap_or(1_048_576) as usize;

        let mut request = self.client.request(method.clone(), url);

        // Default User-Agent mimics Firefox to avoid bot-blocking
        request = request.header(
            USER_AGENT,
            "Mozilla/5.0 (X11; Linux x86_64; rv:133.0) Gecko/20100101 Firefox/133.0",
        );

        // Custom headers
        if let Some(headers_obj) = input.get("headers").and_then(|v| v.as_object()) {
            let mut header_map = HeaderMap::new();
            for (key, val) in headers_obj {
                let name = match key.parse::<HeaderName>() {
                    Ok(n) => n,
                    Err(e) => return ToolOutput::error(format!("Invalid header name '{key}': {e}")),
                };
                let value = match val.as_str() {
                    Some(v) => match v.parse::<HeaderValue>() {
                        Ok(hv) => hv,
                        Err(e) => {
                            return ToolOutput::error(format!(
                                "Invalid header value for '{key}': {e}"
                            ))
                        }
                    },
                    None => continue,
                };
                header_map.insert(name, value);
            }
            request = request.headers(header_map);
        }

        // Body
        if let Some(body) = input.get("body").and_then(|v| v.as_str()) {
            request = request.body(body.to_string());
        }

        // Execute
        let response = match request.send().await {
            Ok(r) => r,
            Err(e) => {
                if e.is_timeout() {
                    return ToolOutput::error(format!("Request timed out: {url}"));
                }
                if e.is_connect() {
                    return ToolOutput::error(format!("Connection failed: {url}"));
                }
                return ToolOutput::error(format!("HTTP request failed: {e}"));
            }
        };

        let status = response.status();
        let status_line = format!("{} {}", status.as_u16(), status.canonical_reason().unwrap_or(""));

        // Collect response headers
        let mut resp_headers = String::new();
        for (name, value) in response.headers() {
            if let Ok(v) = value.to_str() {
                resp_headers.push_str(&format!("{name}: {v}\n"));
            }
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        // HEAD requests: no body
        if method == reqwest::Method::HEAD {
            return ToolOutput::success(format!(
                "HTTP {status_line}\n\n{resp_headers}"
            ));
        }

        // Read body with size limit
        let body_bytes = match response.bytes().await {
            Ok(b) => b,
            Err(e) => return ToolOutput::error(format!("Failed to read response body: {e}")),
        };

        let truncated = body_bytes.len() > max_bytes;
        let body_slice = if truncated {
            &body_bytes[..max_bytes]
        } else {
            &body_bytes[..]
        };

        // Determine if binary
        let is_binary = !content_type.contains("text")
            && !content_type.contains("json")
            && !content_type.contains("xml")
            && !content_type.contains("javascript")
            && !content_type.contains("html")
            && !content_type.contains("css")
            && !content_type.contains("svg")
            && body_slice.iter().any(|&b| b == 0);

        let body_text = if is_binary {
            format!("<binary data, {} bytes>", body_bytes.len())
        } else {
            let text = String::from_utf8_lossy(body_slice).to_string();
            if truncated {
                format!(
                    "{text}\n\n... truncated ({} bytes total, showing first {max_bytes})",
                    body_bytes.len()
                )
            } else {
                text
            }
        };

        let output = format!("HTTP {status_line}\n\n{resp_headers}\n{body_text}");
        ToolOutput::success(output)
    }
}
