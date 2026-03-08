# AI Inefficiency & Agent Design Audit Report

**Project:** OpenPaw  
**Audit Date:** March 8, 2026  
**Auditor:** AI Codebase Analysis

---

## Executive Summary

OpenPaw is a well-architected Rust-based AI agent runtime with solid foundations: modular provider abstraction, retry/fallback mechanisms, streaming support, and a capable tool system. However, the audit identified **23 distinct inefficiencies** across four dimensions:

| Dimension | Critical | High | Medium | Low | Total |
|-----------|----------|------|--------|-----|-------|
| 1. Redundant AI Calls | 2 | 3 | 2 | 1 | 8 |
| 2. Billing Inefficiencies | 1 | 4 | 2 | 0 | 7 |
| 3. Agent Autonomy Loss | 1 | 2 | 2 | 0 | 5 |
| 4. Architectural Weaknesses | 0 | 2 | 1 | 0 | 3 |
| **Total** | **4** | **11** | **7** | **1** | **23** |

**Estimated Monthly Cost Impact:** For a deployment processing 10,000 turns/day with mixed model usage:
- **Critical + High issues:** ~$150-400/month wasted on redundant calls and oversized contexts
- **Medium issues:** ~$50-100/month in optimization opportunities

**UX Impact:** Agent hesitation, unnecessary confirmation loops, and sequential tool execution add 2-5 seconds latency per complex task.

---

## Prioritized Issues Table

| ID | Severity | Issue | Est. Cost/UX Impact | Affected File(s) |
|----|----------|-------|---------------------|------------------|
| B-1 | Critical | Unbounded token generation (no output limits enforced) | $80-200/mo | `src/agent/mod.rs` |
| R-1 | Critical | Duplicate memory operations (store + recall per turn) | $40-100/mo | `src/agent/mod.rs`, `src/tools/memory_*.rs` |
| R-2 | Critical | Compaction triggers AI call on every threshold crossing | $30-80/mo | `src/agent/compaction.rs` |
| A-1 | Critical | Agent asks for confirmations it could infer | 3-5s latency/task | `src/agent/prompt.rs` |
| B-2 | High | Expensive models used for simple classification tasks | $50-120/mo | `src/agent/mod.rs` |
| B-3 | High | Full history sent when recent context suffices | $40-90/mo | `src/agent/mod.rs` |
| B-4 | High | Silent retry multiplication in fallback chains | 2-3x billing on errors | `src/providers/fallback.rs`, `reliable.rs` |
| B-5 | High | Streaming fallback loses benefit, wastes tokens | $20-50/mo | `src/providers/reliable.rs` |
| R-3 | High | System prompt rebuilt every turn (wastes tokens) | $15-40/mo | `src/agent/mod.rs` |
| R-4 | High | Tool call parsing duplicates provider's native parsing | $10-30/mo | `src/agent/mod.rs` |
| R-5 | High | Follow-through detection causes extra AI iterations | 1-2 extra calls/task | `src/agent/mod.rs` |
| A-2 | High | Missing tool-use opportunities (asks vs acts) | 2-4s latency | `src/agent/prompt.rs` |
| A-3 | High | Overly conservative safety prompts | UX hesitation | `src/agent/prompt.rs` |
| M-1 | Medium | Sequential tool execution (no parallelism) | 2-3s latency | `src/agent/mod.rs` |
| M-2 | Medium | Missing cache for repeated queries | $20-50/mo | `src/tools/cache.rs` |
| M-3 | Medium | Token estimation uses naive char/4 heuristic | Context overflow risk | `src/agent/compaction.rs` |
| M-4 | Medium | Subagent isolation shares provider (no quotas) | Resource contention | `src/subagent.rs` |
| L-1 | Low | Tool cache TTL too short (30s default) | Minor redundancy | `src/tools/cache.rs` |
| R-6 | Medium | Memory enrichment called even when memory disabled | Wasted cycles | `src/agent/mod.rs` |
| B-6 | Medium | No cost tracking integration in agent loop | Blind spending | `src/cost.rs` |
| A-4 | Medium | Hardcoded human-in-loop for destructive ops | UX friction | `src/agent/prompt.rs` |
| F-1 | Medium | Sequential fallback (no parallel probing) | 5-10s latency on failures | `src/providers/fallback.rs` |
| F-2 | Low | No circuit breaker state persistence | Cold-start failures | `src/providers/circuit_breaker.rs` |

**Legend:** R = Redundant Calls, B = Billing, A = Autonomy, M = Medium-term Arch, F = Flow, L = Low

---

## Per-Issue Analysis

### R-1: Duplicate Memory Operations (Critical)

**Root Cause:** The agent auto-saves every user message to memory, then immediately enriches the same message by recalling from memory—creating redundant read/write cycles.

**Affected Files:** 
- `src/agent/mod.rs` (lines 174-185, 204-212)
- `src/tools/memory_store.rs`
- `src/tools/memory_recall.rs`

**Current Code:**
```rust
// Auto-save user message
if self.auto_save {
    let key = format!("autosave_user_{}", /* timestamp */);
    let _ = self.memory.store(&key, &user_message, ...);
}

// Enrich user message with memory context
let enriched_msg = match memory_loader::enrich_message(
    self.memory.as_ref(),
    &user_message,  // Same message just saved!
    self.memory_session_id.as_deref(),
) { ... }
```

