# Windows adapter progress (VP-013, partial)

Latest checkpoint (2026-11-05): freeze-05 passed the original independent repair review, confirming **VP013-I-003** fixed. Request ownership and actual receiver lifetime now replace `is_finished` stop/health inference. The unchanged four-case cleanup TLS oracle passes; independent ownership, healthy-followup, lost-loop, automatic owner-loss release and affected regressions pass. Current Mac full entry (55 Rust tests) and the full default Windows all-target check also pass. The coordinator accepted this repair checkpoint only; **full VP-013 remains unaccepted**. Freeze-04 stays rejected history. Earlier service-TLS original101 and its separately justified explicit manual-Retry adaptation0 remain disclosed; neither is silently relabelled. The sections below retain earlier implementation history; the current lifecycle is summarized at the end.

The smallest chosen shell is Rust with target-specific windows-sys Win32 bindings and the existing core API directly. No subprocess, alternate endpoint, shell, PATH helper lookup, runtime cargo, Swift fallback, or second JSONL parser is involved. This is compiled adapter progress, **not a working Windows desktop app** or VP-013 acceptance.

`crates/voiceport-windows` provides a portable controller with unique non-cloneable work tickets, identity invalidation before cancellation, cross-controller identity isolation, failure retention, recording-language-preserving explicit retry, shutdown stop confirmation and no stored transcript. Cancellation invalidates first then signals the shared cancellation token. New work is refused until the old ticket/service future is dropped. A stuck in-process syscall is not forcibly stopped; shutdown reports false, retains the current recording reference and blocks new work. No file deletion or broad cleanup occurs. A controller and in-flight job each hold the supplied finalized recording; the production platform must establish ACL/sharing/path ownership before constructing Recording. A public File/path pair is not a native storage security guarantee.

The service feature (default) explicitly calls the unchanged core TranscriptionClient only when Job.transcribe is invoked. Construction/help/smoke never load credentials or start a request. Shared errors stay sanitized. Core network deadlines remain15/90/110 seconds; there is no new end-to-end recording/worker deadline or background GUI executor. Cancellation and finite network waits cannot interrupt a synchronous file syscall. The result boundary always returns automatic=false: native UIA/password/privilege/window/clipboard-format restoration checks and input injection are entirely unavailable. There is no automatic paste, clipboard operation or submit key path.

A mono16-bit PCM WAV encoder accepts at most180 seconds at16kHz (5,760,044 bytes including header, below core20MB). It requires an empty destination, writes bounded chunks, rejects empty results and refuses to finalize after a partial I/O failure. It is **not device capture**. The synthetic_wav example writes only an explicit caller-owned create_new path for independent Python wave parsing.

The only native implementation so far is a thread-owned RegisterHotKey/UnregisterHotKey RAII primitive for Ctrl+Alt+Space with repeat suppression. Registration is explicit, not performed by default/help/smoke. Same-thread duplicate registration is refused locally; Win32 failure returns Conflict; failed unregister leaves the thread blocked conservatively. !Send/!Sync ownership prevents cross-thread release. This primitive is Rust-typechecked for x86_64-pc-windows-gnu, **not executed or linked into a verified Windows executable**. There is no tray, message pump, action dispatch or alternate-hotkey UX yet.

## Reproduce available checks

```sh
python3 -I scripts/test_windows.py --adapter-typecheck
```

This creates a new owned Cargo target and synthetic environment, runs workspace tests/fmt/clippy, native adapter-only typecheck, normal macOS packaging, existing core/app/loop/packaging checks and safe smoke. Full logs and exact command/environment/exit values are in a new vp013-developer run directory. The cross-check deliberately uses --no-default-features to exclude the shared network service and its native C dependencies; it does not prove the full Windows product compiled. No Windows runtime tests are substituted by this option.

Official rustup x86_64-pc-windows-gnu standard library installation succeeded. The actual default-feature Windows check failed in ring because x86_64-w64-mingw32-gcc is absent. No full Windows build/link/run is claimed. Needed next: a compatible C compiler/linker/SDK and actual Windows environment, then native recording (WASAPI/waveIn), versioned per-user ACL/reparse-safe storage and held leases, tray/message loop/actions/result/manual copy, hotkey conflict/runtime testing, UIA/privilege/clipboard safety, confirmed shutdown/file cleanup and genuine user-controlled device/target validation. Cleanup remains disabled until native ownership and >24h idle rules can be tested. No Windows shipped-dependency/license completeness claim follows from the native Mac resource regeneration.

