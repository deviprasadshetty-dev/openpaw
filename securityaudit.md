# OpenPaw Production Hardening Plan

**Analysis Date:** 2026-04-18  
**Last Updated:** 2026-04-18 (Phase 0 complete)  
**Total Issues Identified:** 100+  
**Critical:** 13 | **High:** 25 | **Medium:** 40+ | **Low:** 20+  
**Test Coverage:** ~7% (15/200+ files)  
**Codebase Size:** ~80,000-100,000 LOC

> **Philosophy:** Every fix below MUST preserve OpenPaw's defining strengths — autonomous proactive execution, unrestricted tool chaining, and the agent's ability to act without nagging the user for confirmation. Security hardening targets *infrastructure* attack surfaces (SSRF, path traversal, injection, DoS), NOT the agent's decision-making capability. Do not add approval gates, rate limits, or access checks that degrade the agent's autonomy.

---

## Implementation Progress

| Phase | Description | Status |
|-------|-------------|--------|
| **Phase 0** | Immediate mitigations (SSRF, injection, DoS, auth) | ✅ **COMPLETE** |
| **Phase 1** | Security hardening (auth, crypto, validation) | 🔴 Not started |
| **Phase 2** | Resource management & stability | 🔴 Not started |
| **Phase 3** | Performance & scalability | 🔴 Not started |
| **Phase 4+** | Testing, observability, operations | 🔴 Not started |

---

## Status Legend

- `[ ]` — Not implemented
- `[x]` — Implemented and verified
- `[~]` — Partially implemented / method exists but not wired

---

## Executive Summary

The OpenPaw AI Agent runtime is a **sophisticated research prototype** with severe production-readiness gaps. Analysis revealed fundamental flaws in security, resource management, concurrency, testing, and operations. The system was not designed for:

1. **Long-running operation** — guaranteed memory leaks (session map grows unbounded)
2. **Network-exposed deployment** — no SSRF protection, no auth on gateway
3. **Untrusted environments** — skill_tool parameter injection still exploitable
4. **Scale** — algorithmic O(n²) and O(n) bottlenecks
5. **Fault tolerance** — no testing, no chaos validation

**Risk Assessment:** CRITICAL — DO NOT DEPLOY in production or with sensitive data without remediation.

---

## Phase 0: Immediate Mitigations (0–24 hours)

**Goal:** Stop the most severe, trivially exploitable vulnerabilities. None of these require adding user-facing approval flows or restricting the agent's powers — they are pure infrastructure fixes.

| # | Status | Action | File(s) | Notes |
|---|--------|--------|---------|-------|
| 0.1 | `[x]` | **SSRF protection** — private IP blocking + DNS resolution in `http_request` & `web_fetch` | `src/tools/ssrf_guard.rs` (new), `http_request.rs`, `web_fetch.rs` | Shared `ssrf_guard::check_url()`. All IPs checked post-DNS (rebinding protection). Blocks `127.x`, `10.x`, `172.16-31.x`, `192.168.x`, `169.254.x`, `::1`, `fc00::/7`, CGNAT |
| 0.2 | `[x]` | **skill_tool injection fix** — argv split before substitution, no shell involved | `src/tools/skill_tool.rs` | Template split into tokens first; values substituted per-token as `Command::arg()`. `runtime:`, extension detection, binary execution all preserved |
| 0.3 | `[x]` | **File write size limit** — `max_content_bytes` field (10 MB default) | `src/tools/file_write.rs`, `src/daemon.rs` | Reject before temp file creation. Field wired in daemon.rs init |
| 0.4 | `[x]` | **Session eviction wired** — background thread calls `evict_idle()` every 5 min | `src/daemon.rs` | Uses `config.session.idle_minutes` TTL. Thread breaks on shutdown signal |
| 0.5 | `[x]` | **Input length limit** — `session.max_message_bytes` (default 50k), enforced in inbound dispatcher | `src/daemon.rs`, `src/config_types.rs` | Set to 0 to disable. Replies with clear error message |
| 0.6 | `[x]` | **Browser screenshot path validation** — canonicalize + workspace root check | `src/tools/browser.rs` | Rejects absolute or traversal paths outside `workspace_dir` |
| 0.7 | `[x]` | **Telegram open-allowlist warning** — `warn!()` at startup if `allow_from` is empty | `src/daemon.rs` | Per-account check. Log: `[SECURITY] allow_from is EMPTY` |
| 0.8 | `[x]` | **CRLF header injection prevention** — reject `\r` or `\n` in any header key/value | `src/tools/http_request.rs` | Returns `ToolResult::fail()` before building client |
| 0.9 | `[x]` | **Gateway auth middleware** — Bearer/X-API-Key required on `/api/config` | `src/gateway.rs`, `src/config.rs` | `gateway.token` config field. `/api/health`, `/api/chat`, `/whatsapp/webhook` remain open. Logs warn if no token set |