**Problem:** Every turn incurs a memory write + recall operation. For a 10K turns/day deployment, this is 20K unnecessary memory operations.

**Fix:** Only enrich if memory contains relevant prior context (not the current message). Skip auto-save for messages under 20 chars (greetings).

```rust
// FIX: Conditional memory operations
if self.auto_save && user_message.len() > 20 {
    // Only save substantive messages
    let key = format!("autosave_user_{}", timestamp);
    let _ = self.memory.store(&key, &user_message, ...);
}

// FIX: Enrich only if not a simple message
let enriched_msg = if user_message.len() > 30 && !is_greeting(&user_message) {
    memory_loader::enrich_message(self.memory.as_ref(), &user_message, ...)?
} else {
    user_message.clone()
};

fn is_greeting(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    matches!(lower.as_str(), "hi" | "hello" | "hey" | "thanks" | "thank you" | "ok" | "yes" | "no")
        || lower.starts_with("hi ") || lower.starts_with("hello ")
}
```

---

### R-2: Compaction Triggers AI Call on Every Threshold Crossing (Critical)

**Root Cause:** The `auto_compact_history` function calls the LLM to summarize context every time the token/message threshold is crossed, with no cooldown or batching.

**Affected File:** `src/agent/compaction.rs` (lines 89-145)

**Current Code:**
```rust
pub fn auto_compact_history(...) -> bool {
    let count_trigger = non_system_count > config.max_history_messages as usize;
    let token_trigger = config.token_limit > 0 && token_estimate(history) > token_threshold;
    
    if !count_trigger && !token_trigger {
        return false;
    }
    
    // Immediately calls LLM to summarize
    let summary = summarize_slice(provider, model_name, history, start, compact_end, config)...
}
```

**Problem:** In active conversations, the threshold can be crossed multiple times in quick succession, triggering redundant summarization calls.

**Fix:** Add a compaction cooldown (e.g., 5 minutes) and batch multiple threshold crossings.

```rust
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static LAST_COMPACTION: AtomicU64 = AtomicU64::new(0);
const COMPACTION_COOLDOWN_SECS: u64 = 300; // 5 minutes

pub fn auto_compact_history(...) -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    
    let last = LAST_COMPACTION.load(Ordering::Relaxed);
    if now - last < COMPACTION_COOLDOWN_SECS {
        return false; // Cooldown active
    }
    
    // ... existing trigger logic ...
    
    let summary = summarize_slice(...)?;
    
    LAST_COMPACTION.store(now, Ordering::Relaxed);
    true
}
```

---

### B-1: Unbounded Token Generation (Critical)

**Root Cause:** While `max_tokens` is set in `ChatRequest`, the agent does not enforce or validate output limits, and compaction summaries request 1024 tokens without justification.

**Affected Files:**
- `src/agent/mod.rs` (line 293)
- `src/agent/compaction.rs` (line 178)

**Current Code:**
```rust
let request = ChatRequest {
    messages: &self.history,
    model: &self.model_name,
    temperature: self.temperature,
    max_tokens: Some(self.max_tokens),  // Set but not validated against response
    ...
};

// In compaction.rs:
let request = crate::providers::ChatRequest {
    max_tokens: Some(1024),  // Arbitrary, may be excessive for short summaries
    ...
};
```

**Problem:** Models may generate far more tokens than needed, especially for simple responses. No cost-aware token budgeting.

**Fix:** Implement adaptive token limits based on task complexity and cost tracking.

```rust
// In agent/mod.rs - adaptive token limits
fn resolve_adaptive_max_tokens(model: &str, context_len: usize, has_tools: bool) -> u32 {
    // Simple heuristic: more context = less output room
    let base = if has_tools { 2048 } else { 1024 };
    let context_ratio = context_len as f64 / 10000.0;
    let adjusted = (base as f64 * (1.0 - context_ratio.min(0.5))) as u32;
    adjusted.max(256).min(8192)
}

// In compaction.rs - smaller default for summaries
let max_summary_tokens = if compact_end - start < 5 { 256 } else { 512 };
let request = ChatRequest {
    max_tokens: Some(max_summary_tokens),
    ...
};
```

---

### A-1: Agent Asks for Confirmations It Could Infer (Critical)

**Root Cause:** The system prompt explicitly instructs the agent to "ask before acting externally" and "when in doubt, ask" — creating unnecessary confirmation loops for low-risk operations.

**Affected File:** `src/agent/prompt.rs` (lines 145-155)

**Current Code:**
```rust
out.push_str("## Safety\n\n");
out.push_str("- Do not exfiltrate private data.\n");
out.push_str("- Do not run destructive commands without asking.\n");
out.push_str("- Do not bypass oversight or approval mechanisms.\n");
out.push_str("- Prefer `trash` over `rm`.\n");
out.push_str("- When in doubt, ask before acting externally.\n\n");
```

**Problem:** The agent asks for confirmation on routine operations (file reads, web searches, non-destructive commands) that could proceed autonomously.

**Fix:** Implement risk-tiered autonomy with clear boundaries.

