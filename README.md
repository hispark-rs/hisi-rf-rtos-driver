# hisi-rf-rtos-driver

`no_std`, chip-neutral task and synchronization contract between HiSilicon
radio adapters and a selected runtime. It contains no scheduler, radio policy,
chip registers, allocator, or network stack.

Exactly one runtime is installed per firmware. Radio callbacks may wake a task
through the bounded semaphore operation, but user callbacks never execute in an
ISR, critical section, or scheduler lock.

The contract exposes small task, scheduler-lock, semaphore, and recursive-mutex
capabilities. Priority inheritance belongs to the selected runtime; chip ABI
adapters translate only the vendor symbols used by their pinned blob and do not
turn this crate into a LiteOS compatibility surface.

Before a radio adapter allocates resources it can require a versioned
`RuntimeContract`. Contract v1 fixes both the capability set and scheduling
priority semantics: `TaskPriority` accepts 0 through 31, with lower numeric
values outranking higher values. A backend with the wrong major version or a
missing capability is rejected before partial radio initialization.

`conformance` is a separate, no-heap `Scenario -> Action -> Observation`
harness for executing the same semantic checks against deterministic runtime
backends. Reports carry a schema version, backend revision, capability bits and
per-scenario status, and can be emitted as allocation-free JSON for CI. The
initial suite covers priority/FIFO ordering, nested scheduler-lock deferral,
sleep deadlines, nested interrupt exit, and task exit/slot reuse. Semaphore,
mutex priority-inheritance, wait timeout/cleanup and stale-handle scenarios
remain required before the v1 conformance suite is considered complete.

Task-table capacity is distinct from allocator/control-block exhaustion:
`Runtime::spawn` returns `Error::NoTaskSlots` when no dynamic task slot remains,
while `Error::ResourceExhausted` continues to describe stack, semaphore, mutex,
or other bounded storage allocation failure.