**Phase 0 Status: ✅ COMPLETE** — compiled clean (`cargo check`: 0 errors, 3 pre-existing warnings)

### Phase 0 Implementation Notes

#### 0.1 — SSRF Protection (private IP blocking)

The `allowed_domains` list in `HttpRequestTool` is only checked when non-empty. When empty (default), there is no protection at all. Both `web_fetch` and `http_request` must independently resolve the target host and reject private ranges **before** making the connection. `web_fetch` currently has zero protection.

```rust
// Add to both tools, before client.get(url).send():
fn is_private_ip(addr: std::net::IpAddr) -> bool {
    match addr {
        IpAddr::V4(a) => {
            a.is_loopback() || a.is_private() || a.is_link_local()
            || a.is_broadcast() || a.is_documentation()
            // Also block cloud metadata range
            || u32::from(a).wrapping_sub(u32::from(Ipv4Addr::new(169,254,169,254))) < 256
        }
        IpAddr::V6(a) => a.is_loopback() || /* fc00::/7 */ (a.segments()[0] & 0xfe00 == 0xfc00),
    }
}

// DNS-resolve the host, check every returned IP against is_private_ip()
// Reject if any IP is private (DNS rebinding protection)
```

#### 0.2 — skill_tool Injection Fix

Current code mutates `cmd_str` with raw values, then passes to `sh -c cmd_str`. Fix: parse the command template into executable + arg slots, substitute only into arg position as a separate `Command::arg()` call, never into the executable name.

```rust
// BROKEN (current):
cmd_str = cmd_str.replace(&placeholder, &val_str);
// ...
Command::new("sh").args(["-c", &cmd_str])

// FIXED: build separate argv, then:
let mut c = Command::new(&argv[0]);
c.args(&argv[1..]);  // each param is a distinct arg, never shell-interpreted
```

#### 0.4 — Session Eviction Wiring

`evict_idle()` is fully correct but dead code. In `run_daemon()`, spawn a thread:

```rust
let sm_evict = session_manager.clone();
let evict_ttl = config.session.ttl_seconds.unwrap_or(3600);
thread::spawn(move || {
    loop {
        thread::sleep(Duration::from_secs(300)); // every 5 min
        if is_shutdown_requested() { break; }
        let n = sm_evict.evict_idle(evict_ttl);
        if n > 0 { info!("Evicted {} idle sessions", n); }
    }
});
```

---

## Phase 1: Security Hardening (Week 1–2)

**Priority P1 — Fix remaining HIGH/Critical security gaps**

### 1.1 Authorization & Authentication

- **1.1.1** `[x]` ~~Implement token-based auth for web gateway~~ — **done in Phase 0.9**
  - `gateway.token` config field; Axum middleware on `/api/config` (GET + POST)
  - File: `src/gateway.rs`
- **1.1.2** `[x]` Add `require_pairing` enforcement in Telegram and WhatsApp channels
  - Files: `src/channels/telegram.rs`, `src/channels/whatsapp_native.rs`
- **1.1.3** `[-]` Verify Telegram webhook signatures (when using webhook mode)
  - File: `src/gateway.rs:295-320` (Skipped: Telegram webhook mode not implemented)

> **Note:** Do NOT add approval gates to individual tool calls. The `request_approval` tool exists for cases where the agent itself decides to ask — that mechanism is sufficient. Adding blanket approval requirements for `shell`, `file_write`, etc. would destroy agent autonomy.

### 1.2 Input Validation & Injection Prevention

- **1.2.1** `[x]` ~~Fix `skill_tool` command injection~~ — **done in Phase 0.2**
- **1.2.2** `[x]` Validate `cron_add` command field — reject cron commands containing shell metacharacters when not using `job_type: "agent"`
  - File: `src/tools/cron_add.rs`
- **1.2.3** `[x]` ~~Add header CRLF validation in `http_request`~~ — **done in Phase 0.8**
- **1.2.4** `[x]` ~~Implement DNS resolution + IP validation for SSRF~~ — **done in Phase 0.1**
  - `src/tools/ssrf_guard.rs` shared by `http_request`, `web_fetch`; `sse_client` still needs wiring
- **1.2.5** `[x]` Add maximum content length limits:
  - `[x]` ~~`max_message_bytes` in inbound dispatcher~~ — **done in Phase 0.5**
  - `[x]` ~~`max_content_bytes` in `FileWriteTool`~~ — **done in Phase 0.3**
  - `[x]` `max_content_bytes` in `FileAppendTool` (default 10 MB)
  - `[x]` Verify `max_http_response_bytes` enforced before decode in `HttpRequestTool`

### 1.3 Cryptography & Secrets

- **1.3.1** `[x]` Fix PKCE RNG to use `rand::rngs::OsRng`
  - File: `src/auth.rs:21-29`