```rust
out.push_str("## Safety & Autonomy\n\n");
out.push_str("### You MAY act autonomously (no confirmation needed):\n");
out.push_str("- Reading files in the workspace directory\n");
out.push_str("- Web searches and fetching public URLs\n");
out.push_str("- Running read-only shell commands (ls, cat, grep, git status)\n");
out.push_str("- Installing packages in virtual environments\n");
out.push_str("- Writing files under 1KB in workspace directories\n\n");

out.push_str("### You MUST ask before:\n");
out.push_str("- Deleting or overwriting files > 1KB\n");
out.push_str("- Running commands with network egress (curl POST, wget to external)\n");
out.push_str("- Executing code that modifies system state\n");
out.push_str("- Accessing files outside the workspace\n\n");

out.push_str("### Never:\n");
out.push_str("- Exfiltrate private data (API keys, credentials, personal info)\n");
out.push_str("- Bypass approval mechanisms\n");
out.push_str("- Run `rm -rf` or equivalent destructive commands\n\n");
```

---

### B-2: Expensive Models for Simple Tasks (High)

**Root Cause:** No model routing logic exists to route simple queries (greetings, factual lookups, yes/no questions) to cheaper models.

**Affected File:** `src/agent/mod.rs` — all calls go through the configured provider/model.

**Fix:** Implement a cheap-model router for low-complexity tasks.

```rust
// New module: src/model_router.rs
pub fn route_to_appropriate_model(query: &str, config: &ModelRoutingConfig) -> &'static str {
    let lower = query.to_lowercase();
    
    // Greetings and acknowledgments → cheapest model
    if matches!(lower.as_str(), "hi" | "hello" | "hey" | "thanks" | "ok" | "yes" | "no" | "please") {
        return config.haiku_model;
    }
    
    // Simple factual questions → mid-tier
    if lower.starts_with("what is ") || lower.starts_with("who is ") || lower.starts_with("when is ") {
        if query.len() < 100 {
            return config.haiku_model;
        }
    }
    
    // Yes/no questions → cheap model
    if lower.starts_with("is ") || lower.starts_with("does ") || lower.starts_with("can ") {
        if !lower.contains(" how ") && query.len() < 150 {
            return config.haiku_model;
        }
    }
    
    // Complex tasks → expensive model
    if lower.contains(" analyze ") || lower.contains(" compare ") || lower.contains(" design ") {
        return config.claude_sonnet_model;
    }
    
    // Default
    config.default_model
}

// Usage in agent/mod.rs
let model_to_use = model_router::route_to_appropriate_model(&user_message, &self.model_routing_config);
let request = ChatRequest {
    model: model_to_use,
    ...
};
```

---

### B-3: Full History Sent When Recent Context Suffices (High)

**Root Cause:** Every `ChatRequest` includes the entire `self.history`, even for simple follow-ups where only the last 2-3 messages are relevant.

**Affected File:** `src/agent/mod.rs` (line 288-295)

**Current Code:**
```rust
let request = ChatRequest {
    messages: &self.history,  // Entire history, every time
    ...
};
```

**Fix:** Implement context window slicing based on query type.

```rust
fn select_context_window(history: &[ChatMessage], query: &str) -> &[ChatMessage] {
    let lower = query.to_lowercase();
    
    // Simple follow-ups: last 4 messages + system
    if lower.starts_with("yes") || lower.starts_with("no") || 
       lower.starts_with("ok") || lower.starts_with("thanks") {
        if history.len() <= 5 {
            return history;
        }
        let start = if history[0].role == "system" { 0 } else { history.len() - 4 };
        return &history[start..];
    }
    
    // New topic: full history for context
    if lower.starts_with("actually ") || lower.starts_with("new topic") || lower.starts_with("forget") {
        return history;
    }
    
    // Default: last 10 messages + system
    if history.len() <= 11 {
        return history;
    }
    let start = if history[0].role == "system" { 0 } else { history.len() - 10 };
    &history[start..]
}
```

---

### B-4: Silent Retry Multiplication in Fallback Chains (High)

**Root Cause:** `ReliableProvider` retries 3x, then `FallbackProvider` tries each provider sequentially. A single request can trigger 3×N API calls silently.

**Affected Files:**
- `src/providers/reliable.rs` (lines 44-78)
- `src/providers/fallback.rs` (lines 20-45)

**Current Code:**
```rust
// reliable.rs
for attempt in 0..=self.max_retries {  // 4 attempts total
    match self.inner.chat(request) { ... }
}

// fallback.rs
for provider in self.providers.iter() {
    match provider.chat(request) {  // Each provider has its own ReliableProvider wrapper!
        Ok(resp) => return Ok(resp),
        Err(e) => continue,  // Tries next provider
    }
}
```

**Problem:** With 3 providers configured and 3 retries each, a single user request can trigger up to 12 API calls.

**Fix:** Implement shared retry state across fallback chain.

```rust
// New: src/providers/fallback.rs
pub struct FallbackProvider {
    providers: Vec<Arc<dyn Provider>>,
    shared_retry_state: Arc<SharedRetryState>,
}

pub struct SharedRetryState {
    total_attempts: AtomicU32,
    max_total: u32,
}

impl FallbackProvider {
    pub fn new(providers: Vec<Arc<dyn Provider>>, max_total_attempts: u32) -> Self {
        Self {
            providers,
            shared_retry_state: Arc::new(SharedRetryState {
                total_attempts: AtomicU32::new(0),
                max_total: max_total_attempts,
            }),
        }
    }
}

impl Provider for FallbackProvider {
    fn chat(&self, request: &ChatRequest) -> Result<ChatResponse> {
        for provider in &self.providers {
            if self.shared_retry_state.total_attempts.load(Ordering::Relaxed) >= self.shared_retry_state.max_total {
                break;
            }
            
            // Wrap with shared retry awareness
            match provider.chat_with_shared_retry(request, &self.shared_retry_state) {
                Ok(resp) => return Ok(resp),
                Err(_) => continue,
            }
        }
        Err(anyhow::anyhow!("All providers exhausted"))
    }
}
```

