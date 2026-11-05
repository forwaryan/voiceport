# VoicePort host protocol v1

`voiceport-rs host [--auth PATH]` is a child process using stdin/stdout pipes, never a shell, daemon or listener. Startup emits `{"version":1,"event":"ready","state":"idle"}` without reading auth/audio or networking. Only transcribe/retry invoke the existing fixed-HTTPS client. Paths, tokens and account IDs are never echoed. Swift integration is a subsequent story.

Every command is one UTF-8 JSON object followed by LF. CRLF is accepted (CR counts toward the limit). Blank lines, invalid UTF-8/JSON, unknown/duplicate fields, unsupported actions/versions and malformed IDs return one static `invalid_frame` error without echoing input or performing an action. An input frame excluding LF is at most 16,384 bytes; oversize frames are drained to LF using bounded memory. An unterminated EOF fragment is invalid once; EOF then invalidates/cancels work and exits, without a terminal result. Frames must not contain raw LF inside strings; JSON escaped LF is fine. `language` may be absent/null; other required fields cannot be null. Audio paths are nonempty UTF-8 strings without NUL; filesystem errors are sanitized.

Each command requires `version:1`, `action`, `request_id`. IDs contain 1–64 ASCII letters/digits or `_ - . :`. Extra fields are forbidden, including raw credentials. Commands:

| action | Additional fields | Response |
| --- | --- | --- |
| status | none | status, current state/session (no text/path) |
| transcribe | audio_path, optional language | accepted, then result/error |
| retry | session_id | accepted with a new session, or not_retryable |
| cancel | session_id | cancelled, or stale_session |
| shutdown | none | shutdown acknowledgement, exit even with stdin open |

Examples (each shown object ends with LF):

```json
{"version":1,"action":"transcribe","request_id":"t1","audio_path":"/private/session/dictation.m4a","language":"zh"}
{"version":1,"action":"cancel","request_id":"c1","session_id":"s1"}
{"version":1,"action":"status","request_id":"q1"}
{"version":1,"action":"shutdown","request_id":"end"}
```

Every event has version, event, state. Correlated events include request_id; work events also include internal session_id (`s1`, `s2`, ...), generated monotonically without reuse within this process. Process restarts reset identity: never correlate sessions across child processes. Caller IDs do not determine generation. accepted/result/service-error include is_retry. Only result includes original transcript in text; JSON escapes control characters and prevents protocol-line injection. A service error carries a sanitized core enum code such as `Network`, `Cancelled`, `Service(401)`; no raw upstream error/body. Protocol errors carry invalid_frame, duplicate_request, stale_session, not_retryable, request_limit, output_too_large. Error frames have no text.

A replacement first invalidates the old identity and cancels its token, emits replaced correlated with the old transcribe request/session, then accepted for the new request/session. Old successes/errors/service cancellations are rejected irrespective of cancellation cooperation. Explicit cancel invalidates first, clears retry context and emits cancelled using the cancel command request_id and target session_id. Service-originated Cancelled instead enters failed and is manually retryable. Only the current failed session can retry; it reuses path/language, rereads auth/audio through the service, generates a new identity and sets is_retry=true (native hosts must disable automatic delivery). Success enters ready with no stored transcript or retry path. Failure retains only the current retry context. Replacing/cancelling/shutdown clears it. No file is deleted by host/session code under any outcome.

Syntactically valid IDs are retained in a bounded 1,024-entry process-lifetime set, including rejected stale commands. Duplicate IDs are rejected and cannot replay work. No eviction or ID reuse: the 1,025th distinct valid request returns request_limit and terminates cleanly. Malformed frames do not reserve IDs. With simultaneous readiness, control/EOF precedes completion, so already queued cancellation/replacement wins. Hosts should wait for each accepted/ack before depending on its session ID.

Input queue capacity is 1 (plus the reader's one pending frame); output queue capacity is 1. One async service task exists; replacement/cancel aborts its previous task after identity invalidation. There is no per-line task spawning. A single dedicated writer serializes outputs and acknowledges flush. Output JSON serialization itself is capped at 2,000,000 bytes excluding LF. An oversized result is replaced by static output_too_large and the process exits with failure. Every frame enqueue/write/flush acknowledgement has a single total 5-second deadline (not merely an inactivity timer). Slow/non-reading stdout, EPIPE or serialization failures cancel active work and terminate. Std I/O uses detached OS threads, not Tokio's blocking stdin pool; an open stdin or blocked writer does not delay process/runtime exit. No unbounded joins or queues are used. A test-only example injects a 150ms deadline; production has no mock or timeout/endpoint override switch.

Fatal output interruption can leave an unterminated final JSON fragment even though serialization was capped before writing. A native parent must accept only complete LF-terminated frames, discard an incomplete tail at EOF, and correlate events with both the current child identity and session identity. The five-second deadline applies to each frame; successive small frames that finish in time can keep the process alive while input continues.

Exit codes: 0 shutdown/EOF/request-limit exhaustion; 1 I/O/write timeout/output limit or unexpected worker failure; 2 CLI grammar error; 130 Ctrl-C. Malformed frames and stale/duplicate commands are recoverable. Read I/O failure is treated as EOF. Ctrl-C stops the process (a blocked write may first consume the bounded write deadline). Synchronous local file syscalls inside the accepted VP-009 client are not interruptible mid-call; no claim is made for arbitrary filesystem hangs. Normal pipeline termination cannot undo an upload already received remotely. Windows compile/runtime, real cloud/TLS, and native Process integration remain unverified.
