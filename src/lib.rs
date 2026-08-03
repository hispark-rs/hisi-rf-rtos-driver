//! Runtime-neutral services required by HiSilicon radio firmware.
//!
//! This crate is the narrow dependency shared by radio adapters and an RTOS.
//! It does not implement a scheduler and does not know about a chip, protocol,
//! vendor blob, allocator, or network stack. A firmware installs exactly one
//! [`Runtime`] before initializing its radio controller.

#![no_std]

pub mod conformance;
mod contract;

use core::cell::Cell;
use core::ffi::c_void;
use core::num::{NonZeroU32, NonZeroUsize};
use critical_section::Mutex;

pub use contract::{
    InvalidTaskPriority, RuntimeCapabilities, RuntimeContract, RuntimeContractVersion,
    RuntimeExecutionModes, RuntimeExecutionProfile, RuntimeRequirements, TASK_PRIORITY_LEVELS,
    TaskPriority,
};

/// Snapshot of dynamic task slots owned by the installed runtime.
///
/// This is a preflight observation, not a reservation. Another subsystem may
/// consume slots after the snapshot is returned. Race-free admission uses an
/// owner-bound reservation token consumed by corresponding spawn operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskCapacity {
    dynamic_capacity: usize,
    dynamic_used: usize,
    dynamic_reserved: usize,
}

impl TaskCapacity {
    /// Builds a valid capacity snapshot.
    pub const fn new(dynamic_capacity: usize, dynamic_used: usize) -> Option<Self> {
        if dynamic_used <= dynamic_capacity {
            Some(Self {
                dynamic_capacity,
                dynamic_used,
                dynamic_reserved: 0,
            })
        } else {
            None
        }
    }

    /// Builds a snapshot that also accounts for unconsumed reservations.
    pub const fn new_with_reserved(
        dynamic_capacity: usize,
        dynamic_used: usize,
        dynamic_reserved: usize,
    ) -> Option<Self> {
        if dynamic_used <= dynamic_capacity && dynamic_reserved <= dynamic_capacity - dynamic_used {
            Some(Self {
                dynamic_capacity,
                dynamic_used,
                dynamic_reserved,
            })
        } else {
            None
        }
    }

    /// Total number of dynamic slots in this runtime instance.
    pub const fn dynamic_capacity(self) -> usize {
        self.dynamic_capacity
    }

    /// Dynamic slots occupied by live tasks at snapshot time.
    pub const fn dynamic_used(self) -> usize {
        self.dynamic_used
    }

    /// Dynamic slots promised to live reservation tokens but not yet consumed.
    pub const fn dynamic_reserved(self) -> usize {
        self.dynamic_reserved
    }

    /// Dynamic slots available at snapshot time.
    pub const fn dynamic_available(self) -> usize {
        self.dynamic_capacity - self.dynamic_used - self.dynamic_reserved
    }
}

/// Opaque owner-bound reservation for dynamic task slots.
///
/// The token is intentionally not `Copy` or `Clone`. A runtime validates its
/// generation on every operation; releasing it invalidates subsequent spawns.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "a task reservation must be retained until its reserved spawns finish"]
pub struct TaskReservation(NonZeroU32);

impl TaskReservation {
    /// Creates a token from a runtime-owned generation-bearing identity.
    ///
    /// # Safety
    ///
    /// `raw` must identify a live reservation owned by the implementing runtime.
    pub const unsafe fn from_raw(raw: NonZeroU32) -> Self {
        Self(raw)
    }

    /// Returns the runtime-owned opaque identity.
    pub const fn into_raw(&self) -> NonZeroU32 {
        self.0
    }
}

/// Atomic task-slot and stack-memory requirements for one subsystem owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskResourceRequirements {
    task_slots: NonZeroUsize,
    stack_bytes_per_task: NonZeroUsize,
}

/// Maximum number of independently owned task groups admitted atomically.
///
/// This is a contract bound, not a chip task limit. A composition with more
/// groups must aggregate adjacent owners before crossing the runtime boundary.
pub const TASK_RESOURCE_GROUP_CAPACITY: usize = 4;

/// Stable, runtime-opaque identity for one task-resource owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct TaskResourceOwner(NonZeroU32);

impl TaskResourceOwner {
    /// Construct an owner identity from a composition-defined non-zero value.
    pub const fn new(raw: NonZeroU32) -> Self {
        Self(raw)
    }

    /// Return the stable numeric identity used in diagnostics and reports.
    pub const fn into_raw(self) -> NonZeroU32 {
        self.0
    }
}

/// One independently owned task group within an atomic resource plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskResourceGroupRequirements {
    owner: TaskResourceOwner,
    resources: TaskResourceRequirements,
}

impl TaskResourceGroupRequirements {
    /// Bind one uniform-stack task group to its owner identity.
    pub const fn new(owner: TaskResourceOwner, resources: TaskResourceRequirements) -> Self {
        Self { owner, resources }
    }

    /// Owner receiving the resulting reservation token.
    pub const fn owner(self) -> TaskResourceOwner {
        self.owner
    }

    /// Slot and stack requirements for this group.
    pub const fn resources(self) -> TaskResourceRequirements {
        self.resources
    }
}

/// Checked, heterogeneous task-resource plan admitted as one transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskResourcePlan<'a> {
    groups: &'a [TaskResourceGroupRequirements],
    total_task_slots: usize,
    total_stack_bytes: usize,
}

impl<'a> TaskResourcePlan<'a> {
    /// Validate group count, owner uniqueness, and checked totals.
    pub const fn new(groups: &'a [TaskResourceGroupRequirements]) -> Option<Self> {
        if groups.is_empty() || groups.len() > TASK_RESOURCE_GROUP_CAPACITY {
            return None;
        }
        let mut total_task_slots = 0usize;
        let mut total_stack_bytes = 0usize;
        let mut index = 0usize;
        while index < groups.len() {
            let group = groups[index];
            let mut previous = 0usize;
            while previous < index {
                if groups[previous].owner.into_raw().get() == group.owner.into_raw().get() {
                    return None;
                }
                previous += 1;
            }
            total_task_slots =
                match total_task_slots.checked_add(group.resources.task_slots().get()) {
                    Some(value) => value,
                    None => return None,
                };
            total_stack_bytes =
                match total_stack_bytes.checked_add(group.resources.total_stack_bytes()) {
                    Some(value) => value,
                    None => return None,
                };
            index += 1;
        }
        Some(Self {
            groups,
            total_task_slots,
            total_stack_bytes,
        })
    }

