//! Runtime-neutral services required by HiSilicon radio firmware.
//!
//! This crate is the narrow dependency shared by radio adapters and an RTOS.
//! It does not implement a scheduler and does not know about a chip, protocol,
//! vendor blob, allocator, or network stack. A firmware installs exactly one
//! [`Runtime`] before initializing its radio controller.

#![no_std]

use core::cell::Cell;
use core::ffi::c_void;
use core::num::{NonZeroU32, NonZeroUsize};
use critical_section::Mutex;

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

/// A task's scheduling parameters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskConfig {
    /// Requested stack allocation in bytes.
    pub stack_size: NonZeroUsize,
    /// Runtime-defined priority. Larger/smaller ordering is documented by the
    /// selected runtime rather than guessed by this contract.
    pub priority: u8,
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

/// Runtime service failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// No runtime has been installed for this firmware.
    NotInstalled,
    /// A different runtime was already installed.
    AlreadyInstalled,
    /// A task, semaphore, stack, or internal control block could not be allocated.
    ResourceExhausted,
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
/// lock or with interrupts disabled. [`Runtime::semaphore_up`] must be bounded
/// and callable from an interrupt; it may only record readiness and request a
/// deferred schedule.
pub trait Runtime: Sync {
    /// Spawns one task.
    fn spawn(
        &self,
        entry: TaskEntry,
        arg: *mut c_void,
        config: TaskConfig,
    ) -> Result<TaskId, Error>;

    /// Makes another ready task eligible to run.
    fn yield_now(&self) -> Result<(), Error>;

    /// Blocks the current task for at least `milliseconds`.
    fn sleep_ms(&self, milliseconds: NonZeroU32) -> Result<(), Error>;

    /// Returns the current task identity.
    fn current_task(&self) -> Result<TaskId, Error>;

    /// Changes a live task's runtime-defined scheduling priority.
    fn set_task_priority(&self, task: TaskId, priority: u8) -> Result<(), Error>;

    /// Prevents scheduler-driven preemption of the current task. Calls nest.
    fn lock_scheduler(&self) -> Result<(), Error>;

    /// Releases one scheduler-lock nesting level for the current task.
    fn unlock_scheduler(&self) -> Result<(), Error>;

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
}

static RUNTIME: Mutex<Cell<Option<&'static dyn Runtime>>> = Mutex::new(Cell::new(None));

/// Installs the firmware's sole runtime implementation.
///
/// Reinstalling the same static implementation is idempotent. Installing a
/// different implementation fails, so two radio/runtime stacks cannot silently
/// compete for the same scheduler resources.
pub fn install(runtime: &'static dyn Runtime) -> Result<(), Error> {
    critical_section::with(|cs| match RUNTIME.borrow(cs).get() {
        None => {
            RUNTIME.borrow(cs).set(Some(runtime));
            Ok(())
        }
        Some(current) if core::ptr::eq(current, runtime) => Ok(()),
        Some(_) => Err(Error::AlreadyInstalled),
    })
}

fn with_runtime<T>(operation: impl FnOnce(&dyn Runtime) -> Result<T, Error>) -> Result<T, Error> {
    let runtime = critical_section::with(|cs| RUNTIME.borrow(cs).get());
    operation(runtime.ok_or(Error::NotInstalled)?)
}

/// Spawns a task through the installed runtime.
pub fn spawn(entry: TaskEntry, arg: *mut c_void, config: TaskConfig) -> Result<TaskId, Error> {
    with_runtime(|runtime| runtime.spawn(entry, arg, config))
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
pub fn set_task_priority(task: TaskId, priority: u8) -> Result<(), Error> {
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
        unsafe fn acquire() -> critical_section::RawRestoreState {}
        unsafe fn release(_: critical_section::RawRestoreState) {}
    }

    struct TestRuntime(AtomicU32);

    impl Runtime for TestRuntime {
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

        fn set_task_priority(&self, _task: TaskId, _priority: u8) -> Result<(), Error> {
            Ok(())
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
                priority: 3,
            },
        )
        .unwrap();
        assert_eq!(id.into_raw(), 1);
        assert_eq!(current_task().unwrap().into_raw(), 7);
        set_task_priority(id, 2).unwrap();
        lock_scheduler().unwrap();
        unlock_scheduler().unwrap();

        static SEMAPHORE: Semaphore = Semaphore::new(1);
        SEMAPHORE.down().unwrap();
        SEMAPHORE.up().unwrap();
        assert_eq!(
            SEMAPHORE.down_timeout(WaitTimeout::NoWait).unwrap(),
            WaitOutcome::Acquired
        );
    }
}