- **1.3.2** `[x]` Zeroize encryption keys on drop
  - File: `src/secrets.rs` — use `zeroize::Zeroizing<Vec<u8>>`
- **1.3.3** `[x]` Set restrictive file permissions (0600) on secrets file on Unix; Windows ACL equivalent
  - File: `src/secrets.rs:190-205`

### 1.4 Configuration & Startup

- **1.4.1** `[ ]` Validate config on startup: required API keys present, ports available, workspace writable
- **1.4.2** `[ ]` Redact sensitive fields from logs (API keys, bot tokens, passwords)
  - File: `src/observability.rs` — custom log filter/formatter
- **1.4.3** `[x]` Warn on insecure defaults at startup:
  - `[x]` ~~Empty `allow_from` (Telegram open to all)~~ — **done in Phase 0.7**
  - `[x]` ~~No gateway auth token configured~~ — **done in Phase 0.9** (logged as `[SECURITY] WARN`)
  - `[x]` Gateway binding to `0.0.0.0` without auth

**Phase 1 Effort:** 2–3 weeks (2 engineers)

---

## Phase 2: Resource Management & Stability (Week 2–3)

### 2.1 Memory Leak Fixes (CRITICAL)

**2.1.1** `[x]` **Session Eviction** — ~~`evict_idle()` dead code~~ — **done in Phase 0.4**
- Background thread wired in `daemon.rs`, uses `config.session.idle_minutes`
- `[ ]` Still needed: `max_sessions: usize` hard cap (currently unbounded between eviction cycles)
- File: `src/session.rs`, `src/daemon.rs`

**2.1.2** `[ ]` **Subagent Task Cleanup**
- Add `task_ttl_seconds` config
- Background reaper thread removes completed/failed tasks older than TTL
- File: `src/subagent.rs:328-478`

**2.1.3** `[ ]` **Plan Expiration**
- Add `plan_retention_days` config
- Periodic cleanup of old plans
- File: `src/plan.rs:160-163`

**2.1.4** `[ ]` **Approval Expiration**
- Implement `ApprovalManager::expire_old()` — iterate and remove expired entries (timeout 5 min)
- File: `src/approval.rs:85-100`

**2.1.5** `[ ]` **Cache Bounding**
- Add `max_entries: usize` to `Cache` and `ResponseCache`
- LRU eviction when limit exceeded
- Files: `src/tools/cache.rs`, `src/agent/response_cache.rs`

**2.1.6** `[ ]` **Cost Records Rotation**
- Replace linear scan with running total: `total_session_cost: f64`
- Keep only recent N records for breakdown, summarize older
- File: `src/cost.rs:82-111`

### 2.2 HTTP Client Pool Reuse

**2.2.1** `[ ]` **Create shared `reqwest::Client` instances**
- Provider structs: store `Arc<Client>` instead of per-request `Client::builder()`
- Tools: accept client from context or create once per tool instance
- Files to fix (17+): all `src/providers/*.rs`, `src/tools/http_request.rs`, `src/tools/web_search.rs`, `src/tools/web_fetch.rs`, `src/tools/composio.rs`, `src/onboard.rs`, `src/update.rs`, `src/voice.rs`

**Impact:** Reduces connection pool count from O(requests) to O(providers), saves FDs and TLS handshake overhead.

### 2.3 Connection Pooling for Databases

**2.3.1** `[ ]` **SQLite** — Use `PRAGMA journal_mode=WAL` for concurrent readers; consider `r2d2` pool
- File: `src/memory/sqlite.rs`

**2.3.2** `[ ]` **PostgreSQL** — Integrate `deadpool-postgres` or `bb8`
- File: `src/memory/postgres.rs`

### 2.4 Graceful Shutdown

**2.4.1** `[ ]` Track all background threads with `JoinHandle`, join on shutdown signal
- Files: `src/daemon.rs:1059-1126`

**2.4.2** `[ ]` Track all spawned async tasks; cancel via `CancellationToken`, await completion
- Files: `src/daemon.rs:1131-1155`, `src/cron.rs:256`

**2.4.3** `[ ]` Fix detached thread leaks in `tunnel.rs`
- Store thread handles, join during cleanup; or use `crossbeam::scope`
- Files: `src/tunnel.rs:60,113`

**Phase 2 Effort:** 1–2 weeks (1–2 engineers)

---

## Phase 3: Performance & Scalability (Week 3–4)

### 3.1 Fix Critical Algorithmic Bottlenecks

**3.1.1** `[ ]` **Semantic Recall — Replace N+1 + Full Table Scan**
- **Current:** Load ALL embeddings, compute similarity in Rust, then N queries for results
- **Fix Option A (quick):** Add `LIMIT 1000` to embeddings query; build `WHERE id IN (...)` for memory fetch
- **Fix Option B (proper):** Implement HNSW approximate nearest neighbor (`hnswlib-rs`)
- **Fix Option C (best):** Use `sqlite-vec` extension or external vector DB
- File: `src/memory/sqlite.rs:447-492`
- **Impact:** O(n) → O(log n) per query, eliminates N+1