    /// Ordered owner groups. Reservation results use this same order.
    pub const fn groups(self) -> &'a [TaskResourceGroupRequirements] {
        self.groups
    }

    /// Total dynamic slots derived from all child groups.
    pub const fn total_task_slots(self) -> usize {
        self.total_task_slots
    }

    /// Total task-stack payload derived from all child groups.
    pub const fn total_stack_bytes(self) -> usize {
        self.total_stack_bytes
    }
}

/// Owner-bound reservations returned by one atomic resource-plan admission.
#[derive(Debug)]
#[must_use = "every reservation in the admitted batch must be retained or released"]
pub struct TaskReservationBatch {
    reservations: [Option<TaskReservation>; TASK_RESOURCE_GROUP_CAPACITY],
    len: usize,
}

impl TaskReservationBatch {
    /// Construct a batch from runtime-owned reservation identities.
    ///
    /// # Safety
    ///
    /// Every populated entry must be a distinct live reservation created by
    /// the implementing runtime, and `len` must cover exactly those entries.
    pub unsafe fn from_reservations(
        reservations: [Option<TaskReservation>; TASK_RESOURCE_GROUP_CAPACITY],
        len: usize,
    ) -> Self {
        debug_assert!(len <= TASK_RESOURCE_GROUP_CAPACITY);
        debug_assert!(reservations[..len].iter().all(Option::is_some));
        debug_assert!(reservations[len..].iter().all(Option::is_none));
        Self { reservations, len }
    }

    /// Number of child reservations in plan order.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether the batch contains no child reservation.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Remove one child reservation by its plan-order index.
    pub fn take(&mut self, index: usize) -> Option<TaskReservation> {
        self.reservations.get_mut(index)?.take()
    }

    /// Number of reservations not yet removed from this batch.
    pub fn remaining(&self) -> usize {
        self.reservations[..self.len]
            .iter()
            .filter(|reservation| reservation.is_some())
            .count()
    }
}

impl Drop for TaskReservationBatch {
    fn drop(&mut self) {
        for reservation in self.reservations[..self.len]
            .iter_mut()
            .filter_map(Option::take)
        {
            let _ = release_task_reservation(&reservation);
        }
    }
}

impl TaskResourceRequirements {
    /// Construct a checked task-resource request.
    pub const fn new(task_slots: NonZeroUsize, stack_bytes_per_task: NonZeroUsize) -> Option<Self> {
        if task_slots
            .get()
            .checked_mul(stack_bytes_per_task.get())
            .is_none()
        {
            return None;
        }
        Some(Self {
            task_slots,
            stack_bytes_per_task,
        })
    }

    /// Dynamic task slots required by the owner.
    pub const fn task_slots(self) -> NonZeroUsize {
        self.task_slots
    }

    /// Stack bytes reserved for each task.
    pub const fn stack_bytes_per_task(self) -> NonZeroUsize {
        self.stack_bytes_per_task
    }

    /// Total task-stack bytes reserved by this request.
    pub const fn total_stack_bytes(self) -> usize {
        self.task_slots.get() * self.stack_bytes_per_task.get()
    }
}

/// Failure to satisfy task-resource admission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskAdmissionError {
    /// Runtime discovery or capacity reporting failed.
    Runtime(Error),
    /// The snapshot did not contain enough free dynamic slots.
    InsufficientTaskSlots {
        /// Slots required by the selected radio profile.
        required: usize,
        /// Slots free when the snapshot was taken.
        available: usize,
    },
    /// The runtime could not reserve every requested task stack.
    InsufficientTaskStackMemory {
        /// Total stack bytes required by the selected profile.
        required: usize,
        /// Bytes successfully reserved before allocation failed.
        available: usize,
    },
    /// One child group could not reserve all of its task slots.
    InsufficientTaskGroupSlots {
        /// Composition-defined owner of the failing group.
        owner: TaskResourceOwner,
        /// Slots required by this child group.
        required: usize,
        /// Slots available to the complete atomic transaction.
        available: usize,
    },
    /// One child group could not reserve every requested stack.
    InsufficientTaskGroupStackMemory {
        /// Composition-defined owner of the failing group.
        owner: TaskResourceOwner,
        /// Stack bytes required by this child group.
        required: usize,
        /// Stack payload successfully allocated before rollback.
        available: usize,
        /// Largest payload allocation possible at the failure point.
        largest_contiguous: usize,
    },
}

/// Entry point used by a vendor-compatible task.
pub type TaskEntry = extern "C" fn(*mut c_void) -> *mut c_void;

/// Opaque task identity owned by the installed runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskId(u32);

impl TaskId {
    /// Creates an identity from a runtime-owned raw value.
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// Returns the runtime-owned raw value.
    pub const fn into_raw(self) -> u32 {
        self.0
    }
}

/// Opaque semaphore identity owned by the installed runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SemaphoreHandle(NonZeroUsize);

impl SemaphoreHandle {
    /// Creates a handle after a runtime allocated the corresponding object.
    ///
    /// # Safety
    ///
    /// `raw` must uniquely identify a live semaphore in that runtime until the
    /// matching destroy operation completes.
    pub const unsafe fn from_raw(raw: NonZeroUsize) -> Self {
        Self(raw)
    }

    /// Returns the runtime-owned opaque value.
    pub const fn into_raw(self) -> NonZeroUsize {
        self.0
    }
}

/// Opaque recursive-mutex identity owned by the installed runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MutexHandle(NonZeroUsize);

impl MutexHandle {
    /// Creates a handle after a runtime allocated the corresponding object.
    ///
    /// # Safety
    ///
    /// `raw` must uniquely identify a live mutex until destroy completes.
    pub const unsafe fn from_raw(raw: NonZeroUsize) -> Self {
        Self(raw)
    }

    /// Returns the runtime-owned opaque value.
    pub const fn into_raw(self) -> NonZeroUsize {
        self.0
    }
}

/// A task's scheduling parameters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskConfig {
    /// Requested stack allocation in bytes.
    pub stack_size: NonZeroUsize,
    /// Validated contract-v1 priority. Lower numeric values run first.
    pub priority: TaskPriority,
}

