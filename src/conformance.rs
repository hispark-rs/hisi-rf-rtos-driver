//! Runtime-neutral executable semantics for radio backends.
//!
//! The production [`crate::Runtime`] trait intentionally stays small. This
//! module provides a separate deterministic harness for proving the behavior
//! behind that trait without adding test controls to the production ABI.

use core::fmt;
use core::num::NonZeroU32;

use crate::{Error, RuntimeContract, RuntimeExecutionProfile, TaskPriority};

/// Version of the conformance scenario and report schema.
pub const SCHEMA_VERSION: u16 = 6;

/// Logical task identity used only inside deterministic scenarios.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct ActorId(u8);

impl ActorId {
    /// Task adopted when a scenario starts.
    pub const MAIN: Self = Self(0);
    /// First dynamic task.
    pub const WORKER_A: Self = Self(1);
    /// Second dynamic task.
    pub const WORKER_B: Self = Self(2);

    /// Returns the stable report representation.
    pub const fn into_raw(self) -> u8 {
        self.0
    }
}

/// Scheduling mode required by one scenario.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionProfile {
    /// Switching occurs only at explicit scheduling points.
    Cooperative,
    /// Higher-priority ready tasks may preempt outside scheduler locks.
    Preemptive,
}

/// Observable task state used by the shared scenarios.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActorState {
    /// Eligible but not currently executing.
    Ready,
    /// Currently executing.
    Running,
    /// Waiting for a resource or deadline.
    Blocked,
    /// Waiting for a monotonic sleep deadline.
    Sleeping,
    /// Task has exited and its identity is no longer live.
    Exited,
}

/// Result associated with one scenario action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActionOutcome {
    /// No externally visible scheduling transition occurred.
    Completed,
    /// A task was created.
    Spawned,
    /// A switch occurred before the action returned.
    ContextSwitched,
    /// A ready task could not preempt while the scheduler lock was held.
    PreemptionDeferred,
    /// A wait acquired its resource.
    Acquired,
    /// A wait deadline elapsed.
    TimedOut,
    /// A resource grant was handed directly to a waiter.
    Granted,
    /// A task identity was retained for a later liveness check.
    IdentityRemembered,
    /// A retained identity was correctly rejected after slot reuse.
    StaleIdentityRejected,
    /// A synchronization resource was created.
    ResourceCreated,
    /// A synchronization resource handle was retained for a later operation.
    ResourceHandleRemembered,
    /// A synchronization resource was destroyed.
    ResourceDestroyed,
    /// A queued wait or unconsumed direct handoff was cancelled.
    WaitCancelled,
    /// The selected live task had no cancellable wait or pending grant.
    NoPendingWait,
    /// Effective priority observed for one actor.
    PriorityObserved(TaskPriority),
    /// The backend rejected an operation before changing observable state.
    Rejected(Error),
}

/// Synchronization resource selected by a lifecycle scenario.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceKind {
    /// Counting semaphore.
    Semaphore,
    /// Recursive priority-inheritance mutex.
    Mutex,
}

/// Which generation-bearing resource handle an action uses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceHandleRef {
    /// The most recently created live handle.
    Current,
    /// A handle retained by [`Action::RememberResourceHandle`].
    Remembered,
}

/// Timeout mode used by deterministic wait scenarios.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wait {
    /// Return immediately when the resource is unavailable.
    NoWait,
    /// Wait until a finite monotonic deadline.
    Milliseconds(NonZeroU32),
    /// Wait without a deadline.
    Forever,
}

/// One operation in a deterministic runtime scenario.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    /// Reset the harness with `MAIN` running at `main_priority`.
    Reset {
        /// Scheduling profile under test.
        profile: ExecutionProfile,
        /// Initial task priority.
        main_priority: TaskPriority,
    },
    /// Create a ready task.
    Spawn {
        /// Logical task identity.
        actor: ActorId,
        /// Requested priority.
        priority: TaskPriority,
    },
    /// Yield from the current task.
    Yield,
    /// Apply the vendor delay convention: zero yields, non-zero sleeps.
    Delay {
        /// Delay in milliseconds. Zero is an explicit scheduling point.
        milliseconds: u32,
    },
    /// Enter one scheduler-lock nesting level.
    LockScheduler,
    /// Leave one scheduler-lock nesting level.
    UnlockScheduler,
    /// Enter one interrupt nesting level.
    EnterInterrupt,
    /// Leave one interrupt nesting level.
    ExitInterrupt,
    /// Advance the deterministic monotonic clock.
    AdvanceTime {
        /// Milliseconds to advance.
        milliseconds: u32,
    },
    /// Block the current task until a monotonic deadline.
    Sleep {
        /// Non-zero sleep duration in milliseconds.
        milliseconds: u32,
    },
    /// Observe one actor without changing scheduler state.
    Observe {
        /// Actor whose state is returned as the observation subject.
        actor: ActorId,
    },
    /// Exit the current task.
    ExitTask,
    /// Wait on the scenario counting semaphore.
    SemaphoreWait {
        /// Wait mode.
        timeout: Wait,
    },
    /// Release the scenario counting semaphore.
    SemaphorePost,
    /// Observe whether an actor owns a direct resource grant.
    ObserveGrant {
        /// Actor whose pending grant is inspected.
        actor: ActorId,
    },
    /// Acquire the scenario recursive mutex.
    MutexLock {
        /// Wait mode.
        timeout: Wait,
    },
    /// Release one recursion level of the scenario mutex.
    MutexUnlock,
    /// Observe an actor's effective scheduling priority.
    ObservePriority {
        /// Actor whose effective priority is inspected.
        actor: ActorId,
    },
    /// Retain an actor's current generation-bearing identity.
    RememberIdentity {
        /// Actor whose identity is retained.
        actor: ActorId,
    },
    /// Validate that the retained identity no longer names a live task.
    ValidateRememberedIdentity,
    /// Create one synchronization resource for lifecycle checks.
    CreateResource {
        /// Resource type to create.
        kind: ResourceKind,
    },
    /// Retain the current resource handle and its identity generation.
    RememberResourceHandle {
        /// Resource type whose handle is retained.
        kind: ResourceKind,
    },
    /// Destroy the selected synchronization resource handle.
    DestroyResource {
        /// Resource type to destroy.
        kind: ResourceKind,
        /// Current or previously retained handle.
        handle: ResourceHandleRef,
    },
    /// Cancel one actor's queued wait or unconsumed direct handoff.
    CancelWait {
        /// Actor whose pending synchronization operation is cancelled.
        actor: ActorId,
    },
}