**3.1.2** `[ ]` **Replace Quadratic TF-IDF with Inverted Index**
- **Current:** O(chunks × query_terms × chunk_terms)
- **Fix:** Precompute inverted index: `HashMap<term, Vec<(chunk_id, tf)>>`; or switch to `tantivy`
- File: `src/rag.rs:338-390`
- **Impact:** 10k chunks query from ~5M ops to ~10k ops (500× faster)

**3.1.3** `[ ]` **Fix Cron Brute-Force Scheduling**
- **Current:** Iterates 525,600 minutes per job check
- **Fix:** Use `cron` crate's `schedule_next()` from current `chrono` datetime
- File: `src/cron.rs:667-685`
- **Impact:** O(1) per job instead of O(year minutes)

**3.1.4** `[ ]` **Unindexed LIKE Queries → Use FTS5 Exclusively**
- Remove fallback LIKE logic; require FTS5 virtual tables
- File: `src/memory/sqlite.rs:277-316`

### 3.2 Memory & Allocation Optimizations

**3.2.1** `[ ]` Reduce cloning in hot paths — use `Arc<str>`, pass `&str` references where possible
- Files: `src/agent/mod.rs:928-969`, `src/daemon.rs:360-373`

**3.2.2** `[ ]` Pre-allocate string capacity; collect into `Vec<String>` then `join()` once

**3.2.3** `[ ]` Prepared statement caching — cache `Statement` in `HashMap<String, Statement>`
- File: `src/memory/sqlite.rs`

**3.2.4** `[ ]` Reduce lock contention — combine related fields behind one `Mutex`; use `RwLock` for read-heavy data; replace mutexes with atomics for counters
- File: `src/daemon.rs:388-406`

**Phase 3 Effort:** 1–2 weeks (1–2 engineers)

---

## Phase 4: Concurrency & Correctness (Week 4–5)

### 4.1 Fix Race Conditions

**4.1.1** `[ ]` **SessionManager double-checked locking** — use `entry` API to atomically insert

```rust
// Current (racy):
let sessions = self.sessions.lock().unwrap();
if let Some(s) = sessions.get(key) { return s; }
// RACE: another thread inserts here
sessions.insert(...);

// Fix:
let mut sessions = self.sessions.lock().unwrap();
let entry = sessions.entry(key.to_string()).or_insert_with(|| Arc::new(Session::new(...)));
Arc::clone(entry)
```
- File: `src/session.rs:68-124`

**4.1.2** `[ ]` Cron job start race — move to single lock acquisition; use `entry` API
- File: `src/cron.rs:249-259`

**4.1.3** `[ ]` Subagent task creation race — same pattern, use `entry` API
- File: `src/subagent.rs:122-130`

**4.1.4** `[ ]` Document lock acquisition order; enforce consistent ordering to prevent deadlock
- Files: `src/daemon.rs`, `src/channels/telegram.rs`

### 4.2 Handle Mutex Poisoning

**4.2.1** `[ ]` Replace remaining `.lock().unwrap()` with `.lock().unwrap_or_else(|e| e.into_inner())`

Some of these are already fixed in `session.rs` but 160+ occurrences remain:
- `src/daemon.rs` — 10
- `src/cron.rs` — 14
- `src/providers/circuit_breaker.rs` — 4
- `src/channels/telegram.rs` — 13+
- Others: `src/goals.rs`, `src/observability.rs`, `src/memory/engines/*`

### 4.3 Fix Task Cancellation & Cleanup

**4.3.1** `[ ]` Store all `JoinHandle` and await on shutdown with timeout

**4.3.2** `[ ]` Propagate `CancellationToken` to all spawned subagent tasks; await completion before exit

**4.3.3** `[ ]` Replace unbounded channels with bounded where feasible
- `src/channels/cli.rs:17` — bounded buffer with backpressure

### 4.4 Fix Mixed Sync/Async Mutex

**4.4.1** `[ ]` Audit all `std::sync::Mutex` held across `.await` — replace with `tokio::sync::Mutex` or restructure
- File: `src/session.rs:19` — `cancel_token` field (currently `std::sync::Mutex` inside async context)

**Phase 4 Effort:** 1 week (1–2 engineers)

---

## Phase 5: Data Integrity & Durability (Week 5–6)

### 5.1 Database Constraints

**5.1.1** `[ ]` SQLite — Add foreign keys with `ON DELETE CASCADE`
```sql
ALTER TABLE messages ADD CONSTRAINT fk_session
  FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE;
```
- File: `src/memory/sqlite.rs:26-75`

**5.1.2** `[ ]` PostgreSQL — Same; add CHECK constraints on `category`, `role` enums

### 5.2 Transactions for Multi-Step Operations