/// Periodic CPU quota for one runtime task, expressed in milliseconds.
///
/// This is an upper bound enforced by a target-backed scheduler. It does not
/// promise that the task receives `capacity_ms` in every period.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskBudget {
    capacity_ms: NonZeroU32,
    replenishment_period_ms: NonZeroU32,
}

impl TaskBudget {
    /// Creates a quota whose capacity does not exceed its replenishment period.
    pub const fn try_new(
        capacity_ms: NonZeroU32,
        replenishment_period_ms: NonZeroU32,
    ) -> Option<Self> {
        if capacity_ms.get() <= replenishment_period_ms.get() {
            Some(Self {
                capacity_ms,
                replenishment_period_ms,
            })
        } else {
            None
        }
    }

    /// Maximum CPU time available in one replenishment period.
    pub const fn capacity_ms(self) -> NonZeroU32 {
        self.capacity_ms
    }

    /// Period at which the CPU quota is replenished.
    pub const fn replenishment_period_ms(self) -> NonZeroU32 {
        self.replenishment_period_ms
    }
}

/// Execution policy assigned atomically when a runtime task is created.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TaskExecutionPolicy {
    /// Runs until it yields, blocks, or exits.
    #[default]
    Cooperative,
    /// Runs cooperatively until its periodic CPU quota is exhausted.
    Budgeted(TaskBudget),
    /// Allows timer-driven equal-priority round-robin preemption.
    Preemptive {
        /// Non-zero time slice in milliseconds.
        time_slice_ms: NonZeroU32,
    },
}

/// A bounded or unbounded wait request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitTimeout {
    /// Return immediately if the resource is unavailable.
    NoWait,
    /// Wait for the given non-zero number of milliseconds.
    Milliseconds(NonZeroU32),
    /// Wait without a deadline.
    Forever,
}

impl WaitTimeout {
    /// Converts the vendor-compatible millisecond convention (`0` no-wait,
    /// `u32::MAX` forever) into the typed contract.
    pub const fn from_millis(milliseconds: u32) -> Self {
        match milliseconds {
            0 => Self::NoWait,
            u32::MAX => Self::Forever,
            value => {
                // SAFETY: the two zero-like sentinel cases were handled above.
                Self::Milliseconds(unsafe { NonZeroU32::new_unchecked(value) })
            }
        }
    }
}

/// Result of a successful wait operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitOutcome {
    /// The resource was acquired.
    Acquired,
    /// The deadline expired before acquisition.
    TimedOut,
}

/// Result of cancelling a task's pending synchronization wait.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitCancellationOutcome {
    /// A queued wait or an unconsumed direct handoff was cancelled.
    Cancelled,
    /// The task was live but had no cancellable wait or pending grant.
    NotWaiting,
}

/// Runtime service failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// No runtime has been installed for this firmware.
    NotInstalled,
    /// A different runtime was already installed.
    AlreadyInstalled,
    /// The runtime's version or capabilities cannot satisfy contract v1.
    IncompatibleContract,
    /// The installed scheduler cannot provide the required execution modes.
    IncompatibleExecutionProfile,
    /// A task, semaphore, stack, or internal control block could not be allocated.
    ResourceExhausted,
    /// The runtime has no dynamic task slot available for another task.
    NoTaskSlots,
    /// The supplied opaque handle is no longer valid.
    InvalidHandle,
    /// The operation is not legal in the current execution context.
    InvalidContext,
    /// A runtime-specific bounded operation timed out.
    TimedOut,
    /// The runtime reported an implementation-specific failure.
    Runtime,
}

/// Scheduler and synchronization capabilities consumed by a radio adapter.
///
/// Implementations must never invoke user callbacks while holding a scheduler
/// lock or with interrupts disabled. [`Runtime::semaphore_up`] must be callable
/// from an interrupt; in that context it may only record readiness and request
/// a deferred schedule. Adapters bracket ISR dispatch with
/// [`Runtime::interrupt_enter`] and [`Runtime::interrupt_exit`] so a backend can
/// distinguish task-context wakeups from interrupt-context wakeups.
pub trait Runtime: Sync {
    /// Describes the versioned semantics implemented by this backend.
    fn contract(&self) -> RuntimeContract;

    /// Describes scheduling modes backed by the installed target resources.
    fn execution_profile(&self) -> RuntimeExecutionProfile;

    /// Returns an advisory snapshot of dynamic task capacity.
    ///
    /// Backends advertising [`RuntimeCapabilities::TASK_CAPACITY_QUERY`] must
    /// override this method. The default preserves source compatibility for
    /// older v1.1 backends.
    fn task_capacity(&self) -> Result<TaskCapacity, Error> {
        Err(Error::IncompatibleContract)
    }

    /// Atomically reserves dynamic task slots for one subsystem owner.
    fn reserve_tasks(
        &self,
        _required: NonZeroUsize,
    ) -> Result<TaskReservation, TaskAdmissionError> {
        Err(TaskAdmissionError::Runtime(Error::IncompatibleContract))
    }

    /// Atomically reserves dynamic task slots and their stack allocations.
    fn reserve_task_resources(
        &self,
        _required: TaskResourceRequirements,
    ) -> Result<TaskReservation, TaskAdmissionError> {
        Err(TaskAdmissionError::Runtime(Error::IncompatibleContract))
    }

    /// Atomically reserves every independently owned group in one plan.
    ///
    /// No reservation may remain live when this method returns an error.
    fn reserve_task_resource_plan(
        &self,
        _plan: TaskResourcePlan<'_>,
    ) -> Result<TaskReservationBatch, TaskAdmissionError> {
        Err(TaskAdmissionError::Runtime(Error::IncompatibleContract))
    }

    /// Releases the still-unconsumed slots held by a reservation.
    fn release_task_reservation(&self, _reservation: &TaskReservation) -> Result<(), Error> {
        Err(Error::IncompatibleContract)
    }

    /// Spawns one task while consuming a slot from `reservation`.
    fn spawn_reserved(
        &self,
        _reservation: &TaskReservation,
        _entry: TaskEntry,
        _arg: *mut c_void,
        _config: TaskConfig,
    ) -> Result<TaskId, Error> {
        Err(Error::IncompatibleContract)
    }