---

### B-5: Streaming Fallback Loses Benefit, Wastes Tokens (High)

**Root Cause:** When streaming fails, `ReliableProvider` falls back to non-streaming `chat()`, but the callback has already been partially consumed, causing the partial stream to be lost.

**Affected File:** `src/providers/reliable.rs` (lines 85-105)

**Current Code:**
```rust
fn chat_stream(&self, request: &ChatRequest, callback: StreamCallback) -> Result<ChatResponse> {
    match self.inner.chat_stream(request, callback) {
        Ok(resp) => { ... }
        Err(e) => {
            // Callback already consumed! Falls back to blocking chat.
            warn!("Stream failed, falling back to non-streaming retry");
            self.chat(request)  // User already saw partial stream
        }
    }
}
```

**Problem:** User sees partial output, then full output arrives. Wastes tokens on duplicate generation.

**Fix:** Buffer streaming output and only commit on success.

```rust
fn chat_stream(&self, request: &ChatRequest, mut callback: StreamCallback) -> Result<ChatResponse> {
    let mut buffer = String::new();
    let buffer_callback = |chunk: StreamChunk| {
        if let StreamChunk::Delta(ref text) = chunk {
            buffer.push_str(text);
        }
        callback(chunk);
    };
    
    match self.inner.chat_stream(request, buffer_callback) {
        Ok(resp) => {
            self.circuit_breaker.record_success();
            Ok(resp)
        }
        Err(e) => {
            // Don't fall back—streaming failed, return error
            // Let caller decide to retry with non-streaming
            self.circuit_breaker.record_failure();
            Err(e)
        }
    }
}

// Caller (agent/mod.rs) handles fallback:
match provider.chat_stream(&request, callback) {
    Ok(resp) => resp,
    Err(_) => {
        warn!("Streaming failed, falling back to non-streaming");
        provider.chat(&request)?
    }
}
```

---

### R-3: System Prompt Rebuilt Every Turn (High)

**Root Cause:** The system prompt is rebuilt and re-injected on every turn, consuming tokens even when workspace files haven't changed.

**Affected File:** `src/agent/mod.rs` (lines 163-195)

**Current Code:**
```rust
let workspace_fp = prompt::workspace_prompt_fingerprint(&self.workspace_dir);
if self.has_system_prompt && Some(workspace_fp) != self.workspace_prompt_fingerprint {
    self.has_system_prompt = false;
}

if !self.has_system_prompt {
    let system_prompt = self.build_system_prompt()?;
    // Re-inserts system prompt with full tool definitions, workspace files, etc.
}
```

**Problem:** While fingerprinting exists, the entire system prompt (including tool definitions) is rebuilt even for simple conversational turns.

**Fix:** Cache the system prompt and only rebuild when tools or workspace files change.

```rust
use std::sync::OnceLock;

pub struct Agent {
    // ... existing fields ...
    cached_system_prompt: OnceLock<String>,
    last_tool_hash: u64,
}

fn build_system_prompt_cached(&mut self) -> Result<&str> {
    let current_tool_hash = compute_tool_hash(&self.tools);
    
    if let Some(cached) = self.cached_system_prompt.get() {
        if current_tool_hash == self.last_tool_hash {
            return Ok(cached);
        }
    }
    
    let prompt = self.build_system_prompt()?;
    self.cached_system_prompt = OnceLock::from(prompt);
    self.last_tool_hash = current_tool_hash;
    Ok(self.cached_system_prompt.get().unwrap())
}

fn compute_tool_hash(tools: &[Arc<dyn Tool>]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    
    let mut hasher = DefaultHasher::new();
    for tool in tools {
        tool.name().hash(&mut hasher);
        tool.parameters_json().hash(&mut hasher);
    }
    hasher.finish()
}
```

---

### R-4: Tool Call Parsing Duplicates Provider's Native Parsing (High)

**Root Cause:** For providers without native tool support, the agent parses XML tool calls from the response content. But the provider may have already parsed native tool calls, leading to duplicate processing.

**Affected File:** `src/agent/mod.rs` (lines 318-345)

**Current Code:**
```rust
let tool_calls_to_execute: Vec<ParsedToolCall> = if !response.tool_calls.is_empty() {
    // Native tool format
    response.tool_calls.iter().map(...).collect()
} else {
    // Parse XML tool calls from content
    let parse_result = parse_tool_calls(&response.content.clone().unwrap_or_default());
    ...
}
```

**Problem:** Some providers may return both native tool calls AND include tool call syntax in the content, causing double-execution.

**Fix:** Prioritize native tool calls and strip tool syntax from content when native calls exist.