## Continuation02 native shell (supersedes the earlier shell-unimplemented status)

Explicit `--run` now enters a real Win32 hidden-window/message pump and notification-area icon. The tray menu exposes disabled Start/Stop (capture and secure native storage unavailable), Cancel, current-result manual Copy, disabled Retry (background service dispatch unavailable), and Exit. Ctrl+Alt+Space registration is thread-owned; conflict changes the tray status rather than pretending success. A100ms timer polls confirmed controller stop; exit waits while the unique ticket remains alive. There is still no background service executor or microphone/storage factory in the native app, so no detached worker is created and no transcription is scheduled. The portable action adapter can admit a caller-owned finalized recording and consume its completion for tests/future platform integration.

WndProc only coalesces bounded action bits in thread-local Cell, catches Rust unwinds, and never stores a Rust object pointer in a window. Controller/result state remains on the loop stack; native calls cannot reenter a mutable Rust state borrow. Window/class/timer/tray/hotkey resources are owned by this thread and torn down in scope. Native manual Unicode clipboard allocation/transfer is only reachable through explicit result-copy action; QA never runs --run or calls the real clipboard. No SendInput/Return/automatic-paste path exists. Windows messages are not proof of a trusted human sender; this is ordinary manual menu dispatch, not a security authentication boundary. All these FFI paths still require actual Windows runtime testing, including destruction/error/panic paths.

The four first-round source graph changes were not just input hashes: bytes, mio, pin-project-lite, signal-hook-registry, socket2 and tokio gained dev+runtime scope through the new crate's dev dependency path. Versions/identities/materials/selections and all687 material bytes stayed unchanged. This round changes Windows-only binding features and reruns the unchanged Mac generator to update its Cargo manifest hash; per-round exact deltas are recorded, with no Windows license-distribution completeness claim.

Help and --smoke-test return before window/hotkey/tray/timer/device/clipboard/auth/network/cleanup. --run is never used in automated checks. Native capture, ACL/sharing/reparse-safe storage, actual background service work, result acquisition and Windows packaging remain incomplete; automatic delivery and broad cleanup remain disabled. Cross compiler/check/link results and actual target artifact identities are recorded separately in the continuation report; even a linked PE is not a Windows run or acceptance.

Compiler outcome for continuation02: official Zig0.17.1 ARM64 macOS archive passed advertised byte count/SHA256 and safe path/link validation, and its Windows GNU compiler compiled ring. Full default-feature `cargo check -p voiceport-windows --all-targets --target x86_64-pc-windows-gnu --locked` now passes, including native shell, shared service and tests (compile only). A real build/link was attempted: first missing dlltool, then Zig's real llvm-dlltool enabled import generation, but final link failed for missing libpthread.a/msvcrt runtime libraries. Rust's installed self-contained target directory contains startup objects only. There is **no successful PE build or Windows run**. Remaining compiler gate is compatible complete GNU runtime/import libraries/link configuration or a Windows MSVC build environment; no dummy libraries or disabled core service were used. Native execution and all missing platform behavior still require independent review and Windows evidence.

## VP013-I-001 repair (developer checks; independent acceptance pending)

The implemented-scope checkpoint was rejected: after Cancel while a unique ticket remained alive, ordinary Poll did not call Controller::poll_stopped unless closing. A later ignored completion or dropped ticket therefore left Stopping/retention and new admission Busy indefinitely. The repair polls stop confirmation on every ordinary Poll. While work is alive it retains the recording and waits; once released, it advances to Idle and allows new finalized admission without another Cancel. Waiting/recovered status is explicit. Ordinary idle/recovered/Ready polls return false, preserving the message pump and current Ready result; only closing plus confirmed stop returns true. No elapsed-time assumption, recording deletion, native ACL guarantee or automatic delivery is introduced. This portable public Actions repair is not proof of a Windows GUI manifestation or runtime fix; native recording/storage/executor/link/runtime/packaging and protected-delivery gates remain outstanding.

## Continuation03 executor (developer implementation, not full product acceptance)