/// State returned after applying one action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Observation {
    /// Task running when the action finishes.
    pub running: ActorId,
    /// Optional task whose state is relevant to this step.
    pub subject: Option<(ActorId, ActorState)>,
    /// Action result.
    pub outcome: ActionOutcome,
    /// Current task's scheduler-lock nesting depth.
    pub scheduler_lock_depth: u16,
    /// Current interrupt nesting depth.
    pub interrupt_depth: u16,
}

/// A partial observation used as a scenario assertion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExpectedObservation {
    /// Required running task, or any task when absent.
    pub running: Option<ActorId>,
    /// Required subject state, or no subject assertion when absent.
    pub subject: Option<(ActorId, ActorState)>,
    /// Required action outcome, or any outcome when absent.
    pub outcome: Option<ActionOutcome>,
    /// Required scheduler-lock depth, or any depth when absent.
    pub scheduler_lock_depth: Option<u16>,
    /// Required interrupt depth, or any depth when absent.
    pub interrupt_depth: Option<u16>,
}

impl ExpectedObservation {
    /// Returns whether an observation satisfies every populated field.
    pub fn matches(self, observed: Observation) -> bool {
        (self.running.is_none() || self.running == Some(observed.running))
            && (self.subject.is_none() || self.subject == observed.subject)
            && (self.outcome.is_none() || self.outcome == Some(observed.outcome))
            && (self.scheduler_lock_depth.is_none()
                || self.scheduler_lock_depth == Some(observed.scheduler_lock_depth))
            && (self.interrupt_depth.is_none()
                || self.interrupt_depth == Some(observed.interrupt_depth))
    }
}

/// One executable action and its required observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Step {
    /// Operation sent to the backend.
    pub action: Action,
    /// Observable contract after the operation.
    pub expected: ExpectedObservation,
}

/// Stable scenario identity used by reports and CI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScenarioId {
    /// Lower numeric priority wins and equal priorities remain FIFO.
    PriorityThenFifo,
    /// Nested scheduler locks defer preemption until the outermost unlock.
    NestedSchedulerLock,
    /// Sleeping tasks become ready only at their monotonic deadline.
    SleepDeadline,
    /// A higher-priority ready task runs only after the outermost IRQ exits.
    NestedInterruptExit,
    /// Task exit switches away and permits later slot reuse.
    TaskExitAndReuse,
    /// Semaphore post transfers a grant directly to the first waiter.
    SemaphoreDirectHandoff,
    /// Timed-out semaphore waiters are removed before a later post.
    SemaphoreTimeoutCleanup,
    /// Recursive ownership, direct handoff and priority inheritance compose.
    MutexPriorityInheritance,
    /// A generation-bearing task identity becomes stale after slot reuse.
    StaleTaskIdentity,
    /// A zero-duration delay has exactly the same scheduling effect as yield.
    ZeroDelayYields,
    /// A wait-forever request cannot expire as monotonic time advances.
    WaitForever,
    /// Equal deadlines become ready in deterministic FIFO order.
    SameDeadlineFifo,
    /// A semaphore grant selects the highest-priority FIFO waiter.
    SemaphoreHighestPriorityWaiter,
    /// Releasing an unlocked scheduler fails without changing lock depth.
    UnbalancedSchedulerUnlock,
    /// Leaving task context as though it were an interrupt fails closed.
    UnbalancedInterruptExit,
    /// Blocking operations are rejected while the scheduler is locked.
    BlockingInSchedulerLock,
    /// Destroying the same resource handle twice fails closed.
    DuplicateResourceDestroy,
    /// A resource handle is stale after its slot is reused by a new generation.
    StaleResourceHandle,
    /// A semaphore with a queued waiter cannot be destroyed.
    SemaphoreBusyDestroy,
    /// A mutex with a live owner cannot be destroyed.
    MutexBusyDestroy,
    /// Cancelling an unconsumed semaphore handoff preserves its count.
    SemaphoreCancelAfterGrant,
    /// Cancelling an unconsumed mutex handoff preserves ownership transfer.
    MutexCancelAfterGrant,
}

impl ScenarioId {
    /// Returns the stable machine-readable scenario name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PriorityThenFifo => "priority_then_fifo",
            Self::NestedSchedulerLock => "nested_scheduler_lock",
            Self::SleepDeadline => "sleep_deadline",
            Self::NestedInterruptExit => "nested_interrupt_exit",
            Self::TaskExitAndReuse => "task_exit_and_reuse",
            Self::SemaphoreDirectHandoff => "semaphore_direct_handoff",
            Self::SemaphoreTimeoutCleanup => "semaphore_timeout_cleanup",
            Self::MutexPriorityInheritance => "mutex_priority_inheritance",
            Self::StaleTaskIdentity => "stale_task_identity",
            Self::ZeroDelayYields => "zero_delay_yields",
            Self::WaitForever => "wait_forever",
            Self::SameDeadlineFifo => "same_deadline_fifo",
            Self::SemaphoreHighestPriorityWaiter => "semaphore_highest_priority_waiter",
            Self::UnbalancedSchedulerUnlock => "unbalanced_scheduler_unlock",
            Self::UnbalancedInterruptExit => "unbalanced_interrupt_exit",
            Self::BlockingInSchedulerLock => "blocking_in_scheduler_lock",
            Self::DuplicateResourceDestroy => "duplicate_resource_destroy",
            Self::StaleResourceHandle => "stale_resource_handle",
            Self::SemaphoreBusyDestroy => "semaphore_busy_destroy",
            Self::MutexBusyDestroy => "mutex_busy_destroy",
            Self::SemaphoreCancelAfterGrant => "semaphore_cancel_after_grant",
            Self::MutexCancelAfterGrant => "mutex_cancel_after_grant",
        }
    }
}

/// One shared executable runtime scenario.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Scenario {
    /// Stable scenario identity.
    pub id: ScenarioId,
    /// Action/observation sequence.
    pub steps: &'static [Step],
}

/// Backend interface used only by deterministic conformance tests.
pub trait Backend {
    /// Contract advertised by the backend under test.
    fn contract(&self) -> RuntimeContract;

    /// Scheduling guarantees backed by the deterministic backend.
    fn execution_profile(&self) -> RuntimeExecutionProfile;

    /// Stable backend revision included in reports.
    fn revision(&self) -> &'static str;

    /// Applies one scenario action and returns the resulting state.
    fn apply(&mut self, action: Action) -> Result<Observation, Error>;
}

/// Reason a scenario failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureKind {
    /// Backend returned a runtime error.
    Backend(Error),
    /// Backend completed the action but produced the wrong state.
    UnexpectedObservation,
}