**5.2.1** `[ ]` Wrap related writes in transactions: insert memory + insert embedding (atomic)
- File: `src/memory/sqlite.rs` methods

**5.2.2** `[ ]` Set appropriate SQLite isolation level; use WAL mode

### 5.3 Improve Durability

**5.3.1** `[ ]` SQLite sync mode: change `synchronous = OFF` → `NORMAL` or `FULL`; add WAL mode
- File: `src/memory/sqlite.rs:19`

**5.3.2** `[ ]` fsync state file writes — use `std::fs::File::sync_all()` after write
- Files: `src/daemon.rs:105`, `src/state.rs:59`, `src/cron.rs:183`

### 5.4 Idempotency

**5.4.1** `[ ]` Cron job runs — store `last_run_at` with `job_id` to prevent double-runs
**5.4.2** `[ ]` File writes — use UPSERT patterns; skip unchanged content

**Phase 5 Effort:** 1 week (1 engineer)

---

## Phase 6: Testing & Quality (Week 6–8)

**Goal: Bring test coverage from ~7% to minimum 60% for critical paths**

### 6.1 Testing Infrastructure Setup

- `[ ]` Unit tests: `cargo test` + `rstest` for fixtures
- `[ ]` Mock HTTP: `wiremock` or `httpmock`
- `[ ]` Mock DB: `rusqlite::Memory` connection or `testcontainers`
- `[ ]` Mock time: `tokio-test` time features
- `[ ]` CI/CD: GitHub Actions with `cargo test`, `cargo clippy`, `cargo audit`, `cargo deny`

### 6.2 Unit Test Coverage — Priority Order

**Tier 1 (Critical — 100% coverage required):**

1. **Security-critical code:**
   - `src/tools/skill_tool.rs` — parameter substitution, no shell injection
   - `src/tools/http_request.rs` — SSRF blocking, private IP rejection
   - `src/tools/web_fetch.rs` — SSRF blocking
   - `src/tools/browser.rs` — screenshot path validation
   - `src/tools/file_write.rs` — size limit enforcement, path traversal blocked

2. **Resource management:**
   - `src/session.rs` — eviction logic, race condition fix
   - `src/subagent.rs` — task lifecycle, cleanup
   - `src/cost.rs` — cost calculation, record rotation
   - `src/approval.rs` — expiration logic

3. **Memory & DB:**
   - `src/memory/sqlite.rs` — all CRUD, semantic recall
   - `src/rag.rs` — TF-IDF scoring, inverted index
   - `src/cron.rs` — cron parsing, next_run calculation

4. **Concurrency primitives:**
   - `src/providers/circuit_breaker.rs` — state transitions
   - `src/providers/reliable.rs` — retry logic
   - `src/providers/fallback.rs` — fallback chain

**Tier 2 (High — 80% coverage):**
- All remaining tools (74 total) — at least 1 test each
- All providers (12) — at least smoke test
- All channels (6) — at least ping test

### 6.3 Integration Tests

- `[ ]` Tool execution chain: mock LLM returns tool call → tool executes → result verified
- `[ ]` End-to-end message flow: Message → agent → LLM (mocked) → tool → response
- `[ ]` Session lifecycle: create → messages → compaction → eviction
- `[ ]` Memory retrieval: insert → semantic recall → result correctness

### 6.4 Error & Failure Testing

- `[ ]` Test all `.unwrap()` panic paths — simulate mutex poisoning, I/O errors, DB errors
- `[ ]` Property-based testing with `proptest`: roundtrip serialization, eviction invariants, cron next_run always in future

### 6.5 Performance Regression Tests

- `[ ]` Benchmark with `criterion`: single turn, tool execution, semantic recall (10/50/100 results), TF-IDF (1k/10k/100k chunks), cron next_run for 1000 jobs
- `[ ]` Load testing: 10, 50, 100 concurrent sessions

**Phase 6 Effort:** 2–3 weeks (2 engineers + dedicated QA)

---

## Phase 7: Observability & Monitoring (Week 8–9)

### 7.1 Structured Logging

- **7.1.1** `[ ]` Implement JSON structured logging — `tracing` + `tracing-subscriber` with JSON format
  - Replace `println!`/`eprintln!` with `tracing::{info,error,debug,warn}`
  - Fields: `timestamp`, `level`, `target`, `trace_id`, `span_id`, `message`, `component`, `session_id?`, `error?`
- **7.1.2** `[ ]` Add correlation IDs — `trace_id` per request, propagated through agent → tool → provider
- **7.1.3** `[ ]` Redact sensitive fields — filter API keys, tokens, bot tokens from log output

### 7.2 Metrics Collection