    /// Spawns a reserved task with its execution policy installed before it
    /// becomes eligible to run.
    fn spawn_reserved_scheduled(
        &self,
        _reservation: &TaskReservation,
        _entry: TaskEntry,
        _arg: *mut c_void,
        _config: TaskConfig,
        _policy: TaskExecutionPolicy,
    ) -> Result<TaskId, Error> {
        Err(Error::IncompatibleContract)
    }

    /// Spawns one task.
    fn spawn(
        &self,
        entry: TaskEntry,
        arg: *mut c_void,
        config: TaskConfig,
    ) -> Result<TaskId, Error>;

    /// Spawns a task with its execution policy installed before it becomes
    /// eligible to run.
    fn spawn_scheduled(
        &self,
        _entry: TaskEntry,
        _arg: *mut c_void,
        _config: TaskConfig,
        _policy: TaskExecutionPolicy,
    ) -> Result<TaskId, Error> {
        Err(Error::IncompatibleContract)
    }

    /// Makes another ready task eligible to run.
    fn yield_now(&self) -> Result<(), Error>;

    /// Blocks the current task for at least `milliseconds`.
    fn sleep_ms(&self, milliseconds: NonZeroU32) -> Result<(), Error>;

    /// Returns the current task identity.
    fn current_task(&self) -> Result<TaskId, Error>;

    /// Changes a live task's runtime-defined scheduling priority.
    fn set_task_priority(&self, task: TaskId, priority: TaskPriority) -> Result<(), Error>;

    /// Cancels a task's queued wait or unconsumed direct resource handoff.
    ///
    /// Cancelling a direct semaphore handoff returns the count to the
    /// semaphore. Cancelling a direct mutex handoff releases that ownership and
    /// hands it to the next waiter, if any.
    fn cancel_wait(&self, task: TaskId) -> Result<WaitCancellationOutcome, Error>;

    /// Prevents scheduler-driven preemption of the current task. Calls nest.
    fn lock_scheduler(&self) -> Result<(), Error>;

    /// Releases one scheduler-lock nesting level for the current task.
    fn unlock_scheduler(&self) -> Result<(), Error>;

    /// Marks entry into an interrupt handler that may call runtime services.
    fn interrupt_enter(&self) -> Result<(), Error> {
        Ok(())
    }

    /// Marks exit from an interrupt handler that may call runtime services.
    fn interrupt_exit(&self) -> Result<(), Error> {
        Ok(())
    }

    /// Allocates a counting semaphore.
    fn semaphore_create(&self, initial: u32) -> Result<SemaphoreHandle, Error>;

    /// Waits for one semaphore count.
    fn semaphore_down(
        &self,
        semaphore: SemaphoreHandle,
        timeout: WaitTimeout,
    ) -> Result<WaitOutcome, Error>;

    /// Adds one count or wakes one waiter. This operation must be ISR-safe.
    fn semaphore_up(&self, semaphore: SemaphoreHandle) -> Result<(), Error>;

    /// Destroys a semaphore.
    ///
    /// # Safety
    ///
    /// The caller must prove that no task or interrupt can use `semaphore`
    /// during or after this call.
    unsafe fn semaphore_destroy(&self, semaphore: SemaphoreHandle) -> Result<(), Error>;

    /// Allocates a recursive priority-inheritance mutex.
    fn mutex_create(&self) -> Result<MutexHandle, Error>;

    /// Acquires a mutex recursively or waits according to `timeout`.
    fn mutex_lock(&self, mutex: MutexHandle, timeout: WaitTimeout) -> Result<WaitOutcome, Error>;

    /// Releases one recursion level. Only the owning task may unlock.
    fn mutex_unlock(&self, mutex: MutexHandle) -> Result<(), Error>;

    /// Destroys a mutex.
    ///
    /// # Safety
    ///
    /// No owner, waiter, task, or interrupt may use `mutex` during or after
    /// this call.
    unsafe fn mutex_destroy(&self, mutex: MutexHandle) -> Result<(), Error>;
}

static RUNTIME: Mutex<Cell<Option<&'static dyn Runtime>>> = Mutex::new(Cell::new(None));

fn validate_contract(contract: RuntimeContract) -> Result<(), Error> {
    if contract.satisfies(RuntimeContract::V1) {
        Ok(())
    } else {
        Err(Error::IncompatibleContract)
    }
}

fn validate_execution_profile(profile: RuntimeExecutionProfile) -> Result<(), Error> {
    let known_modes = RuntimeExecutionModes::PORTLESS_COOPERATIVE
        | RuntimeExecutionModes::PORTED_COOPERATIVE
        | RuntimeExecutionModes::BUDGETED
        | RuntimeExecutionModes::PREEMPTIVE;
    if profile.revision == 1 && profile.modes.bits() != 0 && known_modes.contains(profile.modes) {
        Ok(())
    } else {
        Err(Error::IncompatibleExecutionProfile)
    }
}

/// Installs the firmware's sole runtime implementation.
///
/// Reinstalling the same static implementation is idempotent. Installing a
/// different implementation fails, so two radio/runtime stacks cannot silently
/// compete for the same scheduler resources.
pub fn install(runtime: &'static dyn Runtime) -> Result<(), Error> {
    validate_contract(runtime.contract())?;
    validate_execution_profile(runtime.execution_profile())?;
    critical_section::with(|cs| match RUNTIME.borrow(cs).get() {
        None => {
            RUNTIME.borrow(cs).set(Some(runtime));
            Ok(())
        }
        Some(current) if core::ptr::eq(current, runtime) => Ok(()),
        Some(_) => Err(Error::AlreadyInstalled),
    })
}

/// Returns the installed runtime's versioned contract.
pub fn runtime_contract() -> Result<RuntimeContract, Error> {
    with_runtime(|runtime| Ok(runtime.contract()))
}

/// Returns scheduling guarantees backed by the installed target resources.
pub fn runtime_execution_profile() -> Result<RuntimeExecutionProfile, Error> {
    with_runtime(|runtime| Ok(runtime.execution_profile()))
}

/// Returns an advisory snapshot of the installed runtime's dynamic task slots.
pub fn task_capacity() -> Result<TaskCapacity, Error> {
    with_runtime(|runtime| {
        if !runtime
            .contract()
            .capabilities
            .contains(RuntimeCapabilities::TASK_CAPACITY_QUERY)
        {
            return Err(Error::IncompatibleContract);
        }
        runtime.task_capacity()
    })
}