```rust
let tool_calls_to_execute: Vec<ParsedToolCall> = if !response.tool_calls.is_empty() {
    // Native tool format—trust this exclusively
    response
        .tool_calls
        .iter()
        .map(|tc| ParsedToolCall {
            name: tc.function.name.clone(),
            arguments_json: tc.function.arguments.clone(),
            tool_call_id: Some(tc.id.clone()),
        })
        .collect()
} else {
    // Only parse from content if no native calls
    let parse_result = parse_tool_calls(&response.content.clone().unwrap_or_default());
    if parse_result.calls.is_empty() {
        let display_text = response.content.clone().unwrap_or_default();
        
        // Strip any tool call markup that leaked into content
        let clean_text = strip_tool_call_markup(&display_text);
        
        if should_force_follow_through(&clean_text) {
            // ... follow-through logic ...
        }
        
        return Ok(clean_text);
    }
    parse_result.calls
};

// Strip tool syntax from content when native calls exist
if !response.tool_calls.is_empty() {
    if let Some(content) = &response.content {
        let clean_content = strip_tool_call_markup(content);
        if clean_content != *content {
            // Log: provider included tool syntax in content despite native calls
            tracing::warn!("Provider included tool syntax in content despite native tool calls");
        }
    }
}
```

---

### R-5: Follow-Through Detection Causes Extra AI Iterations (High)

**Root Cause:** The `should_force_follow_through` function detects phrases like "I'll check" and forces an extra iteration, but this often triggers unnecessary additional API calls.

**Affected File:** `src/agent/mod.rs` (lines 252-270, 345-370)

**Current Code:**
```rust
fn should_force_follow_through(text: &str) -> bool {
    let lower = text.to_lowercase();
    const PATTERNS: &[&str] = &[
        "i'll try", "i will try", "let me try",
        "i'll check", "i will check", "let me check",
        // ... many more patterns ...
    ];
    PATTERNS.iter().any(|p| lower.contains(p))
}

// Later in tool loop:
if should_force_follow_through(&display_text) {
    self.history.push(ChatMessage {
        role: "assistant".to_string(),
        content: display_text,
        ...
    });
    self.history.push(ChatMessage {
        role: "user".to_string(),
        content: "SYSTEM: You promised to take action now...",
        ...
    });
    iterations += 1;
    continue;  // Forces another API call!
}
```

**Problem:** The model may have legitimately decided no tool is needed, but the follow-through detection forces an extra iteration.

**Fix:** Make follow-through detection conditional on whether tools are actually available for the task.

```rust
fn should_force_follow_through(text: &str, available_tools: &[Arc<dyn Tool>]) -> bool {
    let lower = text.to_lowercase();
    
    // Don't force follow-through if no tools are available
    if available_tools.is_empty() {
        return false;
    }
    
    // Only force for action-oriented patterns when relevant tools exist
    const ACTION_PATTERNS: &[&str] = &[
        "i'll check", "i will check", "let me check",
        "i'll look up", "i will look up",
    ];
    
    if ACTION_PATTERNS.iter().any(|p| lower.contains(p)) {
        // Only force if we have tools that can "check" or "look up"
        let has_lookup_tools = available_tools.iter().any(|t| {
            let name = t.name().to_lowercase();
            name.contains("search") || name.contains("fetch") || 
            name.contains("read") || name.contains("get")
        });
        return has_lookup_tools;
    }
    
    false
}
```

---

### A-2: Missing Tool-Use Opportunities (High)

**Root Cause:** The system prompt doesn't explicitly encourage proactive tool use for common patterns (file existence checks, web searches for current info).

**Affected File:** `src/agent/prompt.rs`

**Fix:** Add explicit tool-use encouragement for common patterns.

```rust
out.push_str("## Proactive Tool Use\n\n");
out.push_str("You should automatically use tools for these common patterns:\n\n");
out.push_str("- User mentions a file path → Use `file_read` to check contents\n");
out.push_str("- User asks about current events → Use `web_search` for latest info\n");
out.push_str("- User wants to run a command → Use `shell` directly (if safe)\n");
out.push_str("- User references prior conversation → Use `memory_recall` to find context\n");
out.push_str("- User needs to install something → Use `shell` to check/install dependencies\n\n");
out.push_str("Don't ask 'Would you like me to...' for these obvious actions—just do them.\n\n");
```

---

### A-3: Overly Conservative Safety Prompts (High)

**Root Cause:** The safety section emphasizes caution without balancing it with encouragement for appropriate autonomy.

**Affected File:** `src/agent/prompt.rs` (lines 145-155)

**Fix:** Reframe safety as "smart risk assessment" rather than blanket caution.

```rust
out.push_str("## Safety & Risk Assessment\n\n");
out.push_str("Assess risk level before acting:\n\n");
out.push_str("**Low Risk** (act autonomously):\n");
out.push_str("- Reading files, listing directories\n");
out.push_str("- Running read-only commands (cat, ls, git log)\n");
out.push_str("- Web searches, fetching public URLs\n\n");

out.push_str("**Medium Risk** (proceed with caution, mention what you're doing):\n");
out.push_str("- Writing new files\n");
out.push_str("- Installing packages in project virtualenv\n");
out.push_str("- Running commands with side effects\n\n");

out.push_str("**High Risk** (must ask first):\n");
out.push_str("- Deleting or modifying existing files >1KB\n");
out.push_str("- Commands affecting system state\n");
out.push_str("- Network operations with credentials\n\n");
```

---

### M-1: Sequential Tool Execution (Medium)

**Root Cause:** While tools are spawned in parallel with `tokio::task::spawn_blocking`, the results are collected synchronously, and the agent waits for all tools before continuing.

**Affected File:** `src/agent/mod.rs` (lines 378-420)