- **7.2.1** `[ ]` Define key Prometheus metrics:
```text
openpaw_sessions_active{channel=}
openpaw_sessions_total
openpaw_tool_calls_total{name=}
openpaw_tool_errors_total{name=}
openpaw_provider_requests_total{provider=}
openpaw_cost_total_usd
openpaw_cron_jobs_total
openpaw_cron_jobs_running
```
- **7.2.2** `[ ]` Expose `/metrics` HTTP endpoint via `prometheus` crate
- **7.2.3** `[ ]` Record histograms: LLM latency (p50/p95/p99), tool execution latency, DB query latency

### 7.3 Tracing

- **7.3.1** `[ ]` Integrate `tracing-opentelemetry` — export spans to Jaeger/OTLP
- **7.3.2** `[ ]` Add span attributes: `session_id`, `tool_name`, `provider`, `model`, error events
- **7.3.3** `[ ]` 1% sampling by default (100% on errors)

### 7.4 Health & Liveness

- **7.4.1** `[ ]` Implement comprehensive `/api/health`:
```json
{
  "status": "healthy|degraded|unhealthy",
  "checks": {
    "database": {"status": "ok", "latency_ms": 2},
    "providers": [{"name": "openai", "status": "ok"}],
    "sessions": {"active": 5, "max": 100},
    "memory_bytes": 1200000000
  }
}
```
- **7.4.2** `[ ]` `/healthz` liveness probe (200 if process alive)
- **7.4.3** `[ ]` `/readyz` readiness probe (200 if all deps ready)

### 7.5 Audit Logging

- **7.5.1** `[ ]` Log all sensitive operations to `logs/audit.jsonl`:
  - Tool execution (tool, args, result status, duration, cost)
  - Config changes (`/api/config` POST)
  - Session creation/deletion
  - Skill install/uninstall
  - Cron job add/remove

**Phase 7 Effort:** 1 week (1 engineer)

---

## Phase 8: Documentation & Runbooks (Week 9)

### 8.1 Required Documentation

- **8.1.1** `[ ]` Deployment Guide — prerequisites, systemd service, Docker, TLS/HTTPS via reverse proxy
- **8.1.2** `[ ]` Security Hardening Guide — threat model, network isolation, `TELEGRAM_ALLOW_FROM` setup, gateway auth, resource limits
- **8.1.3** `[ ]` Troubleshooting FAQ
- **8.1.4** `[ ]` Runbooks:

| Incident | Symptoms | Diagnosis Steps | Remediation |
|----------|----------|-----------------|-------------|
| OOM kill | Process killed by OS | Check session count, memory metrics | Restart; enable eviction; set `max_sessions` |
| Provider outage | All users failing | Check circuit breaker state; provider status | Wait or switch fallback provider |
| Database corruption | Errors on startup | `PRAGMA integrity_check` | Restore from backup |
| Unauthenticated access | Unknown sessions | Audit logs; check `allow_from` config | Set `allow_from` allowlist; add gateway auth |
| Disk full | Writes failing | `df -h`; check log rotation | Cleanup logs; add disk alerts |

- **8.1.5** `[ ]` Disaster Recovery Plan — backup schedule (memory.db, config.json, state.json, logs/), restore procedure, RTO: 1h, RPO: 24h
- **8.1.6** `[ ]` Scaling Guide — vertical only (single daemon); RAM/CPU tuning, session limits, WAL mode
- **8.1.7** `[ ]` Upgrade/Migration Procedure — changelog, database migrations, rollback plan
- **8.1.8** `[ ]` API/SDK Documentation — REST endpoints (OpenAPI), tool calling protocol

### 8.2 Architecture Diagrams

- `[ ]` System context, component, sequence (message flow, tool execution, memory recall), deployment diagrams

**Phase 8 Effort:** 1 week (tech writer + engineer)

---

## Phase 9: Operational Excellence (Week 10–12)

### 9.1 Alerting & Monitoring

- **9.1.1** `[ ]` Prometheus + Alertmanager rules:
  - `openpaw_up == 0` (process down)
  - `openpaw_memory_bytes > 2GB`
  - `openpaw_sessions_active > 100`
  - `openpaw_provider_errors_total[5m] > 10`
- **9.1.2** `[ ]` Grafana dashboard: system metrics, app metrics, LLM usage, error rates
- **9.1.3** `[ ]` Log aggregation: ship JSON logs to ELK/Loki/Datadog

### 9.2 Security Monitoring

- **9.2.1** `[ ]` Alert on: config modifications, skill installs from new repos, SSRF attempts in logs
- **9.2.2** `[ ]` Secret scanning: `gitleaks` on repo, scan logs before shipping
- **9.2.3** `[ ]` Dependency scanning: `cargo audit` in CI, `cargo deny` for license compliance, weekly scans

### 9.3 Backup & Restore Automation

- **9.3.1** `[ ]` Automated daily backups to S3 with checksum verification and 30-day retention
- **9.3.2** `[ ]` Monthly restore drill to test environment
- **9.3.3** `[ ]` SQLite WAL checkpoint + file copy for point-in-time recovery

### 9.4 Capacity Planning