/// Failure details for one scenario.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Failure {
    /// Zero-based failing step.
    pub step: u16,
    /// Failure category.
    pub kind: FailureKind,
}

/// Result of one scenario.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScenarioResult {
    /// Stable scenario identity.
    pub id: ScenarioId,
    /// `None` means every step passed.
    pub failure: Option<Failure>,
}

/// Fixed-capacity conformance report suitable for CI and no-heap targets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Report<const N: usize> {
    /// Schema used by scenarios and JSON output.
    pub schema_version: u16,
    /// Contract advertised by the backend.
    pub contract: RuntimeContract,
    /// Scheduling guarantees exercised by this report.
    pub execution_profile: RuntimeExecutionProfile,
    /// Stable backend revision.
    pub backend_revision: &'static str,
    /// One result per requested scenario.
    pub results: [ScenarioResult; N],
}

impl<const N: usize> Report<N> {
    /// Returns true only when every requested scenario passed.
    pub fn all_passed(&self) -> bool {
        self.results.iter().all(|result| result.failure.is_none())
    }

    /// Writes deterministic JSON without requiring allocation or `serde`.
    pub fn write_json(&self, output: &mut impl fmt::Write) -> fmt::Result {
        write!(
            output,
            "{{\"schema_version\":{},\"contract\":{{\"major\":{},\"minor\":{},\"capabilities\":{}}},\"execution_profile\":{{\"revision\":{},\"modes\":{}}},\"backend_revision\":",
            self.schema_version,
            self.contract.version.major,
            self.contract.version.minor,
            self.contract.capabilities.bits(),
            self.execution_profile.revision,
            self.execution_profile.modes.bits(),
        )?;
        write_json_string(output, self.backend_revision)?;
        output.write_str(",\"scenarios\":[")?;
        for (index, result) in self.results.iter().enumerate() {
            if index != 0 {
                output.write_char(',')?;
            }
            output.write_str("{\"id\":")?;
            write_json_string(output, result.id.as_str())?;
            match result.failure {
                None => output.write_str(",\"status\":\"passed\"}")?,
                Some(failure) => {
                    write!(
                        output,
                        ",\"status\":\"failed\",\"step\":{},\"kind\":",
                        failure.step
                    )?;
                    let kind = match failure.kind {
                        FailureKind::Backend(_) => "backend",
                        FailureKind::UnexpectedObservation => "observation",
                    };
                    write_json_string(output, kind)?;
                    output.write_char('}')?;
                }
            }
        }
        output.write_str("]}")
    }
}

/// Runs the same fixed scenario suite against any deterministic backend.
pub fn run_suite<B: Backend, const N: usize>(
    backend: &mut B,
    scenarios: &[Scenario; N],
) -> Report<N> {
    let results = core::array::from_fn(|index| run_scenario(backend, scenarios[index]));
    Report {
        schema_version: SCHEMA_VERSION,
        contract: backend.contract(),
        execution_profile: backend.execution_profile(),
        backend_revision: backend.revision(),
        results,
    }
}

fn run_scenario(backend: &mut impl Backend, scenario: Scenario) -> ScenarioResult {
    for (index, step) in scenario.steps.iter().enumerate() {
        let observed = match backend.apply(step.action) {
            Ok(observed) => observed,
            Err(error) => {
                return ScenarioResult {
                    id: scenario.id,
                    failure: Some(Failure {
                        step: index as u16,
                        kind: FailureKind::Backend(error),
                    }),
                };
            }
        };
        if !step.expected.matches(observed) {
            return ScenarioResult {
                id: scenario.id,
                failure: Some(Failure {
                    step: index as u16,
                    kind: FailureKind::UnexpectedObservation,
                }),
            };
        }
    }
    ScenarioResult {
        id: scenario.id,
        failure: None,
    }
}

fn write_json_string(output: &mut impl fmt::Write, value: &str) -> fmt::Result {
    output.write_char('"')?;
    for character in value.chars() {
        match character {
            '"' => output.write_str("\\\"")?,
            '\\' => output.write_str("\\\\")?,
            '\n' => output.write_str("\\n")?,
            '\r' => output.write_str("\\r")?,
            '\t' => output.write_str("\\t")?,
            character if character.is_control() => write!(output, "\\u{:04x}", character as u32)?,
            character => output.write_char(character)?,
        }
    }
    output.write_char('"')
}

const ANY: ExpectedObservation = ExpectedObservation {
    running: None,
    subject: None,
    outcome: None,
    scheduler_lock_depth: None,
    interrupt_depth: None,
};

const PRIORITY_FIFO_STEPS: &[Step] = &[
    Step {
        action: Action::Reset {
            profile: ExecutionProfile::Cooperative,
            main_priority: TaskPriority::LOWEST,
        },
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            outcome: Some(ActionOutcome::Completed),
            scheduler_lock_depth: Some(0),
            interrupt_depth: Some(0),
            ..ANY
        },
    },
    Step {
        action: Action::Spawn {
            actor: ActorId::WORKER_A,
            priority: TaskPriority::new(4).unwrap(),
        },
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            subject: Some((ActorId::WORKER_A, ActorState::Ready)),
            outcome: Some(ActionOutcome::Spawned),
            ..ANY
        },
    },
    Step {
        action: Action::Spawn {
            actor: ActorId::WORKER_B,
            priority: TaskPriority::new(4).unwrap(),
        },
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            subject: Some((ActorId::WORKER_B, ActorState::Ready)),
            outcome: Some(ActionOutcome::Spawned),
            ..ANY
        },
    },
    Step {
        action: Action::Yield,
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            subject: Some((ActorId::MAIN, ActorState::Ready)),
            outcome: Some(ActionOutcome::ContextSwitched),
            ..ANY
        },
    },
    Step {
        action: Action::Yield,
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_B),
            subject: Some((ActorId::WORKER_A, ActorState::Ready)),
            outcome: Some(ActionOutcome::ContextSwitched),
            ..ANY
        },
    },
];

const NESTED_LOCK_STEPS: &[Step] = &[
    Step {
        action: Action::Reset {
            profile: ExecutionProfile::Preemptive,
            main_priority: TaskPriority::new(10).unwrap(),
        },
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            scheduler_lock_depth: Some(0),
            ..ANY
        },
    },
    Step {
        action: Action::LockScheduler,
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            scheduler_lock_depth: Some(1),
            ..ANY
        },
    },
    Step {
        action: Action::LockScheduler,
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            scheduler_lock_depth: Some(2),
            ..ANY
        },
    },
    Step {
        action: Action::Spawn {
            actor: ActorId::WORKER_A,
            priority: TaskPriority::new(2).unwrap(),
        },
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            subject: Some((ActorId::WORKER_A, ActorState::Ready)),
            outcome: Some(ActionOutcome::PreemptionDeferred),
            scheduler_lock_depth: Some(2),
            ..ANY
        },
    },
    Step {
        action: Action::UnlockScheduler,
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            scheduler_lock_depth: Some(1),
            ..ANY
        },
    },
    Step {
        action: Action::UnlockScheduler,
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            subject: Some((ActorId::MAIN, ActorState::Ready)),
            outcome: Some(ActionOutcome::ContextSwitched),
            scheduler_lock_depth: Some(0),
            ..ANY
        },
    },
];