Actions::with_executor configures an inert service; default Actions/help/smoke do not construct a runtime/client, resolve auth, start a thread or read files. The native --run shell owns a production CoreService executor and polls it through the message loop. A future secure recording/storage owner calls submit_finalized(Recording) on that message thread only after explicit user finalization. Start/Stop remain disabled; no file-upload CLI is added. The current Failed recording enables explicit native Retry; the same worker path rereads auth via the unchanged core, preserves recorded path/language and issues a fresh manual ticket. No automatic retries or delivery occur.

Execution has one process-wide permit, one OS thread and one bounded completion slot. Thread creation failure returns the same ticket as failure for retention/retry. Runtime creation and actual Job::transcribe run exclusively on that thread; the GUI neither reads auth/audio nor waits on a channel/runtime. Panic, missing completion/disconnect-like thread exit and oversized result become static failure. Only results up to1,000,000 UTF8 bytes enter the completion slot. Actual worker finish (including runtime destruction), not a timeout or a populated slot, permits the completed ticket to be consumed on the message thread. Conditional join occurs only after is_finished; thread creation/OS teardown syscalls are not hard-real-time guarantees.

Cancel/Exit invalidate before any completion draining or copying. Poll drains finished work then advances the accepted controller stop protocol; open idle/Ready polls remain false and preserve Ready/copy-error status. Closing waits for the actual ticket/thread, keeping the UI poll responsive. There is no forced interrupt of a synchronous file syscall. On abnormal Actions/executor drop, cancellation is signalled and the sole active handle/ticket moves through a nonblocking bounded handoff to a process-wide owner; no GUI join or detached unowned file reader. Later explicit submission reaps it only after thread finish. While it is held, the global permit blocks additional workers; an inactive stopped owner may retain its ticket until that reap or process termination. An unreachable handoff invariant failure deliberately retains one bounded owner/permit rather than freeing live audio. Abrupt process abort/termination remains outside graceful-stop guarantees.

The executor does not supply Windows ACL/sharing/reparse ownership, create/delete recording storage or make the existing caller-owned File/path admission secure. Native capture, secure storage/cleanup, protected delivery, complete PE link/runtime and Windows packaging/material review remain required. Fake services/spawn/runtime seams exist for deterministic tests, not runtime endpoint/mock/credential switches. Native service/clipboard/hotkey/message paths remain compiler-only until actual Windows execution.

## VP013-I-002 repair: OS thread/TLS stop boundary

The executor checkpoint was rejected after acknowledged real thread-local destructors held ordinary Poll, cancelled Poll and orphan-reaping submission for about one second. JoinHandle::is_finished is not proof that join cannot wait: Rust closure completion precedes OS/TLS teardown. The earlier conditional-join description is superseded.

There is now one process-owned cleanup thread, initialized lazily on first valid submission, and at most one service worker. A bounded request slot feeds the cleanup loop. That loop starts the service worker and performs the only worker join, off the UI thread, including TLS teardown. Only after join returns does it publish a release/acquire stopped acknowledgement. UI poll/orphan-reap never joins any thread and never treats result publication, cancellation, timeout or worker is_finished as stop proof. The retained Job/File and process-wide service permit remain owned while real teardown is held. Normal completion is consumed exactly once; cancelled/orphan results are discarded. One idle cleanup thread persists for process lifetime, rather than spawning a new joiner for each request.

Cleanup-thread initialization failure starts no worker and returns the ticket as a static failed request with retained recording; explicit later work may retry initialization. Worker spawn/runtime/missing-result/panic paths remain static failures. The cleanup channel is process-owned and bounded; normal code does not close its receiver or replace its cleanup thread. A returned cleanup loop with an unclaimed request proves no service was launched, allowing static failure without joining; it is never used to respawn accumulating cleanup threads. Catastrophic infrastructure loss after a claimed request without a stop acknowledgement stays conservatively owned/blocked, not falsely confirmed. Trusted Spawner implementations must cover the supplied work lifetime and start/retain no work on Err. Native kernel/process abort and uninterruptible syscalls remain outside graceful-stop guarantees.

During held TLS, global contention may return a retained Failed request promptly. After actual stop, recovery then requires explicit manual Retry, not automatic replay. The original TLS oracle's orphan tail expected immediate success; both the unmodified outcome and a recovery-only explicit-Retry adaptation are retained in repair evidence without weakening its three acknowledged responsiveness assertions. Poll open/closing/result/priority behavior from VP013-I-001 remains unchanged. Synthetic descriptor observations prove only local File retention, not Windows ACL/sharing/lease guarantees. No device/storage/deletion/automatic delivery, native Windows runtime, link or packaging acceptance is added.