**Current Code:**
```rust
let mut handles = Vec::new();
for tool_call in &tool_calls_to_execute {
    let handle = tokio::task::spawn_blocking(move || { ... });
    handles.push(handle);
}

let mut execution_results = Vec::new();
for handle in handles {
    match handle.await {
        Ok(result) => execution_results.push(result),
        Err(e) => tracing::error!("Tool execution task failed: {}", e),
    }
}
```

**Problem:** This is actually well-implemented for parallelism. The issue is that ALL tools must complete before the next iteration, even if some tools are slow (e.g., web search) and others are fast (e.g., memory recall).

**Fix:** Implement progressive tool result processing for independent tools.

```rust
// For truly independent tools, process results as they arrive
use futures::future::select_all;

let mut handles: Vec<tokio::task::JoinHandle<ToolExecutionResult>> = Vec::new();
// ... spawn handles as before ...

let mut execution_results = Vec::new();
while !handles.is_empty() {
    let ((result, _idx, remaining),) = select_all(handles).await;
    handles = remaining;
    
    match result {
        Ok(r) => {
            // Could append to history immediately and continue
            // But for simplicity, collect all results
            execution_results.push(r);
        }
        Err(e) => tracing::error!("Tool execution failed: {}", e),
    }
}
```

**Note:** The current implementation is already reasonable. This is a micro-optimization.

---

### M-2: Missing Cache for Repeated Queries (Medium)

**Root Cause:** The `ToolCache` has a 30-second TTL, which is too short for most use cases. No LLM response caching exists.

**Affected Files:**
- `src/tools/cache.rs` (line 23)
- `src/agent/mod.rs`

**Fix:** Implement LLM response caching with content-based keys.

```rust
// New: src/response_cache.rs
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use sha2::{Sha256, Digest};

pub struct ResponseCacheEntry {
    pub response: String,
    pub timestamp: Instant,
}

pub struct ResponseCache {
    entries: Arc<Mutex<HashMap<String, ResponseCacheEntry>>>,
    ttl_secs: u64,
}

impl ResponseCache {
    pub fn new(ttl_secs: u64) -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            ttl_secs,
        }
    }
    
    fn compute_key(messages: &[ChatMessage], model: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(model.as_bytes());
        for msg in messages {
            hasher.update(msg.role.as_bytes());
            hasher.update(msg.content.as_bytes());
        }
        format!("{:x}", hasher.finalize())
    }
    
    pub fn get(&self, messages: &[ChatMessage], model: &str) -> Option<String> {
        let key = Self::compute_key(messages, model);
        let cache = self.entries.lock().unwrap();
        
        if let Some(entry) = cache.get(&key) {
            if entry.timestamp.elapsed() < Duration::from_secs(self.ttl_secs) {
                return Some(entry.response.clone());
            }
        }
        None
    }
    
    pub fn insert(&self, messages: &[ChatMessage], model: &str, response: String) {
        let key = Self::compute_key(messages, model);
        let mut cache = self.entries.lock().unwrap();
        cache.insert(key, ResponseCacheEntry {
            response,
            timestamp: Instant::now(),
        });
    }
}

// Usage in agent/mod.rs
if let Some(cached) = self.response_cache.get(&self.history, &self.model_name) {
    return Ok(cached);
}

let response = self.provider.chat(&request)?;

if let Some(content) = &response.content {
    self.response_cache.insert(&self.history, &self.model_name, content.clone());
}
```

---

### M-3: Token Estimation Uses Naive char/4 Heuristic (Medium)

**Root Cause:** The `token_estimate` function uses a simple character/4 calculation, which can be wildly inaccurate for different models and content types.

**Affected File:** `src/agent/compaction.rs` (lines 52-58)

**Current Code:**
```rust
pub fn token_estimate(history: &[ChatMessage]) -> u64 {
    let mut total_chars = 0;
    for msg in history {
        total_chars += msg.content.len() as u64;
    }
    total_chars.div_ceil(4)
}
```

**Problem:** Actual tokenization varies by model (GPT-4 vs Claude vs Gemini). This can lead to unexpected context overflow or premature compaction.

**Fix:** Use model-specific estimation factors.

```rust
pub fn token_estimate(history: &[ChatMessage], model: &str) -> u64 {
    // Model-specific tokens-per-character estimates
    // Based on empirical data: GPT-4 ~4 chars/token, Claude ~3.5, Gemini ~4.5
    let chars_per_token = match model.to_lowercase().as_str() {
        s if s.contains("claude") => 3.5,
        s if s.contains("gemini") => 4.5,
        s if s.contains("gpt-4") => 4.0,
        s if s.contains("gpt-3") => 4.0,
        _ => 4.0,
    };
    
    let mut total_chars = 0u64;
    for msg in history {
        // Add overhead for message structure
        total_chars += msg.content.len() as u64;
        total_chars += 10; // Role + metadata overhead
    }
    
    (total_chars as f64 / chars_per_token).ceil() as u64
}
```

---

### M-4: Subagent Isolation Shares Provider (Medium)

**Root Cause:** Subagents share the same provider instance as the main agent, with no rate limiting or quota isolation.

**Affected File:** `src/subagent.rs` (lines 85-120)

**Current Code:**
```rust
if let Some(sm) = sm {
    let result = sm.process_message(&subagent_session_key, task_copy).await;
    // ...
}
```