const SLEEP_DEADLINE_STEPS: &[Step] = &[
    Step {
        action: Action::Reset {
            profile: ExecutionProfile::Cooperative,
            main_priority: TaskPriority::LOWEST,
        },
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            ..ANY
        },
    },
    Step {
        action: Action::Spawn {
            actor: ActorId::WORKER_A,
            priority: TaskPriority::new(4).unwrap(),
        },
        expected: ExpectedObservation {
            subject: Some((ActorId::WORKER_A, ActorState::Ready)),
            ..ANY
        },
    },
    Step {
        action: Action::Yield,
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            outcome: Some(ActionOutcome::ContextSwitched),
            ..ANY
        },
    },
    Step {
        action: Action::Sleep { milliseconds: 5 },
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            subject: Some((ActorId::WORKER_A, ActorState::Sleeping)),
            outcome: Some(ActionOutcome::ContextSwitched),
            ..ANY
        },
    },
    Step {
        action: Action::AdvanceTime { milliseconds: 4 },
        expected: ANY,
    },
    Step {
        action: Action::Observe {
            actor: ActorId::WORKER_A,
        },
        expected: ExpectedObservation {
            subject: Some((ActorId::WORKER_A, ActorState::Sleeping)),
            ..ANY
        },
    },
    Step {
        action: Action::AdvanceTime { milliseconds: 1 },
        expected: ANY,
    },
    Step {
        action: Action::Observe {
            actor: ActorId::WORKER_A,
        },
        expected: ExpectedObservation {
            subject: Some((ActorId::WORKER_A, ActorState::Ready)),
            ..ANY
        },
    },
    Step {
        action: Action::Yield,
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            outcome: Some(ActionOutcome::ContextSwitched),
            ..ANY
        },
    },
];

const NESTED_INTERRUPT_STEPS: &[Step] = &[
    Step {
        action: Action::Reset {
            profile: ExecutionProfile::Preemptive,
            main_priority: TaskPriority::new(10).unwrap(),
        },
        expected: ANY,
    },
    Step {
        action: Action::Spawn {
            actor: ActorId::WORKER_A,
            priority: TaskPriority::new(20).unwrap(),
        },
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            ..ANY
        },
    },
    Step {
        action: Action::Yield,
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            ..ANY
        },
    },
    Step {
        action: Action::EnterInterrupt,
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            interrupt_depth: Some(1),
            ..ANY
        },
    },
    Step {
        action: Action::EnterInterrupt,
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            interrupt_depth: Some(2),
            ..ANY
        },
    },
    Step {
        action: Action::ExitInterrupt,
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            interrupt_depth: Some(1),
            outcome: Some(ActionOutcome::Completed),
            ..ANY
        },
    },
    Step {
        action: Action::ExitInterrupt,
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            subject: Some((ActorId::WORKER_A, ActorState::Ready)),
            interrupt_depth: Some(0),
            outcome: Some(ActionOutcome::ContextSwitched),
            ..ANY
        },
    },
];

const TASK_EXIT_STEPS: &[Step] = &[
    Step {
        action: Action::Reset {
            profile: ExecutionProfile::Cooperative,
            main_priority: TaskPriority::LOWEST,
        },
        expected: ANY,
    },
    Step {
        action: Action::Spawn {
            actor: ActorId::WORKER_A,
            priority: TaskPriority::new(4).unwrap(),
        },
        expected: ANY,
    },
    Step {
        action: Action::Yield,
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            ..ANY
        },
    },
    Step {
        action: Action::ExitTask,
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            subject: Some((ActorId::WORKER_A, ActorState::Exited)),
            outcome: Some(ActionOutcome::ContextSwitched),
            ..ANY
        },
    },
    Step {
        action: Action::Spawn {
            actor: ActorId::WORKER_A,
            priority: TaskPriority::new(4).unwrap(),
        },
        expected: ExpectedObservation {
            subject: Some((ActorId::WORKER_A, ActorState::Ready)),
            outcome: Some(ActionOutcome::Spawned),
            ..ANY
        },
    },
];

const SEMAPHORE_HANDOFF_STEPS: &[Step] = &[
    Step {
        action: Action::Reset {
            profile: ExecutionProfile::Cooperative,
            main_priority: TaskPriority::LOWEST,
        },
        expected: ANY,
    },
    Step {
        action: Action::Spawn {
            actor: ActorId::WORKER_A,
            priority: TaskPriority::new(4).unwrap(),
        },
        expected: ANY,
    },
    Step {
        action: Action::Spawn {
            actor: ActorId::WORKER_B,
            priority: TaskPriority::new(4).unwrap(),
        },
        expected: ANY,
    },
    Step {
        action: Action::Yield,
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            ..ANY
        },
    },
    Step {
        action: Action::SemaphoreWait {
            timeout: Wait::Forever,
        },
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_B),
            subject: Some((ActorId::WORKER_A, ActorState::Blocked)),
            outcome: Some(ActionOutcome::ContextSwitched),
            ..ANY
        },
    },
    Step {
        action: Action::SemaphorePost,
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_B),
            subject: Some((ActorId::WORKER_A, ActorState::Ready)),
            outcome: Some(ActionOutcome::Granted),
            ..ANY
        },
    },
    Step {
        action: Action::ObserveGrant {
            actor: ActorId::WORKER_A,
        },
        expected: ExpectedObservation {
            subject: Some((ActorId::WORKER_A, ActorState::Ready)),
            outcome: Some(ActionOutcome::Acquired),
            ..ANY
        },
    },
];