/// Checks whether a capacity snapshot can accommodate `required` tasks.
///
/// This catches deterministic under-provisioning before radio state is
/// consumed. It does not reserve the observed slots; callers requiring atomic
/// admission must use [`reserve_task_capacity`] or [`reserve_task_resources`].
pub fn require_task_capacity(required: usize) -> Result<TaskCapacity, TaskAdmissionError> {
    let snapshot = task_capacity().map_err(TaskAdmissionError::Runtime)?;
    let available = snapshot.dynamic_available();
    if available < required {
        return Err(TaskAdmissionError::InsufficientTaskSlots {
            required,
            available,
        });
    }
    Ok(snapshot)
}

/// Atomically reserves `required` dynamic slots before radio initialization.
pub fn reserve_task_capacity(
    required: NonZeroUsize,
) -> Result<TaskReservation, TaskAdmissionError> {
    with_runtime_admission(|runtime| {
        if !runtime
            .contract()
            .capabilities
            .contains(RuntimeCapabilities::TASK_RESERVATION)
        {
            return Err(TaskAdmissionError::Runtime(Error::IncompatibleContract));
        }
        runtime.reserve_tasks(required)
    })
}

/// Atomically reserves dynamic task slots and one stack allocation per slot.
pub fn reserve_task_resources(
    required: TaskResourceRequirements,
) -> Result<TaskReservation, TaskAdmissionError> {
    with_runtime_admission(|runtime| {
        if !runtime
            .contract()
            .capabilities
            .contains(RuntimeCapabilities::TASK_STACK_RESERVATION)
        {
            return Err(TaskAdmissionError::Runtime(Error::IncompatibleContract));
        }
        runtime.reserve_task_resources(required)
    })
}

/// Atomically reserves all independently owned groups in `plan`.
///
/// Returned child reservations follow [`TaskResourcePlan::groups`] order. An
/// error guarantees that the runtime rolled back every slot and stack.
pub fn reserve_task_resource_plan(
    plan: TaskResourcePlan<'_>,
) -> Result<TaskReservationBatch, TaskAdmissionError> {
    with_runtime_admission(|runtime| {
        if !runtime
            .contract()
            .capabilities
            .contains(RuntimeCapabilities::TASK_RESOURCE_PLAN_RESERVATION)
        {
            return Err(TaskAdmissionError::Runtime(Error::IncompatibleContract));
        }
        runtime.reserve_task_resource_plan(plan)
    })
}

/// Releases the unconsumed portion of a task reservation.
pub fn release_task_reservation(reservation: &TaskReservation) -> Result<(), Error> {
    with_runtime(|runtime| runtime.release_task_reservation(reservation))
}

/// Proves that the installed backend satisfies an adapter's requirements.
///
/// Radio adapters should call this before allocating tasks or publishing
/// callbacks. A mismatch fails before partial initialization begins.
pub fn require_runtime_contract(required: RuntimeContract) -> Result<RuntimeContract, Error> {
    with_runtime(|runtime| {
        let offered = runtime.contract();
        if offered.satisfies(required) {
            Ok(offered)
        } else {
            Err(Error::IncompatibleContract)
        }
    })
}

/// Cancels a live task's queued synchronization wait or pending direct grant.
pub fn cancel_wait(task: TaskId) -> Result<WaitCancellationOutcome, Error> {
    with_runtime(|runtime| runtime.cancel_wait(task))
}

/// Proves both semantic capabilities and executable scheduling guarantees.
pub fn require_runtime(required: RuntimeRequirements) -> Result<RuntimeRequirements, Error> {
    with_runtime(|runtime| {
        let contract = runtime.contract();
        if !contract.satisfies(required.contract) {
            return Err(Error::IncompatibleContract);
        }
        let execution_profile = runtime.execution_profile();
        if !execution_profile.satisfies(required.execution_profile) {
            return Err(Error::IncompatibleExecutionProfile);
        }
        Ok(RuntimeRequirements {
            contract,
            execution_profile,
        })
    })
}

fn with_runtime<T>(operation: impl FnOnce(&dyn Runtime) -> Result<T, Error>) -> Result<T, Error> {
    let runtime = critical_section::with(|cs| RUNTIME.borrow(cs).get());
    operation(runtime.ok_or(Error::NotInstalled)?)
}

fn with_runtime_admission<T>(
    operation: impl FnOnce(&dyn Runtime) -> Result<T, TaskAdmissionError>,
) -> Result<T, TaskAdmissionError> {
    let runtime = critical_section::with(|cs| RUNTIME.borrow(cs).get())
        .ok_or(TaskAdmissionError::Runtime(Error::NotInstalled))?;
    operation(runtime)
}

/// Spawns a task through the installed runtime.
pub fn spawn(entry: TaskEntry, arg: *mut c_void, config: TaskConfig) -> Result<TaskId, Error> {
    with_runtime(|runtime| runtime.spawn(entry, arg, config))
}

/// Spawns a task with an explicit execution policy assigned atomically.
pub fn spawn_scheduled(
    entry: TaskEntry,
    arg: *mut c_void,
    config: TaskConfig,
    policy: TaskExecutionPolicy,
) -> Result<TaskId, Error> {
    with_runtime(|runtime| runtime.spawn_scheduled(entry, arg, config, policy))
}

/// Spawns a task while consuming one slot from an owner-bound reservation.
pub fn spawn_reserved(
    reservation: &TaskReservation,
    entry: TaskEntry,
    arg: *mut c_void,
    config: TaskConfig,
) -> Result<TaskId, Error> {
    with_runtime(|runtime| runtime.spawn_reserved(reservation, entry, arg, config))
}

/// Spawns a reserved task with an explicit execution policy assigned atomically.
pub fn spawn_reserved_scheduled(
    reservation: &TaskReservation,
    entry: TaskEntry,
    arg: *mut c_void,
    config: TaskConfig,
    policy: TaskExecutionPolicy,
) -> Result<TaskId, Error> {
    with_runtime(|runtime| {
        runtime.spawn_reserved_scheduled(reservation, entry, arg, config, policy)
    })
}

/// Yields through the installed runtime.
pub fn yield_now() -> Result<(), Error> {
    with_runtime(|runtime| runtime.yield_now())
}