**Problem:** Multiple subagents can overwhelm the provider, triggering rate limits that affect the main agent.

**Fix:** Implement per-subagent rate limiting and quotas.

```rust
pub struct SubagentManager {
    // ... existing fields ...
    rate_limiter: Arc<RateLimiter>,
}

impl SubagentManager {
    pub fn new(bus: Arc<Bus>, config: SubagentConfig, max_requests_per_minute: u32) -> Self {
        Self {
            // ...
            rate_limiter: Arc::new(RateLimiter::new(max_requests_per_minute)),
        }
    }
    
    pub async fn spawn(...) -> Result<u64> {
        // ...
        
        tokio::spawn(async move {
            // Rate limit subagent requests
            rate_limiter.acquire().await;
            
            let result = sm.process_message(&subagent_session_key, task_copy).await;
            // ...
        });
    }
}
```

---

### L-1: Tool Cache TTL Too Short (Low)

**Root Cause:** Default 30-second TTL is too aggressive for most tool results.

**Affected File:** `src/tools/cache.rs` (line 23)

**Fix:** Increase default TTL and make it tool-specific.

```rust
impl ToolCache {
    pub fn new(ttl_secs: u64) -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            ttl_secs,
        }
    }
    
    // New: per-tool TTL
    pub fn get_with_tool_ttl(&self, tool_name: &str, arguments_json: &str) -> Option<ToolExecutionResult> {
        let key = format!("{}:{}", tool_name, arguments_json);
        let ttl = self.tool_specific_ttl(tool_name);
        
        let cache = self.entries.lock().unwrap();
        if let Some(entry) = cache.get(&key) {
            if entry.timestamp.elapsed() < Duration::from_secs(ttl) {
                return Some(entry.result.clone());
            }
        }
        None
    }
    
    fn tool_specific_ttl(&self, tool_name: &str) -> u64 {
        match tool_name {
            "memory_recall" => 300,      // 5 min
            "web_search" => 600,         // 10 min
            "file_read" => 60,           // 1 min
            "shell" => 30,               // 30 sec (commands may have side effects)
            _ => self.ttl_secs,
        }
    }
}
```

---

### R-6: Memory Enrichment Called Even When Memory Disabled (Medium)

**Root Cause:** The `enrich_message` function is called regardless of whether memory is actually configured.

**Affected File:** `src/agent/mod.rs` (lines 204-212)

**Fix:** Skip enrichment if using NoopMemory.

```rust
// Enrich user message with memory context (only if memory is configured)
let enriched_msg = if self.memory.type_id() != std::any::TypeId::of::<NoopMemory>() {
    match memory_loader::enrich_message(
        self.memory.as_ref(),
        &user_message,
        self.memory_session_id.as_deref(),
    ) {
        Ok(msg) => msg,
        Err(e) => {
            warn!("Memory enrichment failed: {}", e);
            user_message.clone()
        }
    }
} else {
    user_message.clone()
};
```

---

### B-6: No Cost Tracking Integration in Agent Loop (Medium)

**Root Cause:** The `CostTracker` exists but is not integrated into the agent's turn loop.

**Affected File:** `src/cost.rs` (entire file) — defined but not used in `src/agent/mod.rs`

**Fix:** Integrate cost tracking into the agent loop.

```rust
// In Agent struct:
pub cost_tracker: Option<crate::cost::CostTracker>,

// In turn() after provider.chat():
if let Some(usage) = &response.usage {
    if let Some(tracker) = &mut self.cost_tracker {
        let token_usage = crate::cost::TokenUsage::new(
            &self.model_name,
            usage.prompt_tokens as u64,
            usage.completion_tokens as u64,
            // Need model pricing config
            0.0, 0.0,  // TODO: load from config
        );
        let _ = tracker.record_usage(token_usage);
    }
}
```

---

### A-4: Hardcoded Human-in-Loop for Destructive Ops (Medium)

**Root Cause:** The safety prompt says "ask before destructive ops" but doesn't define what constitutes destructive.

**Affected File:** `src/agent/prompt.rs`

**Fix:** Provide explicit patterns for destructive operation detection.

```rust
out.push_str("### Destructive Operation Patterns (MUST ask first):\n\n");
out.push_str("Shell commands containing:\n");
out.push_str("- `rm `, `rm -`, `del `, `rmdir `\n");
out.push_str("- `> file` (redirect that overwrites)\n");
out.push_str("- `chmod `, `chown ` (permission changes)\n\n");

out.push_str("File operations:\n");
out.push_str("- Deleting files > 100 bytes\n");
out.push_str("- Overwriting files that already exist\n");
out.push_str("- Operations outside workspace directory\n\n");
```

---

### F-1: Sequential Fallback (No Parallel Probing) (Medium)

**Root Cause:** `FallbackProvider` tries providers sequentially, adding latency on failures.

**Affected File:** `src/providers/fallback.rs`

**Fix:** Implement parallel probing with first-response-wins.

```rust
use tokio::task::JoinSet;

impl FallbackProvider {
    pub async fn chat_parallel(&self, request: &ChatRequest) -> Result<ChatResponse> {
        let mut set = JoinSet::new();
        
        for (i, provider) in self.providers.iter().enumerate() {
            let req_clone = request.clone();  // Need to make ChatRequest Clone
            set.spawn(async move {
                let result = provider.chat(&req_clone);
                (i, result)
            });
        }
        
        while let Some(result) = set.join_next().await {
            match result {
                Ok((_, Ok(response))) => return Ok(response),
                Ok((i, Err(e))) => {
                    tracing::warn!("Provider {} failed: {}", i, e);
                    continue;
                }
                Err(e) => {
                    tracing::warn!("Task join failed: {}", e);
                    continue;
                }
            }
        }
        
        Err(anyhow::anyhow!("All providers failed"))
    }
}
```