const SEMAPHORE_TIMEOUT_STEPS: &[Step] = &[
    Step {
        action: Action::Reset {
            profile: ExecutionProfile::Cooperative,
            main_priority: TaskPriority::LOWEST,
        },
        expected: ANY,
    },
    Step {
        action: Action::Spawn {
            actor: ActorId::WORKER_A,
            priority: TaskPriority::new(4).unwrap(),
        },
        expected: ANY,
    },
    Step {
        action: Action::Yield,
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            ..ANY
        },
    },
    Step {
        action: Action::SemaphoreWait {
            timeout: Wait::Milliseconds(NonZeroU32::new(5).unwrap()),
        },
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            subject: Some((ActorId::WORKER_A, ActorState::Blocked)),
            outcome: Some(ActionOutcome::ContextSwitched),
            ..ANY
        },
    },
    Step {
        action: Action::AdvanceTime { milliseconds: 5 },
        expected: ANY,
    },
    Step {
        action: Action::ObserveGrant {
            actor: ActorId::WORKER_A,
        },
        expected: ExpectedObservation {
            subject: Some((ActorId::WORKER_A, ActorState::Ready)),
            outcome: Some(ActionOutcome::TimedOut),
            ..ANY
        },
    },
    Step {
        action: Action::SemaphorePost,
        expected: ExpectedObservation {
            outcome: Some(ActionOutcome::Completed),
            ..ANY
        },
    },
    Step {
        action: Action::Yield,
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            ..ANY
        },
    },
    Step {
        action: Action::SemaphoreWait {
            timeout: Wait::NoWait,
        },
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            outcome: Some(ActionOutcome::Acquired),
            ..ANY
        },
    },
];

const MUTEX_PI_STEPS: &[Step] = &[
    Step {
        action: Action::Reset {
            profile: ExecutionProfile::Cooperative,
            main_priority: TaskPriority::LOWEST,
        },
        expected: ANY,
    },
    Step {
        action: Action::Spawn {
            actor: ActorId::WORKER_A,
            priority: TaskPriority::new(20).unwrap(),
        },
        expected: ANY,
    },
    Step {
        action: Action::Yield,
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            ..ANY
        },
    },
    Step {
        action: Action::MutexLock {
            timeout: Wait::Forever,
        },
        expected: ExpectedObservation {
            outcome: Some(ActionOutcome::Acquired),
            ..ANY
        },
    },
    Step {
        action: Action::MutexLock {
            timeout: Wait::Forever,
        },
        expected: ExpectedObservation {
            outcome: Some(ActionOutcome::Acquired),
            ..ANY
        },
    },
    Step {
        action: Action::Spawn {
            actor: ActorId::WORKER_B,
            priority: TaskPriority::new(2).unwrap(),
        },
        expected: ANY,
    },
    Step {
        action: Action::Yield,
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_B),
            ..ANY
        },
    },
    Step {
        action: Action::MutexLock {
            timeout: Wait::Forever,
        },
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            subject: Some((ActorId::WORKER_B, ActorState::Blocked)),
            outcome: Some(ActionOutcome::ContextSwitched),
            ..ANY
        },
    },
    Step {
        action: Action::ObservePriority {
            actor: ActorId::WORKER_A,
        },
        expected: ExpectedObservation {
            outcome: Some(ActionOutcome::PriorityObserved(
                TaskPriority::new(2).unwrap(),
            )),
            ..ANY
        },
    },
    Step {
        action: Action::MutexUnlock,
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            outcome: Some(ActionOutcome::Completed),
            ..ANY
        },
    },
    Step {
        action: Action::MutexUnlock,
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            subject: Some((ActorId::WORKER_B, ActorState::Ready)),
            outcome: Some(ActionOutcome::Granted),
            ..ANY
        },
    },
    Step {
        action: Action::ObservePriority {
            actor: ActorId::WORKER_A,
        },
        expected: ExpectedObservation {
            outcome: Some(ActionOutcome::PriorityObserved(
                TaskPriority::new(20).unwrap(),
            )),
            ..ANY
        },
    },
    Step {
        action: Action::ObserveGrant {
            actor: ActorId::WORKER_B,
        },
        expected: ExpectedObservation {
            outcome: Some(ActionOutcome::Acquired),
            ..ANY
        },
    },
];

const STALE_IDENTITY_STEPS: &[Step] = &[
    Step {
        action: Action::Reset {
            profile: ExecutionProfile::Cooperative,
            main_priority: TaskPriority::LOWEST,
        },
        expected: ANY,
    },
    Step {
        action: Action::Spawn {
            actor: ActorId::WORKER_A,
            priority: TaskPriority::new(4).unwrap(),
        },
        expected: ANY,
    },
    Step {
        action: Action::RememberIdentity {
            actor: ActorId::WORKER_A,
        },
        expected: ExpectedObservation {
            outcome: Some(ActionOutcome::IdentityRemembered),
            ..ANY
        },
    },
    Step {
        action: Action::Yield,
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            ..ANY
        },
    },
    Step {
        action: Action::ExitTask,
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            ..ANY
        },
    },
    Step {
        action: Action::Spawn {
            actor: ActorId::WORKER_A,
            priority: TaskPriority::new(4).unwrap(),
        },
        expected: ANY,
    },
    Step {
        action: Action::ValidateRememberedIdentity,
        expected: ExpectedObservation {
            outcome: Some(ActionOutcome::StaleIdentityRejected),
            ..ANY
        },
    },
];

const ZERO_DELAY_STEPS: &[Step] = &[
    Step {
        action: Action::Reset {
            profile: ExecutionProfile::Cooperative,
            main_priority: TaskPriority::LOWEST,
        },
        expected: ANY,
    },
    Step {
        action: Action::Spawn {
            actor: ActorId::WORKER_A,
            priority: TaskPriority::new(4).unwrap(),
        },
        expected: ANY,
    },
    Step {
        action: Action::Delay { milliseconds: 0 },
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            subject: Some((ActorId::MAIN, ActorState::Ready)),
            outcome: Some(ActionOutcome::ContextSwitched),
            ..ANY
        },
    },
];

const WAIT_FOREVER_STEPS: &[Step] = &[
    Step {
        action: Action::Reset {
            profile: ExecutionProfile::Cooperative,
            main_priority: TaskPriority::LOWEST,
        },
        expected: ANY,
    },
    Step {
        action: Action::Spawn {
            actor: ActorId::WORKER_A,
            priority: TaskPriority::new(4).unwrap(),
        },
        expected: ANY,
    },
    Step {
        action: Action::Yield,
        expected: ANY,
    },
    Step {
        action: Action::SemaphoreWait {
            timeout: Wait::Forever,
        },
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            subject: Some((ActorId::WORKER_A, ActorState::Blocked)),
            outcome: Some(ActionOutcome::ContextSwitched),
            ..ANY
        },
    },
    Step {
        action: Action::AdvanceTime {
            milliseconds: u32::MAX,
        },
        expected: ANY,
    },
    Step {
        action: Action::Observe {
            actor: ActorId::WORKER_A,
        },
        expected: ExpectedObservation {
            subject: Some((ActorId::WORKER_A, ActorState::Blocked)),
            ..ANY
        },
    },
    Step {
        action: Action::SemaphorePost,
        expected: ExpectedObservation {
            subject: Some((ActorId::WORKER_A, ActorState::Ready)),
            outcome: Some(ActionOutcome::Granted),
            ..ANY
        },
    },
];