/// Sleeps through the installed runtime. A zero duration is represented by
/// [`yield_now`] instead of an invalid sleep request.
pub fn sleep_ms(milliseconds: NonZeroU32) -> Result<(), Error> {
    with_runtime(|runtime| runtime.sleep_ms(milliseconds))
}

/// Returns the current task identity.
pub fn current_task() -> Result<TaskId, Error> {
    with_runtime(|runtime| runtime.current_task())
}

/// Changes a task's scheduling priority through the installed runtime.
pub fn set_task_priority(task: TaskId, priority: TaskPriority) -> Result<(), Error> {
    with_runtime(|runtime| runtime.set_task_priority(task, priority))
}

/// Prevents scheduler-driven preemption of the current task.
pub fn lock_scheduler() -> Result<(), Error> {
    with_runtime(|runtime| runtime.lock_scheduler())
}

/// Releases one scheduler-lock nesting level for the current task.
pub fn unlock_scheduler() -> Result<(), Error> {
    with_runtime(|runtime| runtime.unlock_scheduler())
}

/// Marks entry into an interrupt handler through the installed runtime.
pub fn interrupt_enter() -> Result<(), Error> {
    with_runtime(|runtime| runtime.interrupt_enter())
}

/// Marks exit from an interrupt handler through the installed runtime.
pub fn interrupt_exit() -> Result<(), Error> {
    with_runtime(|runtime| runtime.interrupt_exit())
}

/// Allocates a semaphore through the installed runtime.
pub fn semaphore_create(initial: u32) -> Result<SemaphoreHandle, Error> {
    with_runtime(|runtime| runtime.semaphore_create(initial))
}

/// Waits on a semaphore through the installed runtime.
pub fn semaphore_down(
    semaphore: SemaphoreHandle,
    timeout: WaitTimeout,
) -> Result<WaitOutcome, Error> {
    with_runtime(|runtime| runtime.semaphore_down(semaphore, timeout))
}

/// Releases a semaphore through the installed runtime.
pub fn semaphore_up(semaphore: SemaphoreHandle) -> Result<(), Error> {
    with_runtime(|runtime| runtime.semaphore_up(semaphore))
}

/// Destroys a semaphore through the installed runtime.
///
/// # Safety
///
/// See [`Runtime::semaphore_destroy`].
pub unsafe fn semaphore_destroy(semaphore: SemaphoreHandle) -> Result<(), Error> {
    with_runtime(|runtime| unsafe { runtime.semaphore_destroy(semaphore) })
}

/// Allocates a recursive priority-inheritance mutex.
pub fn mutex_create() -> Result<MutexHandle, Error> {
    with_runtime(|runtime| runtime.mutex_create())
}

/// Acquires a mutex through the installed runtime.
pub fn mutex_lock(mutex: MutexHandle, timeout: WaitTimeout) -> Result<WaitOutcome, Error> {
    with_runtime(|runtime| runtime.mutex_lock(mutex, timeout))
}

/// Releases one mutex recursion level through the installed runtime.
pub fn mutex_unlock(mutex: MutexHandle) -> Result<(), Error> {
    with_runtime(|runtime| runtime.mutex_unlock(mutex))
}

/// Destroys a mutex through the installed runtime.
///
/// # Safety
///
/// See [`Runtime::mutex_destroy`].
pub unsafe fn mutex_destroy(mutex: MutexHandle) -> Result<(), Error> {
    with_runtime(|runtime| unsafe { runtime.mutex_destroy(mutex) })
}

/// Runtime-neutral counting semaphore suitable for static or embedded use.
///
/// The backend object is created lazily in normal context. Allocation happens
/// outside the critical section; only publishing the opaque handle is atomic.
/// Call [`Semaphore::try_init`] before an interrupt can first use the object.
pub struct Semaphore {
    initial: u32,
    handle: Mutex<Cell<Option<SemaphoreHandle>>>,
}

// SAFETY: the only local mutable state is serialized by critical-section; the
// installed Runtime owns and synchronizes the backend object.
unsafe impl Sync for Semaphore {}

impl Semaphore {
    /// Creates an uninitialized semaphore descriptor.
    pub const fn new(initial: u32) -> Self {
        Self {
            initial,
            handle: Mutex::new(Cell::new(None)),
        }
    }

    /// Ensures the backend semaphore exists and returns its opaque handle.
    ///
    /// This may allocate and therefore must not be called for the first time
    /// from an interrupt or critical section.
    pub fn try_init(&self) -> Result<SemaphoreHandle, Error> {
        if let Some(handle) = critical_section::with(|cs| self.handle.borrow(cs).get()) {
            return Ok(handle);
        }

        let candidate = semaphore_create(self.initial)?;
        let selected = critical_section::with(|cs| {
            let slot = self.handle.borrow(cs);
            if let Some(handle) = slot.get() {
                handle
            } else {
                slot.set(Some(candidate));
                candidate
            }
        });

        if selected != candidate {
            // SAFETY: the candidate was never published and no other context
            // can have observed it.
            unsafe { semaphore_destroy(candidate)? };
        }
        Ok(selected)
    }

    /// Waits for one count.
    pub fn down(&self) -> Result<(), Error> {
        match semaphore_down(self.try_init()?, WaitTimeout::Forever)? {
            WaitOutcome::Acquired => Ok(()),
            WaitOutcome::TimedOut => Err(Error::TimedOut),
        }
    }

    /// Waits for one count until `timeout` expires.
    pub fn down_timeout(&self, timeout: WaitTimeout) -> Result<WaitOutcome, Error> {
        semaphore_down(self.try_init()?, timeout)
    }

    /// Adds one count or wakes one waiter.
    ///
    /// Call [`try_init`](Self::try_init) in normal context before this method is
    /// reachable from an interrupt.
    pub fn up(&self) -> Result<(), Error> {
        semaphore_up(self.try_init()?)
    }