## VP013-I-003 repair: explicit request ownership (independently accepted checkpoint)

A lawful cleanup loop can run inside the returned thread's TLS destructor after its main closure completes. The previous unclaimed-request fallback falsely treated that state as stopped and could panic when unwrapping a Job still held by the queue. Freeze-04 remains rejected for that defect.

The accepted freeze-05 executor gives UI Running and queued/active RequestOwner explicit references to the Job and shared process permit. Only actual unclaimed request disposal or a real off-UI worker join allows RequestOwner to release its references, before publishing stopped. UI uses Acquire before consuming its remaining unique Job. Cleanup availability follows the whole actual receiver guard lifetime, allowing a loop delayed into TLS to serve healthy later requests. Dropping the UI cancels and releases only its own references; cleanup releases after actual stop without later UI reaping. There is no orphan queue or closure-finished inference. Unknown join loss conservatively retains a bounded Job/permit; no forced thread stop is used.

The original independent verifier reran the unchanged four-case cleanup TLS fixture, new current/cancel/exit/drop ownership and healthy follow-up controls, lost-loop disposal, automatic orphan release, prior service-TLS/original11 and current regressions. The coordinator accepted that implemented repair scope. Five-compaction coordinator rotation was checked read-only; no extra rotation stress or forced compaction was performed. All mandatory Windows native recording, private storage/leases/cleanup, protected delivery, successful full linking/runtime and distribution/material gates remain open.

## Storage01: private native ownership foundation (developer, not accepted security)

The new `storage::Storage` value is inert: construction, default Actions, help and
safe smoke do no storage IO. Only explicit `open_or_create` resolves Windows
LocalAppData, and explicit `Root::create` creates a recording. The tray still does
not call these APIs: capture and Start/Stop remain disabled. No scanning, adoption
of unknown files, deletion, ACL repair, cleanup thread, upload CLI or automatic
paste is added. On non-Windows `open_or_create` returns Unsupported without IO.

The candidate native boundary supports only bounded drive-qualified local paths
on a fixed NTFS volume. UNC/device/extended namespaces, streams, dot/empty
components, DOS device aliases and trailing-dot/space aliases are rejected. Parent
paths are at most180 UTF8 bytes/24 components, leaving room below legacy Win32
path limits for the fixed version directory and random filename. Unicode names
are supported within that deliberately conservative bound. Every existing
ancestor is opened with OPEN_REPARSE_POINT, checked by handle as a non-reparse
directory and retained with read-only sharing (no write/delete sharing). Active
ancestor allow ACEs granting data/attribute/EA mutation, delete-child/delete,
owner/DACL mutation or generic write/all to an untrusted SID are refused; complex
unknown ACEs fail closed. Owners must be the process user, SYSTEM, Administrators
or the fixed Windows TrustedInstaller service SID. Inherit-only ACEs do not grant
rights on the inspected ancestor; basic deny ACEs only reduce access. Creating
subdirectories elsewhere in an ancestor alone is not treated as permission to
replace a pinned existing component. This conservative policy can reject valid
complex profiles; there is no insecure fallback or permission repair.

`VoicePort-recordings-v1` is created with an explicit protected DACL containing
exactly one current-process-user SID full-access ACE, with the same SID owner.
Existing roots must already match; a preexisting inherited/broad/unknown ACL,
reparse point, wrong type or missing marker is refused. The small exact version
marker includes the current SID, has the same private DACL, and is opened/retained
rather than inferred from its filename. Marker hardlinks and wrong bytes/lengths
are rejected. A failed initial marker write may leave a non-adoptable private
root; the API never silently repairs or removes it. One Root is reused for
creation: reopening a newly created root concurrently may conservatively fail
because its marker writer is still held.

Each file name is128 random bits from the system RNG plus `.wav`, with at most
8 CREATE_NEW collision attempts and no caller-chosen leaf. File creation passes
the same explicit descriptor and uses read/write access with FILE_SHARE_READ
only; handles are non-inheritable. Type, reparse status, link count1, owner and
DACL are checked through the actual open handle. The bounded existing mono16kHz
PCM writer produces at most5,760,044 bytes/180 seconds; any append failure
permanently forbids finalization. Finalize rejects empty/partial output, flushes
and syncs the same file, rechecks its volume/file ID/security and rewinds it.
No replacement file is opened during this conversion. Partial/failed files are
left private and non-admitted; dropping them closes owned handles, never deletes.