const SAME_DEADLINE_STEPS: &[Step] = &[
    Step {
        action: Action::Reset {
            profile: ExecutionProfile::Cooperative,
            main_priority: TaskPriority::LOWEST,
        },
        expected: ANY,
    },
    Step {
        action: Action::Spawn {
            actor: ActorId::WORKER_A,
            priority: TaskPriority::new(4).unwrap(),
        },
        expected: ANY,
    },
    Step {
        action: Action::Spawn {
            actor: ActorId::WORKER_B,
            priority: TaskPriority::new(4).unwrap(),
        },
        expected: ANY,
    },
    Step {
        action: Action::Yield,
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            ..ANY
        },
    },
    Step {
        action: Action::Sleep { milliseconds: 5 },
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_B),
            ..ANY
        },
    },
    Step {
        action: Action::Sleep { milliseconds: 5 },
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            ..ANY
        },
    },
    Step {
        action: Action::AdvanceTime { milliseconds: 5 },
        expected: ANY,
    },
    Step {
        action: Action::Yield,
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            outcome: Some(ActionOutcome::ContextSwitched),
            ..ANY
        },
    },
];

const SEMAPHORE_PRIORITY_STEPS: &[Step] = &[
    Step {
        action: Action::Reset {
            profile: ExecutionProfile::Cooperative,
            main_priority: TaskPriority::LOWEST,
        },
        expected: ANY,
    },
    Step {
        action: Action::Spawn {
            actor: ActorId::WORKER_A,
            priority: TaskPriority::new(10).unwrap(),
        },
        expected: ANY,
    },
    Step {
        action: Action::Yield,
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            ..ANY
        },
    },
    Step {
        action: Action::Spawn {
            actor: ActorId::WORKER_B,
            priority: TaskPriority::new(2).unwrap(),
        },
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            ..ANY
        },
    },
    Step {
        action: Action::SemaphoreWait {
            timeout: Wait::Forever,
        },
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_B),
            ..ANY
        },
    },
    Step {
        action: Action::SemaphoreWait {
            timeout: Wait::Forever,
        },
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            ..ANY
        },
    },
    Step {
        action: Action::SemaphorePost,
        expected: ExpectedObservation {
            subject: Some((ActorId::WORKER_B, ActorState::Ready)),
            outcome: Some(ActionOutcome::Granted),
            ..ANY
        },
    },
    Step {
        action: Action::ObserveGrant {
            actor: ActorId::WORKER_B,
        },
        expected: ExpectedObservation {
            outcome: Some(ActionOutcome::Acquired),
            ..ANY
        },
    },
];

const UNBALANCED_SCHEDULER_UNLOCK_STEPS: &[Step] = &[
    Step {
        action: Action::Reset {
            profile: ExecutionProfile::Cooperative,
            main_priority: TaskPriority::LOWEST,
        },
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            outcome: Some(ActionOutcome::Completed),
            scheduler_lock_depth: Some(0),
            ..ANY
        },
    },
    Step {
        action: Action::UnlockScheduler,
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            outcome: Some(ActionOutcome::Rejected(Error::InvalidContext)),
            scheduler_lock_depth: Some(0),
            ..ANY
        },
    },
];

const UNBALANCED_INTERRUPT_EXIT_STEPS: &[Step] = &[
    Step {
        action: Action::Reset {
            profile: ExecutionProfile::Cooperative,
            main_priority: TaskPriority::LOWEST,
        },
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            outcome: Some(ActionOutcome::Completed),
            interrupt_depth: Some(0),
            ..ANY
        },
    },
    Step {
        action: Action::ExitInterrupt,
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            outcome: Some(ActionOutcome::Rejected(Error::InvalidContext)),
            interrupt_depth: Some(0),
            ..ANY
        },
    },
];

const BLOCKING_IN_SCHEDULER_LOCK_STEPS: &[Step] = &[
    Step {
        action: Action::Reset {
            profile: ExecutionProfile::Cooperative,
            main_priority: TaskPriority::LOWEST,
        },
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            outcome: Some(ActionOutcome::Completed),
            ..ANY
        },
    },
    Step {
        action: Action::MutexLock {
            timeout: Wait::NoWait,
        },
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            outcome: Some(ActionOutcome::Acquired),
            ..ANY
        },
    },
    Step {
        action: Action::Spawn {
            actor: ActorId::WORKER_A,
            priority: TaskPriority::LOWEST,
        },
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            outcome: Some(ActionOutcome::Spawned),
            ..ANY
        },
    },
    Step {
        action: Action::Yield,
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            outcome: Some(ActionOutcome::ContextSwitched),
            ..ANY
        },
    },
    Step {
        action: Action::LockScheduler,
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            outcome: Some(ActionOutcome::Completed),
            scheduler_lock_depth: Some(1),
            ..ANY
        },
    },
    Step {
        action: Action::Sleep { milliseconds: 1 },
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            outcome: Some(ActionOutcome::Rejected(Error::InvalidContext)),
            scheduler_lock_depth: Some(1),
            ..ANY
        },
    },
    Step {
        action: Action::SemaphoreWait {
            timeout: Wait::Forever,
        },
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            outcome: Some(ActionOutcome::Rejected(Error::InvalidContext)),
            scheduler_lock_depth: Some(1),
            ..ANY
        },
    },
    Step {
        action: Action::MutexLock {
            timeout: Wait::Forever,
        },
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            outcome: Some(ActionOutcome::Rejected(Error::InvalidContext)),
            scheduler_lock_depth: Some(1),
            ..ANY
        },
    },
    Step {
        action: Action::UnlockScheduler,
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            outcome: Some(ActionOutcome::Completed),
            scheduler_lock_depth: Some(0),
            ..ANY
        },
    },
];