    /// Destroys the backend semaphore.
    ///
    /// # Safety
    ///
    /// No task or interrupt may access this object during or after destruction.
    pub unsafe fn destroy(&self) -> Result<(), Error> {
        let handle = critical_section::with(|cs| self.handle.borrow(cs).take());
        if let Some(handle) = handle {
            unsafe { semaphore_destroy(handle) }?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use core::sync::atomic::{AtomicU32, Ordering};

    struct TestCriticalSection;

    critical_section::set_impl!(TestCriticalSection);

    unsafe impl critical_section::Impl for TestCriticalSection {
        unsafe fn acquire() -> critical_section::RawRestoreState {
            // SAFETY: the host test implementation never inspects restore state.
            unsafe { core::mem::zeroed() }
        }
        unsafe fn release(_: critical_section::RawRestoreState) {}
    }

    struct TestRuntime(AtomicU32);

    impl Runtime for TestRuntime {
        fn contract(&self) -> RuntimeContract {
            RuntimeContract::V1_3
        }

        fn execution_profile(&self) -> RuntimeExecutionProfile {
            RuntimeExecutionProfile::V1_PORTED
        }

        fn task_capacity(&self) -> Result<TaskCapacity, Error> {
            Ok(TaskCapacity::new(15, 2).unwrap())
        }

        fn reserve_tasks(
            &self,
            required: NonZeroUsize,
        ) -> Result<TaskReservation, TaskAdmissionError> {
            if required.get() > 13 {
                return Err(TaskAdmissionError::InsufficientTaskSlots {
                    required: required.get(),
                    available: 13,
                });
            }
            Ok(unsafe { TaskReservation::from_raw(NonZeroU32::new(1).unwrap()) })
        }

        fn release_task_reservation(&self, _reservation: &TaskReservation) -> Result<(), Error> {
            Ok(())
        }

        fn spawn_reserved(
            &self,
            _reservation: &TaskReservation,
            entry: TaskEntry,
            arg: *mut c_void,
            config: TaskConfig,
        ) -> Result<TaskId, Error> {
            self.spawn(entry, arg, config)
        }

        fn spawn(
            &self,
            _entry: TaskEntry,
            _arg: *mut c_void,
            _config: TaskConfig,
        ) -> Result<TaskId, Error> {
            Ok(TaskId::from_raw(self.0.fetch_add(1, Ordering::Relaxed)))
        }

        fn yield_now(&self) -> Result<(), Error> {
            Ok(())
        }

        fn sleep_ms(&self, _milliseconds: NonZeroU32) -> Result<(), Error> {
            Ok(())
        }

        fn current_task(&self) -> Result<TaskId, Error> {
            Ok(TaskId::from_raw(7))
        }

        fn set_task_priority(&self, _task: TaskId, _priority: TaskPriority) -> Result<(), Error> {
            Ok(())
        }

        fn cancel_wait(&self, _task: TaskId) -> Result<WaitCancellationOutcome, Error> {
            Ok(WaitCancellationOutcome::NotWaiting)
        }

        fn lock_scheduler(&self) -> Result<(), Error> {
            Ok(())
        }

        fn unlock_scheduler(&self) -> Result<(), Error> {
            Ok(())
        }

        fn semaphore_create(&self, _initial: u32) -> Result<SemaphoreHandle, Error> {
            Ok(unsafe { SemaphoreHandle::from_raw(NonZeroUsize::new(1).unwrap()) })
        }

        fn semaphore_down(
            &self,
            _semaphore: SemaphoreHandle,
            _timeout: WaitTimeout,
        ) -> Result<WaitOutcome, Error> {
            Ok(WaitOutcome::Acquired)
        }

        fn semaphore_up(&self, _semaphore: SemaphoreHandle) -> Result<(), Error> {
            Ok(())
        }

        unsafe fn semaphore_destroy(&self, _semaphore: SemaphoreHandle) -> Result<(), Error> {
            Ok(())
        }

        fn mutex_create(&self) -> Result<MutexHandle, Error> {
            Ok(unsafe { MutexHandle::from_raw(NonZeroUsize::new(2).unwrap()) })
        }

        fn mutex_lock(
            &self,
            _mutex: MutexHandle,
            _timeout: WaitTimeout,
        ) -> Result<WaitOutcome, Error> {
            Ok(WaitOutcome::Acquired)
        }

        fn mutex_unlock(&self, _mutex: MutexHandle) -> Result<(), Error> {
            Ok(())
        }

        unsafe fn mutex_destroy(&self, _mutex: MutexHandle) -> Result<(), Error> {
            Ok(())
        }
    }

    static RUNTIME_A: TestRuntime = TestRuntime(AtomicU32::new(1));
    static RUNTIME_B: TestRuntime = TestRuntime(AtomicU32::new(1));

    extern "C" fn task(_arg: *mut c_void) -> *mut c_void {
        core::ptr::null_mut()
    }

    #[test]
    fn installs_exactly_one_runtime_and_dispatches() {
        install(&RUNTIME_A).unwrap();
        install(&RUNTIME_A).unwrap();
        assert_eq!(install(&RUNTIME_B), Err(Error::AlreadyInstalled));

        let id = spawn(
            task,
            core::ptr::null_mut(),
            TaskConfig {
                stack_size: NonZeroUsize::new(1024).unwrap(),
                priority: TaskPriority::new(3).unwrap(),
            },
        )
        .unwrap();
        assert_eq!(id.into_raw(), 1);
        assert_eq!(
            spawn_scheduled(
                task,
                core::ptr::null_mut(),
                TaskConfig {
                    stack_size: NonZeroUsize::new(1024).unwrap(),
                    priority: TaskPriority::new(3).unwrap(),
                },
                TaskExecutionPolicy::Cooperative,
            ),
            Err(Error::IncompatibleContract)
        );
        assert_eq!(
            cancel_wait(id).unwrap(),
            WaitCancellationOutcome::NotWaiting
        );
        assert_eq!(current_task().unwrap().into_raw(), 7);
        set_task_priority(id, TaskPriority::new(2).unwrap()).unwrap();
        lock_scheduler().unwrap();
        unlock_scheduler().unwrap();

        static SEMAPHORE: Semaphore = Semaphore::new(1);
        SEMAPHORE.down().unwrap();
        SEMAPHORE.up().unwrap();
        assert_eq!(
            SEMAPHORE.down_timeout(WaitTimeout::NoWait).unwrap(),
            WaitOutcome::Acquired
        );

        let mutex = mutex_create().unwrap();
        assert_eq!(
            mutex_lock(mutex, WaitTimeout::Forever).unwrap(),
            WaitOutcome::Acquired
        );
        mutex_unlock(mutex).unwrap();
        unsafe { mutex_destroy(mutex).unwrap() };
        assert_eq!(runtime_contract().unwrap(), RuntimeContract::V1_3);
        assert_eq!(
            runtime_execution_profile().unwrap(),
            RuntimeExecutionProfile::V1_PORTED
        );
        assert_eq!(
            require_runtime_contract(RuntimeContract::V1).unwrap(),
            RuntimeContract::V1_3
        );
        assert_eq!(
            require_runtime(RuntimeRequirements::V1_PORTED_COOPERATIVE).unwrap(),
            RuntimeRequirements {
                contract: RuntimeContract::V1_3,
                execution_profile: RuntimeExecutionProfile::V1_PORTED,
            }
        );
        assert_eq!(task_capacity().unwrap().dynamic_capacity(), 15);
        assert_eq!(task_capacity().unwrap().dynamic_used(), 2);
        assert_eq!(task_capacity().unwrap().dynamic_available(), 13);
        assert_eq!(require_task_capacity(13), Ok(task_capacity().unwrap()));
        assert_eq!(
            require_task_capacity(14),
            Err(TaskAdmissionError::InsufficientTaskSlots {
                required: 14,
                available: 13,
            })
        );
        let reservation = reserve_task_capacity(NonZeroUsize::new(3).unwrap()).unwrap();
        assert_eq!(reservation.into_raw(), NonZeroU32::new(1).unwrap());
        let reserved_task = spawn_reserved(
            &reservation,
            task,
            core::ptr::null_mut(),
            TaskConfig {
                stack_size: NonZeroUsize::new(1024).unwrap(),
                priority: TaskPriority::new(3).unwrap(),
            },
        )
        .unwrap();
        assert_eq!(reserved_task.into_raw(), 2);
        release_task_reservation(&reservation).unwrap();
    }

    #[test]
    fn contract_validation_rejects_missing_capabilities() {
        let incomplete = RuntimeContract {
            version: RuntimeContractVersion::V1_0,
            capabilities: RuntimeCapabilities::TASKS,
        };
        assert_eq!(
            validate_contract(incomplete),
            Err(Error::IncompatibleContract)
        );
        assert_eq!(
            validate_execution_profile(RuntimeExecutionProfile {
                revision: 1,
                modes: RuntimeExecutionModes::NONE,
            }),
            Err(Error::IncompatibleExecutionProfile)
        );
    }

    #[test]
    fn task_capacity_rejects_invalid_snapshots() {
        assert_eq!(TaskCapacity::new(2, 3), None);
        let empty = TaskCapacity::new(0, 0).unwrap();
        assert_eq!(empty.dynamic_available(), 0);
        let reserved = TaskCapacity::new_with_reserved(15, 2, 5).unwrap();
        assert_eq!(reserved.dynamic_reserved(), 5);
        assert_eq!(reserved.dynamic_available(), 8);
        assert_eq!(TaskCapacity::new_with_reserved(15, 12, 4), None);
    }

    #[test]
    fn task_budget_rejects_capacity_larger_than_period() {
        let ten = NonZeroU32::new(10).unwrap();
        let twenty = NonZeroU32::new(20).unwrap();
        let budget = TaskBudget::try_new(ten, twenty).unwrap();
        assert_eq!(budget.capacity_ms(), ten);
        assert_eq!(budget.replenishment_period_ms(), twenty);
        assert_eq!(TaskBudget::try_new(twenty, ten), None);
    }

    #[test]
    fn task_resource_requirements_reject_overflow_and_report_exact_bytes() {
        let requirements = TaskResourceRequirements::new(
            NonZeroUsize::new(6).unwrap(),
            NonZeroUsize::new(24 * 1024).unwrap(),
        )
        .unwrap();
        assert_eq!(requirements.task_slots().get(), 6);
        assert_eq!(requirements.stack_bytes_per_task().get(), 24 * 1024);
        assert_eq!(requirements.total_stack_bytes(), 144 * 1024);
        assert_eq!(
            TaskResourceRequirements::new(
                NonZeroUsize::new(usize::MAX).unwrap(),
                NonZeroUsize::new(2).unwrap(),
            ),
            None
        );
    }

    #[test]
    fn heterogeneous_resource_plan_derives_totals_and_rejects_bad_shapes() {
        let vendor = TaskResourceGroupRequirements::new(
            TaskResourceOwner::new(NonZeroU32::new(1).unwrap()),
            TaskResourceRequirements::new(
                NonZeroUsize::new(7).unwrap(),
                NonZeroUsize::new(24 * 1024).unwrap(),
            )
            .unwrap(),
        );
        let worker = TaskResourceGroupRequirements::new(
            TaskResourceOwner::new(NonZeroU32::new(2).unwrap()),
            TaskResourceRequirements::new(
                NonZeroUsize::new(1).unwrap(),
                NonZeroUsize::new(8 * 1024).unwrap(),
            )
            .unwrap(),
        );
        let groups = [vendor, worker];
        let plan = TaskResourcePlan::new(&groups).unwrap();
        assert_eq!(plan.total_task_slots(), 8);
        assert_eq!(plan.total_stack_bytes(), 7 * 24 * 1024 + 8 * 1024);
        assert_eq!(plan.groups(), &groups);

        assert_eq!(TaskResourcePlan::new(&[]), None);
        assert_eq!(TaskResourcePlan::new(&[vendor, vendor]), None);
        assert_eq!(
            TaskResourcePlan::new(&[vendor, worker, vendor, worker, vendor]),
            None
        );
    }

    #[test]
    fn reservation_batch_transfers_each_child_once() {
        let first = unsafe { TaskReservation::from_raw(NonZeroU32::new(0x101).unwrap()) };
        let second = unsafe { TaskReservation::from_raw(NonZeroU32::new(0x102).unwrap()) };
        let mut reservations = [const { None }; TASK_RESOURCE_GROUP_CAPACITY];
        reservations[0] = Some(first);
        reservations[1] = Some(second);
        let mut batch = unsafe { TaskReservationBatch::from_reservations(reservations, 2) };
        assert_eq!(batch.len(), 2);
        assert_eq!(batch.remaining(), 2);
        assert_eq!(batch.take(0).unwrap().into_raw().get(), 0x101);
        assert_eq!(batch.remaining(), 1);
        assert!(batch.take(0).is_none());
        assert_eq!(batch.take(1).unwrap().into_raw().get(), 0x102);
        assert_eq!(batch.remaining(), 0);
    }
}
