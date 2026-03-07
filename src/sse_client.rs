use anyhow::{anyhow, Result};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

#[derive(Debug)]
pub enum SseLineResult {
    Delta(String),
    Done,
    Skip,
}

pub struct SseClient;

impl SseClient {
    pub fn parse_sse_line(line: &str) -> Result<SseLineResult> {
        let trimmed = line.trim_end();
        if trimmed.is_empty() || trimmed.starts_with(':') {
            return Ok(SseLineResult::Skip);
        }

        if !trimmed.starts_with("data: ") {
            return Ok(SseLineResult::Skip);
        }

        let data = &trimmed["data: ".len()..];
        if data == "[DONE]" {
            return Ok(SseLineResult::Done);
        }

        if let Some(content) = Self::extract_delta_content(data) {
            Ok(SseLineResult::Delta(content))
        } else {
            Ok(SseLineResult::Skip)
        }
    }

    fn extract_delta_content(json_str: &str) -> Option<String> {
        let parsed: serde_json::Value = serde_json::from_str(json_str).ok()?;
        let choices = parsed.get("choices")?.as_array()?;
        let first = choices.get(0)?;
        let delta = first.get("delta")?;
        let content = delta.get("content")?.as_str()?;
        if content.is_empty() {
            None
        } else {
            Some(content.to_string())
        }
    }

    pub fn curl_stream<F>(
        url: &str,
        body: &str,
        auth_header: Option<&str>,
        extra_headers: &[&str],
        timeout_secs: u64,
        callback: F,
    ) -> Result<()>
    where
        F: Fn(String),
    {
        let mut cmd = Command::new("curl");
        cmd.arg("-s")
            .arg("--no-buffer")
            .arg("--fail-with-body");

        if timeout_secs > 0 {
            cmd.arg("--max-time").arg(timeout_secs.to_string());
        }

        cmd.arg("-X")
            .arg("POST")
            .arg("-H")
            .arg("Content-Type: application/json");

        if let Some(auth) = auth_header {
            cmd.arg("-H").arg(auth);
        }

        for hdr in extra_headers {
            cmd.arg("-H").arg(hdr);
        }

        cmd.arg("-d").arg(body).arg(url);

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::null());

        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("Failed to capture stdout"))?;
        let reader = BufReader::new(stdout);

        for line in reader.lines() {
            let line = line?;
            match Self::parse_sse_line(&line)? {
                SseLineResult::Delta(text) => callback(text),
                SseLineResult::Done => break,
                SseLineResult::Skip => continue,
            }
        }

        let status = child.wait()?;
        if !status.success() {
            return Err(anyhow!("Curl failed with status: {}", status));
        }

        Ok(())
    }
}