const DUPLICATE_RESOURCE_DESTROY_STEPS: &[Step] = &[
    Step {
        action: Action::Reset {
            profile: ExecutionProfile::Cooperative,
            main_priority: TaskPriority::LOWEST,
        },
        expected: ANY,
    },
    Step {
        action: Action::CreateResource {
            kind: ResourceKind::Semaphore,
        },
        expected: ExpectedObservation {
            outcome: Some(ActionOutcome::ResourceCreated),
            ..ANY
        },
    },
    Step {
        action: Action::RememberResourceHandle {
            kind: ResourceKind::Semaphore,
        },
        expected: ExpectedObservation {
            outcome: Some(ActionOutcome::ResourceHandleRemembered),
            ..ANY
        },
    },
    Step {
        action: Action::DestroyResource {
            kind: ResourceKind::Semaphore,
            handle: ResourceHandleRef::Current,
        },
        expected: ExpectedObservation {
            outcome: Some(ActionOutcome::ResourceDestroyed),
            ..ANY
        },
    },
    Step {
        action: Action::DestroyResource {
            kind: ResourceKind::Semaphore,
            handle: ResourceHandleRef::Remembered,
        },
        expected: ExpectedObservation {
            outcome: Some(ActionOutcome::Rejected(Error::InvalidHandle)),
            ..ANY
        },
    },
];

const STALE_RESOURCE_HANDLE_STEPS: &[Step] = &[
    Step {
        action: Action::Reset {
            profile: ExecutionProfile::Cooperative,
            main_priority: TaskPriority::LOWEST,
        },
        expected: ANY,
    },
    Step {
        action: Action::CreateResource {
            kind: ResourceKind::Semaphore,
        },
        expected: ANY,
    },
    Step {
        action: Action::RememberResourceHandle {
            kind: ResourceKind::Semaphore,
        },
        expected: ANY,
    },
    Step {
        action: Action::DestroyResource {
            kind: ResourceKind::Semaphore,
            handle: ResourceHandleRef::Current,
        },
        expected: ANY,
    },
    Step {
        action: Action::CreateResource {
            kind: ResourceKind::Semaphore,
        },
        expected: ExpectedObservation {
            outcome: Some(ActionOutcome::ResourceCreated),
            ..ANY
        },
    },
    Step {
        action: Action::DestroyResource {
            kind: ResourceKind::Semaphore,
            handle: ResourceHandleRef::Remembered,
        },
        expected: ExpectedObservation {
            outcome: Some(ActionOutcome::Rejected(Error::InvalidHandle)),
            ..ANY
        },
    },
    Step {
        action: Action::DestroyResource {
            kind: ResourceKind::Semaphore,
            handle: ResourceHandleRef::Current,
        },
        expected: ExpectedObservation {
            outcome: Some(ActionOutcome::ResourceDestroyed),
            ..ANY
        },
    },
];

const SEMAPHORE_BUSY_DESTROY_STEPS: &[Step] = &[
    Step {
        action: Action::Reset {
            profile: ExecutionProfile::Preemptive,
            main_priority: TaskPriority::LOWEST,
        },
        expected: ANY,
    },
    Step {
        action: Action::CreateResource {
            kind: ResourceKind::Semaphore,
        },
        expected: ANY,
    },
    Step {
        action: Action::Spawn {
            actor: ActorId::WORKER_A,
            priority: TaskPriority::new(4).unwrap(),
        },
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            ..ANY
        },
    },
    Step {
        action: Action::SemaphoreWait {
            timeout: Wait::Forever,
        },
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            subject: Some((ActorId::WORKER_A, ActorState::Blocked)),
            ..ANY
        },
    },
    Step {
        action: Action::DestroyResource {
            kind: ResourceKind::Semaphore,
            handle: ResourceHandleRef::Current,
        },
        expected: ExpectedObservation {
            outcome: Some(ActionOutcome::Rejected(Error::InvalidContext)),
            ..ANY
        },
    },
];

const MUTEX_BUSY_DESTROY_STEPS: &[Step] = &[
    Step {
        action: Action::Reset {
            profile: ExecutionProfile::Cooperative,
            main_priority: TaskPriority::LOWEST,
        },
        expected: ANY,
    },
    Step {
        action: Action::CreateResource {
            kind: ResourceKind::Mutex,
        },
        expected: ANY,
    },
    Step {
        action: Action::MutexLock {
            timeout: Wait::NoWait,
        },
        expected: ExpectedObservation {
            outcome: Some(ActionOutcome::Acquired),
            ..ANY
        },
    },
    Step {
        action: Action::DestroyResource {
            kind: ResourceKind::Mutex,
            handle: ResourceHandleRef::Current,
        },
        expected: ExpectedObservation {
            outcome: Some(ActionOutcome::Rejected(Error::InvalidContext)),
            ..ANY
        },
    },
];

const SEMAPHORE_CANCEL_AFTER_GRANT_STEPS: &[Step] = &[
    Step {
        action: Action::Reset {
            profile: ExecutionProfile::Preemptive,
            main_priority: TaskPriority::LOWEST,
        },
        expected: ANY,
    },
    Step {
        action: Action::CreateResource {
            kind: ResourceKind::Semaphore,
        },
        expected: ANY,
    },
    Step {
        action: Action::Spawn {
            actor: ActorId::WORKER_A,
            priority: TaskPriority::new(4).unwrap(),
        },
        expected: ANY,
    },
    Step {
        action: Action::SemaphoreWait {
            timeout: Wait::Forever,
        },
        expected: ANY,
    },
    Step {
        action: Action::SemaphorePost,
        expected: ExpectedObservation {
            subject: Some((ActorId::WORKER_A, ActorState::Ready)),
            outcome: Some(ActionOutcome::Granted),
            ..ANY
        },
    },
    Step {
        action: Action::DestroyResource {
            kind: ResourceKind::Semaphore,
            handle: ResourceHandleRef::Current,
        },
        expected: ExpectedObservation {
            outcome: Some(ActionOutcome::Rejected(Error::InvalidContext)),
            ..ANY
        },
    },
    Step {
        action: Action::CancelWait {
            actor: ActorId::WORKER_A,
        },
        expected: ExpectedObservation {
            subject: Some((ActorId::WORKER_A, ActorState::Ready)),
            outcome: Some(ActionOutcome::WaitCancelled),
            ..ANY
        },
    },
    Step {
        action: Action::SemaphoreWait {
            timeout: Wait::NoWait,
        },
        expected: ExpectedObservation {
            outcome: Some(ActionOutcome::Acquired),
            ..ANY
        },
    },
];