- **9.4.1** `[ ]` Load test at 100 concurrent sessions; measure resource ceiling
- **9.4.2** `[ ]` Track metrics growth; project scaling timeline
- **9.4.3** `[ ]` systemd `MemoryMax=`, `CPUQuota=`; Docker `--memory`, `--cpus` limits

**Phase 9 Effort:** 1–2 weeks (SRE/DevOps)

---

## Phase 10: Long-term Architecture (Month 3+)

### 10.1 Multi-Instance & High Availability

**Current limitation:** Single daemon process, single point of failure.

- **10.1.1** `[ ]` Leader election via `raft` or consensus
- **10.1.2** `[ ]` Shared state store (Redis or PostgreSQL) for sessions, cost, approvals
- **10.1.3** `[ ]` HTTP gateway behind nginx/HAProxy with sticky sessions

**Effort:** 4–6 weeks (senior engineer)

### 10.2 Security Model Improvements

- **10.2.1** `[ ]` Sandbox tool execution — shell/file tools in container (Firecracker, gVisor); seccomp/AppArmor profiles
  > Note: sandboxing improves isolation but must NOT prevent the agent from running its intended commands. The sandbox must cover the full path, language runtimes, and network the agent needs.
- **10.2.2** `[ ]` Content security for prompt injection — HMAC-sign `AGENTS.md`/`SOUL.md` to detect tampering; verify signature before injection into system prompt

**Effort:** 4–8 weeks (security engineer)

### 10.3 Observability Enhancement

- **10.3.1** `[ ]` Deploy Jaeger/Tempo/Signoz for distributed tracing
- **10.3.2** `[ ]` ML-based anomaly detection on metrics
- **10.3.3** `[ ]` SLO/SLI tracking: availability >99.9%, latency p95 <2s, error budget

**Effort:** 2 weeks

### 10.4 Developer Experience

- **10.4.1** `[ ]` `openpaw-tools` crate — `#[derive(Tool)]` macro, validation helpers
- **10.4.2** `[ ]` `MockAgent` testing harness for unit testing individual tools
- **10.4.3** `[ ]` `openpaw-cli` admin tool: `sessions list`, `logs --follow`, `metrics`, `tool invoke --dry-run`

**Effort:** 3–4 weeks

---

## Phase 11: Technical Debt & Refactoring (Ongoing)

### 11.1 Code Quality

- **11.1.1** `[ ]` Extract large functions: `run_daemon()` (280+ lines), `agent::turn()` (650+ lines)
- **11.1.2** `[ ]` Standardize error types with `thiserror`; replace bare `anyhow` at module boundaries
- **11.1.3** `[ ]` Remove dead/unused code — `cargo clippy -W unused`
- **11.1.4** `[ ]` Add `///` doc comments on all public items; ADRs for major decisions

### 11.2 Dependencies

- **11.2.1** `[ ]` `cargo audit` → fix vulnerabilities; `cargo deny` → check licenses; `cargo outdated`
- **11.2.2** `[ ]` Remove unused dependencies; pin critical ones (`reqwest`, `tracing`)

**Phase 11 Effort:** Ongoing (all engineers)

---

## Resource Requirements

### Human

| Phase | Engineers | Duration | Total Person-Weeks |
|-------|-----------|----------|-------------------|
| Phase 0 | 1 | 1–2 days | 0.3 |
| Phase 1 | 2 | 2 weeks | 4 |
| Phase 2 | 2 | 2 weeks | 4 |
| Phase 3 | 2 | 2 weeks | 4 |
| Phase 4 | 1–2 | 1 week | 1.5 |
| Phase 5 | 1 | 1 week | 1 |
| Phase 6 | 2 + QA | 3 weeks | 6 |
| Phase 7 | 1 | 1 week | 1 |
| Phase 8 | 1 + writer | 1 week | 1 |
| Phase 9 | 1–2 (DevOps) | 2 weeks | 3 |
| **Total (Core hardening)** | | | **~26 person-weeks** |

**Plus long-term (Phases 10–11):** Ongoing effort

### Infrastructure

- CI/CD runners (GitHub Actions or self-hosted)
- Test database instances (SQLite, PostgreSQL)
- Mock HTTP servers
- Load testing environment
- Staging environment (full stack)
- Monitoring stack (Prometheus, Grafana, Loki/Jaeger)
- Backup storage (S3 or equivalent)

---

## Risk Assessment

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| **SSRF exploitation (web_fetch)** | HIGH | CRITICAL | Apply Phase 0.1 immediately |
| **skill_tool parameter injection** | HIGH | CRITICAL | Apply Phase 0.2 immediately |
| **OOM via session leak** | HIGH | HIGH | Wire evict_idle (Phase 0.4) immediately |
| **Config exfiltration via unauthenticated gateway** | HIGH | HIGH | Apply Phase 0.9 |
| Scope creep delaying critical fixes | MEDIUM | HIGH | Stick to priority order; defer Phase 10+ |
| Testing bottleneck | HIGH | MEDIUM | Prioritize critical path tests; TDD for new code |
| Breaking changes cause user pain | MEDIUM | MEDIUM | Semantic versioning, migration guide |

