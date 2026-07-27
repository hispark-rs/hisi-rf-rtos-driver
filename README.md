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
`RuntimeContract`. Contract v1.1 fixes the capability set, generation-bearing
resource handles, cancellable waits, and scheduling priority semantics:
`TaskPriority` accepts 0 through 31, with lower numeric values outranking higher
values. A backend with the wrong version or a missing capability is rejected
before partial radio initialization.

Contract v1.2 adds an advisory dynamic-task capacity snapshot. Radio profiles
can reject deterministic under-provisioning before consuming their storage, but
the snapshot is deliberately not called a reservation: another subsystem can
spawn between the check and radio startup.

Contract v1.3 adds race-free, owner-bound task admission. A profile atomically
reserves a non-zero number of dynamic slots, retains the opaque generation-
bearing `TaskReservation`, and uses `spawn_reserved` for the corresponding
tasks. Ordinary `spawn` cannot consume unfilled reservations; releasing a token
returns only its unconsumed slots, and stale or exhausted tokens fail closed.

Contract v1.4 extends the same owner-bound token to task-stack admission.
`reserve_task_resources` atomically promises dynamic slots and one fixed-size
stack allocation per slot. Partial allocation or slot-reservation failure rolls
back every stack before the token becomes visible; releasing the token returns
only unconsumed stacks and slots.

Scheduling guarantees are advertised separately through a versioned execution
profile. Port-less cooperative execution is distinct from timer/SWI-backed
cooperative, budgeted, and preemptive modes, so adapters can fail before
initialization when the target lacks the required switch-delivery mechanism.

`conformance` is a separate, no-heap `Scenario -> Action -> Observation`
harness for executing the same semantic checks against deterministic runtime
backends. Reports carry a schema version, backend revision, capability bits,
execution profile revision/modes and per-scenario status, and can be emitted as
allocation-free JSON for CI. The
suite covers priority/FIFO ordering, nested scheduler-lock deferral, sleep
deadlines, nested interrupt exit, task exit/slot reuse, semaphore direct
handoff and timeout cleanup, recursive mutex priority inheritance, stale task
and resource identities, busy destroy, and cancellation after a direct grant.
It also fixes zero-delay-as-yield, wait-forever, equal-deadline FIFO, and
highest-priority FIFO semaphore selection. Tick rounding/wrap and the RF
archive-bound task classification remain separate A5R closure gates.

Task-table capacity is distinct from allocator/control-block exhaustion:
`Runtime::spawn` returns `Error::NoTaskSlots` when no dynamic task slot remains,
while `Error::ResourceExhausted` continues to describe stack, semaphore, mutex,
or other bounded storage allocation failure.