const MUTEX_CANCEL_AFTER_GRANT_STEPS: &[Step] = &[
    Step {
        action: Action::Reset {
            profile: ExecutionProfile::Cooperative,
            main_priority: TaskPriority::new(10).unwrap(),
        },
        expected: ANY,
    },
    Step {
        action: Action::CreateResource {
            kind: ResourceKind::Mutex,
        },
        expected: ANY,
    },
    Step {
        action: Action::MutexLock {
            timeout: Wait::NoWait,
        },
        expected: ExpectedObservation {
            outcome: Some(ActionOutcome::Acquired),
            ..ANY
        },
    },
    Step {
        action: Action::Spawn {
            actor: ActorId::WORKER_A,
            priority: TaskPriority::new(4).unwrap(),
        },
        expected: ANY,
    },
    Step {
        action: Action::Yield,
        expected: ExpectedObservation {
            running: Some(ActorId::WORKER_A),
            ..ANY
        },
    },
    Step {
        action: Action::MutexLock {
            timeout: Wait::Forever,
        },
        expected: ExpectedObservation {
            running: Some(ActorId::MAIN),
            subject: Some((ActorId::WORKER_A, ActorState::Blocked)),
            ..ANY
        },
    },
    Step {
        action: Action::MutexUnlock,
        expected: ExpectedObservation {
            subject: Some((ActorId::WORKER_A, ActorState::Ready)),
            outcome: Some(ActionOutcome::Granted),
            ..ANY
        },
    },
    Step {
        action: Action::CancelWait {
            actor: ActorId::WORKER_A,
        },
        expected: ExpectedObservation {
            subject: Some((ActorId::WORKER_A, ActorState::Ready)),
            outcome: Some(ActionOutcome::WaitCancelled),
            ..ANY
        },
    },
    Step {
        action: Action::MutexLock {
            timeout: Wait::NoWait,
        },
        expected: ExpectedObservation {
            outcome: Some(ActionOutcome::Acquired),
            ..ANY
        },
    },
];

/// Shared scheduler and synchronization semantics required by contract V1.
pub const V1_SCENARIOS: [Scenario; 22] = [
    Scenario {
        id: ScenarioId::PriorityThenFifo,
        steps: PRIORITY_FIFO_STEPS,
    },
    Scenario {
        id: ScenarioId::NestedSchedulerLock,
        steps: NESTED_LOCK_STEPS,
    },
    Scenario {
        id: ScenarioId::SleepDeadline,
        steps: SLEEP_DEADLINE_STEPS,
    },
    Scenario {
        id: ScenarioId::NestedInterruptExit,
        steps: NESTED_INTERRUPT_STEPS,
    },
    Scenario {
        id: ScenarioId::TaskExitAndReuse,
        steps: TASK_EXIT_STEPS,
    },
    Scenario {
        id: ScenarioId::SemaphoreDirectHandoff,
        steps: SEMAPHORE_HANDOFF_STEPS,
    },
    Scenario {
        id: ScenarioId::SemaphoreTimeoutCleanup,
        steps: SEMAPHORE_TIMEOUT_STEPS,
    },
    Scenario {
        id: ScenarioId::MutexPriorityInheritance,
        steps: MUTEX_PI_STEPS,
    },
    Scenario {
        id: ScenarioId::StaleTaskIdentity,
        steps: STALE_IDENTITY_STEPS,
    },
    Scenario {
        id: ScenarioId::ZeroDelayYields,
        steps: ZERO_DELAY_STEPS,
    },
    Scenario {
        id: ScenarioId::WaitForever,
        steps: WAIT_FOREVER_STEPS,
    },
    Scenario {
        id: ScenarioId::SameDeadlineFifo,
        steps: SAME_DEADLINE_STEPS,
    },
    Scenario {
        id: ScenarioId::SemaphoreHighestPriorityWaiter,
        steps: SEMAPHORE_PRIORITY_STEPS,
    },
    Scenario {
        id: ScenarioId::UnbalancedSchedulerUnlock,
        steps: UNBALANCED_SCHEDULER_UNLOCK_STEPS,
    },
    Scenario {
        id: ScenarioId::UnbalancedInterruptExit,
        steps: UNBALANCED_INTERRUPT_EXIT_STEPS,
    },
    Scenario {
        id: ScenarioId::BlockingInSchedulerLock,
        steps: BLOCKING_IN_SCHEDULER_LOCK_STEPS,
    },
    Scenario {
        id: ScenarioId::DuplicateResourceDestroy,
        steps: DUPLICATE_RESOURCE_DESTROY_STEPS,
    },
    Scenario {
        id: ScenarioId::StaleResourceHandle,
        steps: STALE_RESOURCE_HANDLE_STEPS,
    },
    Scenario {
        id: ScenarioId::SemaphoreBusyDestroy,
        steps: SEMAPHORE_BUSY_DESTROY_STEPS,
    },
    Scenario {
        id: ScenarioId::MutexBusyDestroy,
        steps: MUTEX_BUSY_DESTROY_STEPS,
    },
    Scenario {
        id: ScenarioId::SemaphoreCancelAfterGrant,
        steps: SEMAPHORE_CANCEL_AFTER_GRANT_STEPS,
    },
    Scenario {
        id: ScenarioId::MutexCancelAfterGrant,
        steps: MUTEX_CANCEL_AFTER_GRANT_STEPS,
    },
];

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    struct StaticBackend;

    impl Backend for StaticBackend {
        fn contract(&self) -> RuntimeContract {
            RuntimeContract::V1
        }

        fn execution_profile(&self) -> RuntimeExecutionProfile {
            RuntimeExecutionProfile::V1_PORTED
        }

        fn revision(&self) -> &'static str {
            "test\"backend"
        }

        fn apply(&mut self, _action: Action) -> Result<Observation, Error> {
            Ok(Observation {
                running: ActorId::MAIN,
                subject: None,
                outcome: ActionOutcome::Completed,
                scheduler_lock_depth: 0,
                interrupt_depth: 0,
            })
        }
    }

    #[test]
    fn selected_observation_fields_are_matched() {
        let expected = ExpectedObservation {
            running: Some(ActorId::MAIN),
            subject: None,
            outcome: Some(ActionOutcome::Completed),
            scheduler_lock_depth: None,
            interrupt_depth: None,
        };
        assert!(expected.matches(Observation {
            running: ActorId::MAIN,
            subject: Some((ActorId::WORKER_A, ActorState::Ready)),
            outcome: ActionOutcome::Completed,
            scheduler_lock_depth: 7,
            interrupt_depth: 3,
        }));
    }

    #[test]
    fn report_is_json_and_mismatch_fails_closed() {
        let mut backend = StaticBackend;
        let report = run_suite(&mut backend, &V1_SCENARIOS);
        assert!(!report.all_passed());
        assert_eq!(report.results[0].failure.unwrap().step, 1);

        let mut json = std::string::String::new();
        report.write_json(&mut json).unwrap();
        assert!(json.contains("\"backend_revision\":\"test\\\"backend\""));
        assert!(json.contains("\"execution_profile\":{\"revision\":1,\"modes\":14}"));
        assert!(json.contains("\"status\":\"failed\""));
    }
}