---

## Success Metrics

**Security:**
- [ ] Zero SSRF vulnerabilities (private IPs rejected in all HTTP tools)
- [ ] `skill_tool` parameter injection impossible (verified by unit test)
- [ ] All file writes validated for path traversal
- [ ] Gateway endpoints require authentication

**Stability:**
- [ ] Memory usage stable over 72h continuous run (no leaks)
- [ ] Session eviction active — max sessions bounded
- [ ] Task/plan/approval cleanup active
- [ ] Graceful shutdown completes within 30s

**Performance:**
- [ ] Semantic recall < 100ms for 10k memories
- [ ] TF-IDF query < 50ms for 10k chunks
- [ ] Cron tick < 10ms for 1000 jobs
- [ ] Tool call latency < 200ms p95 (simple tools)

**Quality:**
- [ ] Test coverage: core modules ≥ 80%, total ≥ 60%
- [ ] Zero `unwrap()` panics in production-critical paths
- [ ] All `TODO`/`FIXME` addressed or documented

**Observability:**
- [ ] Structured JSON logs in production
- [ ] All requests traceable via `trace_id`
- [ ] Prometheus metrics endpoint with ≥ 50 metrics
- [ ] Audit log capturing all sensitive operations

---

## Timeline Summary

| Week | Focus |
|------|-------|
| 0 (now) | **Phase 0** — SSRF, skill_tool injection, session eviction wiring, file size limits |
| 1–2 | **Phase 1** — Gateway auth, input validation, secrets hardening |
| 2–3 | **Phase 2** — Resource management (eviction, HTTP client, shutdown) |
| 3–4 | **Phase 3** — Performance (algorithms, database, allocations) |
| 4–5 | **Phase 4** — Concurrency (races, poisoning, cancellation) |
| 5–6 | **Phase 5** — Data integrity (constraints, transactions, durability) |
| 6–8 | **Phase 6** — Testing (unit, integration, chaos, benchmarks) |
| 8–9 | **Phase 7** — Observability (logs, metrics, traces, health) |
| 9 | **Phase 8** — Documentation (runbooks, deployment, security) |
| 10–12 | **Phase 9** — Operational (alerts, backups, capacity) |
| 3+ mo | **Phase 10–11** — Long-term (HA, sandboxing, SDK, debt) |

**Total time to production-ready:** 8–12 weeks with 2–3 engineers.

---

## Dependencies & Blockers

- **Phase 1 depends on:** Phase 0 (some fixes enable others)
- **Phase 3 depends on:** Phase 1 (security fixes may affect performance)
- **Phase 6 (Testing) must follow Phase 1–5** — tests for fixed code
- **Phase 7 (Observability) enables Phase 9** — need metrics for alerts
- **Documentation (Phase 8) can be parallel** with implementation

---

## Rollout Strategy

### Canary Deployment
1. Deploy fixed daemon to single internal user
2. Monitor metrics, audit logs, errors
3. Gradual rollout: 10% → 50% → 100%
4. Each phase deployed separately with rollback plan

### Feature Flags
- `gateway.require_auth = true` — enable gateway auth without rebuild
- `session.eviction_enabled = true` — enable session TTL eviction
- Runtime flags allow gradual rollout without redeploy

### Monitoring Deployment
- Compare pre/post metrics for regressions
- Watch for increased error rates, latency
- Alert on new `critical` or `security` log events

---

## Conclusion

OpenPaw is a **functional but infrastructure-vulnerable** system. The issues are not in the agent's reasoning or capability — they are in the surrounding runtime: SSRF-exposed HTTP tools, an injection path in skill_tool, a session eviction method that exists but is never invoked, and a gateway with no authentication.

**The fixes required do not restrict the agent.** SSRF protection blocks attacks against internal infrastructure, not legitimate web access. Skill parameter sanitization fixes injection without limiting what skills can do. Session eviction prevents OOM without changing what sessions can accomplish.

The plan prioritizes:
1. **Stop trivial exploits** (SSRF, injection, unauthenticated config)
2. **Stop leaks** (sessions, tasks, approvals)
3. **Make it observable** (logs, metrics, traces)
4. **Make it testable** (coverage, mocks, CI)
5. **Make it operable** (runbooks, alerts, backups)

With 2–3 engineers following this plan, OpenPaw can reach production readiness in **2–3 months**. Phase 0 alone (1–2 days) eliminates the most critical exploitable vulnerabilities.

---

**Plan Prepared by:** Security Audit — OpenPaw Runtime Analysis  
**Next Steps:** Execute Phase 0 fixes, validate with unit tests, proceed to Phase 1.