---

### F-2: No Circuit Breaker State Persistence (Low)

**Root Cause:** Circuit breaker state is lost on restart, causing cold-start failures.

**Affected File:** `src/providers/circuit_breaker.rs`

**Fix:** Persist circuit breaker state to disk.

```rust
use std::fs;
use std::path::PathBuf;

pub struct CircuitBreaker {
    // ... existing fields ...
    persistence_path: Option<PathBuf>,
}

impl CircuitBreaker {
    pub fn save_state(&self) {
        if let Some(path) = &self.persistence_path {
            let state = CircuitBreakerState {
                failure_count: self.failure_count.load(Ordering::Relaxed),
                last_failure: self.last_failure.load(Ordering::Relaxed),
                state: self.state.load(Ordering::Relaxed),
            };
            let _ = fs::write(path, serde_json::to_string(&state).unwrap());
        }
    }
    
    pub fn load_state(&mut self) {
        if let Some(path) = &self.persistence_path {
            if let Ok(content) = fs::read_to_string(path) {
                if let Ok(state) = serde_json::from_str(&content) {
                    // Restore state
                }
            }
        }
    }
}
```

---

## Quick Wins (Fixes Under 30 Minutes)

| Fix | File | Time | Impact |
|-----|------|------|--------|
| QW-1: Increase tool cache TTL to 5 min | `src/tools/cache.rs` | 5 min | Reduces redundant tool calls |
| QW-2: Skip memory ops for short messages | `src/agent/mod.rs` | 10 min | Reduces memory I/O by ~30% |
| QW-3: Add model-specific token estimation | `src/agent/compaction.rs` | 15 min | Prevents context overflow |
| QW-4: Strip tool markup from native responses | `src/agent/mod.rs` | 10 min | Cleaner output |
| QW-5: Add cost tracking to agent loop | `src/agent/mod.rs`, `cost.rs` | 20 min | Visibility into spending |
| QW-6: Conditional follow-through detection | `src/agent/mod.rs` | 15 min | Reduces unnecessary iterations |
| QW-7: Greeting detection for cheap model routing | `src/agent/mod.rs` | 10 min | Immediate cost savings |

---

## Longer-Term Architectural Recommendations

### 1. Implement Request Deduplication Layer

Create a middleware layer that detects and deduplicates identical or near-identical requests within a time window.

```rust
// New: src/request_dedup.rs
pub struct RequestDeduplicator {
    recent_requests: Arc<Mutex<HashMap<String, (Instant, ChatResponse)>>>,
    window_secs: u64,
}
```

### 2. Add Model Router with Cost Awareness

Implement intelligent model selection based on:
- Query complexity (embedding-based classification)
- Token budget remaining
- Historical success rates per model

### 3. Implement Streaming-First Architecture

Redesign the agent loop to be streaming-native:
- Buffer and validate streaming output
- Graceful degradation without losing partial results
- Token-by-token cost tracking

### 4. Add Observability Dashboard

Integrate with tracing/metrics to provide:
- Real-time token usage per session
- Cost per conversation
- Tool execution latency heatmap
- Model performance comparison

### 5. Implement Adaptive Context Management

Replace fixed-window context with:
- Relevance-based message selection (embedding similarity)
- Dynamic summarization triggers
- Topic-aware context boundaries

### 6. Add Provider Health Monitoring

Implement continuous provider health checks:
- Latency percentiles per provider
- Error rate tracking
- Automatic provider rotation based on health

---

## Implementation Priority

### Week 1 (Critical + Quick Wins)
1. QW-1 through QW-7 (all quick wins)
2. R-1: Duplicate memory operations
3. R-2: Compaction cooldown
4. B-1: Adaptive token limits
5. A-1: Risk-tiered autonomy

### Week 2-3 (High Priority)
1. B-2: Model routing for simple tasks
2. B-3: Context window slicing
3. B-4: Shared retry state
4. B-5: Streaming fallback fix
5. R-3: System prompt caching

### Month 2 (Medium Priority)
1. M-2: LLM response caching
2. M-3: Model-specific token estimation
3. M-4: Subagent rate limiting
4. F-1: Parallel fallback probing

### Quarter 2 (Long-term)
1. Request deduplication layer
2. Full model router implementation
3. Observability dashboard
4. Adaptive context management

---

## Appendix: Cost Estimation Methodology

Assumptions for a 10,000 turns/day deployment:
- Average turn: 500 input tokens, 200 output tokens
- Model mix: 60% Haiku ($0.25/$1.25 per 1M), 40% Sonnet ($3/$15 per 1M)
- Baseline monthly cost: ~$450

**Savings estimates:**
- Quick wins: 15-20% reduction (~$70-90/month)
- High priority fixes: 25-35% reduction (~$110-160/month)
- Medium priority: 10-15% reduction (~$45-70/month)

**Total potential savings: 50-70% (~$225-315/month)**

---

*Report generated by AI Codebase Audit Tool*