Recording now has private path/File/language fields and retains an optional
Arc lifetime guard; this prevents replacing a native finalized pair while keeping
an unrelated guard. The caller_owned constructors remain explicit **caller
ownership contracts**, not security validators. The native finalizer moves its
root/ancestor/marker guards into Recording. Controller and actual executor Job
references therefore keep those guards through failure retention, language-fixed
manual Retry, cancellation, worker/TLS teardown and abnormal Actions drop. No
accepted executor/actions stop logic changed. The held recording handle denies
competing write/delete opens; Rust/core's read-only path reopen uses compatible
sharing. This last compatibility, rename denial, exact DACL creation and all
native integration assertions still require real Windows execution.

The rationale is pinned object identities plus restricted ancestor/private ACLs,
not a universal absolute-path TOCTOU proof. Windows sharing does **not** constrain
all attribute/security requests. Same-user malicious code (including duplicate
handles/impersonation), administrators/SYSTEM/TrustedInstaller, previously granted
hostile security/attribute handles, privileged backup/restore access, drive-letter
remapping, filter drivers and kernel compromise are outside this foundation's
threat model. No native security acceptance or general Windows lease guarantee
follows from cross-compilation. See Microsoft's
[CreateFile sharing and reparse rules](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilea)
and [handle-based security query](https://learn.microsoft.com/en-us/windows/win32/api/aclapi/nf-aclapi-getsecurityinfo).

Portable tests exercise bounded path rejection, inert unsupported construction,
static errors and guard lifetime through failure/retry/success/cancel and real
worker thread-local teardown/owner drop using synthetic Files. Existing controller,
actions, service/cleanup TLS matrices retain their assertions after the narrow
constructor adaptation. Windows-only integration tests assert real private-root
creation/adoption refusal, marker tampering/hardlinks, atomic collision preservation,
WAV admission/poison rules, read compatibility, competing-write/rename refusal
and eventual release. They are explicitly ignored and **not executed here**;
full default Windows target checks typecheck them and the native APIs only.

This slice still has no actual Windows runtime or linked PE, device capture,
>24h residual cleanup, protected UIA/clipboard delivery, user recording/target
acceptance, or Windows distribution/material review. Those remain mandatory
full VP-013 gates. The unchanged native Mac license generator only updates its
manifest input fingerprint for the Windows binding-feature declaration; recorded
package/material/selection/scope comparisons determine the actual delta, not a
claim about Windows shipped dependencies. Full product_accepted remains false.

## Independent freeze06 checkpoint

The original independent reviewer passed the complete source/compile/offline
ownership checkpoint, and the coordinator checked the sealed reports, complete
1851-record evidence index and all frozen/history identities before accepting
that bounded stage. The current Mac entry passes25 steps including60 Rust tests;
the fresh default Windows all-target check passes with16 real ring COFF objects.
New policy, WAV, guard and actual service-thread TLS controls pass. Historical
fixtures retain their exact originals and necessary constructor patches; one
ownership helper additionally needs a read-only path accessor migration. Earlier
service-TLS original semantics still fail101; the explicit manual-Retry recovery
adaptation passes separately. Test-fixture and collector failures remain recorded.

This decision does not accept native NTFS ACL/sharing/reparse/security or device
runtime behavior. Three native integration tests were compiled and not executed.
Capture, Start/Stop, deletion and automatic delivery remain disabled. Native
runtime, retention cleanup, protected delivery, successful Windows link/PE and
packaging/material gates remain mandatory. See [validation](VALIDATION.md) for
execution, evidence and historical receipt-restoration boundaries.

## Capture01: explicit off-UI acquisition foundation (not native runtime acceptance)

The new `capture` module is unwired library code. NativeCapture/default values,
Actions/help/safe smoke do not create storage, enumerate/open devices, request
permission, record, read auth or start a service. Native Start/Stop remain disabled.
On non-Windows explicit NativeCapture.prepare returns static Unsupported with zero
IO. On Windows it accepts only an already held native storage::Root, reserves the
single process capture budget and creates its private Pending before device work.
The returned NativeSession.begin explicitly opens/prepares/queues/starts the device.
There is no public native arbitrary-File/path entry. Existing Root/Pending,
Recording/Controller/Job, executor/actions/shell and WAV behavior are unchanged.

Backend and Output are trusted embedding interfaces used by the production Session
pipeline and synthetic QA. Caller-owned synthetic outputs promise object lifetime,
not native ACL safety. NativeSession wraps its backend privately and always uses
one process budget; isolated Budget values are only for independently owned
synthetic contexts. Identities are checked, globally monotonic UInt64 session IDs
with checked exhaustion, selected device ID, exact format and consecutive packet
sequence. Malformed, zero/odd/oversized, foreign, duplicate, non-DONE and late frames
cannot produce output success. At most one result can be taken; cancel invalidates
before reset/draining, stop cannot undo cancellation and closed sessions cannot
begin or consume new frames. Language and the same native Pending flow to finalize.

Windows uses legacy WinMM waveIn with CALLBACK_EVENT and zero-time event polling,
not a Rust callback/resampler/framework. Exact PCM query plus WAVE_FORMAT_DIRECT
forbids ACM conversion; the actual opened device handle's ID is bound. The interface
is mono signed16-bit PCM at16kHz: at most2,880,000 samples/180 seconds and5,760,044
WAV bytes. Swift's existing mono AAC24kHz/64kbps format remains unchanged. No sample
conversion is implemented. waveIn is a conservative small dependency already in
the locked bindings, not a recommendation over modern WASAPI for future work.

Two stable, initially queued, fixed buffers contain the entire180-second sample
budget plus a4096-sample overflow sentinel (5,768,192 data bytes total). No refill
or reuse occurs, avoiding a requeue gap that WinMM could silently discard without
an overflow notification. This backend is bounded batch acquisition, not live
streaming: normally partial data is returned only after explicit Stop/reset.
If the sentinel completes, the run fails; excess data during final drain fails
instead of truncating into success. When running accounting reaches the maximum,
Session requests stop/reset; empty, excess, partial IO and malformed runs never
finalize. There is no autonomous timer/worker: the off-UI owner must poll/stop;
blocked/absent polling can leave the device active, and later output is rejected
if the sample budget was exceeded. No hard wall-clock microphone-stop guarantee
or interruptible driver/file syscall is claimed.

Header and sample allocations never move while WinMM may hold their pointers.
Headers have an explicit aligned UnsafeCell wrapper (the packed bindings are
read by value); driver completion flags are polled volatile at verified aligned
offsets. Sample memory is UnsafeCell and copied only after WHDR_DONE with validated
pointer, user identity, buffer length, byte count, alignment, prepared/not-inqueue
flags and bounded offset. Each copied packet has at most4096 samples. No Rust
callback unwinds across FFI, no unsafe Send/Sync is asserted, and the native owner
is thread-bound. The binding declares waveInGetID's output pointer const; the code
passes a raw pointer derived from unique mutable self rather than an immutable
Rust reference. Native driver/OS memory-order and lifetime promises need actual
Windows review/tests; compiler success is not runtime safety evidence.

Stop/error/cancel uses stop, successful reset, bounded drain, confirmed unprepare
and close. A reset acknowledgement alone does not free buffers. Up to four drain
packets per call and a fixed maximum packet count bound progress without growing
queues. Errors permanently forbid this recording's success, even when later cleanup
recovers. Wav range rejection alone leaves its earlier prefix finishable; Pending
already latches every append error permanently. Capture independently latches
its accounting/output error, so neither a valid prefix nor later cleanup can
turn an over-limit or partial-write capture into success. Output finalization occurs only after close and the backend destructor
body has actually returned. The same Pending's finalization then supplies its
immutable Recording and accepted ancestor/marker/File guards. There is no deletion,
transcription, retry, history or alternate storage policy in capture.

This is a synchronous **off-UI owner API**. Native calls, output writes and Drop
may block; future GUI integration needs a dedicated owner context and separate
review. The message pump never calls this code now. Unknown reset/drain/unprepare/
close keeps the complete owner, output guards and process capacity. Explicit cleanup
polling may retry release steps, never capture or service work. Ordinary off-UI
Drop drains all bounded already-returned packets, while each poll retains its
four-packet limit. Drop continues only after a full returned-packet quota, never
on Idle or an unknown reset/unprepare/close. If release remains unconfirmed (or
while already unwinding), the owner/permit is retained until process termination. It does not start
reapers/replacements or infer actual stop from timeout, result publication, Weak
expiry or JoinHandle::is_finished. This conservative lost-owner case requires process
restart, not silent recovery; no universal panic/abnormal kernel-stop guarantee.

Portable production-pipeline tests use newly owned synthetic Files and controllable
backends: exact PCM/language/identity, empty/maximum/excess, partial actual file writes,
each open/prepare/start/read/write/stop/reset/unprepare/close/finalize error, malformed
and stale frames, cancel/late data, capacity/replacement, reset acknowledged before
buffer return, destructor-body gates and unknown Drop ownership. Native API/device+
private-NTFS integration is an explicit ignored test; together with the three existing
storage tests it is compiled only here, never run. Successful portable controls do
not prove Windows driver, device, permission, ACL, sharing or NTFS semantics.

Microsoft documents [waveInOpen format/event flags](https://learn.microsoft.com/en-us/windows/win32/api/mmeapi/nf-mmeapi-waveinopen),
[queued input and possible data loss](https://learn.microsoft.com/en-us/windows/win32/multimedia/managing-waveform-audio-recording),
[reset returning pending buffers](https://learn.microsoft.com/en-us/windows/win32/api/mmeapi/nf-mmeapi-waveinreset),
and [unprepare before freeing buffers](https://learn.microsoft.com/en-us/previous-versions/dd743853%28v%3Dvs.85%29).
Actual Windows device/NTFS gates, full PE/link/runtime, future user Start/Stop,
protected delivery, residual cleanup and Windows distribution/material review remain
mandatory. Source/compile/offline developer checks do not accept full VP-013.


## Capture repair02: confirmed unwind retention and ordinary Drop drainage

Freeze08 was rejected for VP013-C08-001 and VP013-C08-002: a destructor panic
inside Session::drop bypassed its final leak statement and freed the normal owner
field, and ordinary returned data beyond four packets leaked admission unnecessarily.
The exact independent13 fixture/assertions remain unchanged: baseline10pass/3fail101
and repaired13pass0 are separately retained, alongside the two adjacent controls.
These developer checks are not independent acceptance or Windows runtime evidence.

Session now keeps its allocation in a field-level OwnerSlot. Its own destructor
retains the entire still-unconfirmed Box even if Session::drop unwinds. A latch
surrounds calls into backends/outputs, including their complete destruction and
Output::finish; only NORMAL RETURN clears it. If an embedding caller catches a
panic, later begin/stop/cancel/poll cannot reinterpret it as cleanup or retry that
interrupted work. They retain admission/remaining output guards and return only a
static failure. In particular backend destructor panic keeps remaining
output/root guards, and output destruction/finish panic keeps capacity. Rust may
unwind fields of the value already being destroyed; this policy retains the whole
remaining owner and cannot resurrect a consumed panicking value. No valid audio is
published on these paths. Propagating panic, abort/double-panic and native FFI/kernel
failure are not claimed as graceful GUI recovery by an external catch harness.

A normal off-UI Drop now permits a finite larger drain budget: only a full quota
of four returned packets enables another pass, bounded by MAX_DRAIN_PACKETS and
one End allowance. Idle, release errors and an interrupted operation terminate
attempts immediately with conservative ownership. Ordinary five-packet, exact
maximum, overflow-sentinel and finite drain-limit disposal can reach confirmed
unprepare/close/destructor completion and allow a later admission. This is bounded
work, not a timeout or proof inferred from silence. Normal owners are actually
freed, avoiding per-session retention in the successful lifecycle. No reaper,
thread, new queue, unsafe Send/Sync, GUI wiring, device operation, automatic retry,
file deletion, format change or native storage alteration was added. All original
native capture/NTFS/security/link/runtime and future product gates remain open.

## Capture repair03: explicit recovery after returned release errors

A normally returned reset, unprepare or close error now prevents Drop from
retrying that unknown cleanup stage, even if the external fault disappears.
The remaining owner, output guards and permit stay retained. A new explicit
stop, cancel or poll may retry; successful completion of the failed stage allows
cleanup to advance, while another release-stage error again requires explicit
recovery. This condition is separate from the first-error latch: earlier
open/read/output errors stay visible and never admit a partial recording, and
a recovered release error does not prohibit bounded ordinary Drop drainage.
Interrupted calls retain the permanent unconfirmed latch and cannot recover.
Native release and complete backend/output destructor return, including fields,
remain required before capacity returns. These are portable policy self-checks;
independent review and all native runtime/product gates remain outstanding.
