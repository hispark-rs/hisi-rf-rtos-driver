use core::ops::{BitOr, BitOrAssign};

/// Number of priority levels required by runtime contract v1.
pub const TASK_PRIORITY_LEVELS: u8 = 32;

/// A validated runtime task priority.
///
/// Contract v1 has 32 levels. Lower numeric values outrank higher values;
/// priority 0 is highest and priority 31 is lowest. Chip adapters must perform
/// any vendor-specific numeric mapping before constructing this value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct TaskPriority(u8);

/// A numeric priority outside the contract-v1 range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidTaskPriority {
    value: u8,
}

impl InvalidTaskPriority {
    /// Returns the rejected numeric value.
    pub const fn value(self) -> u8 {
        self.value
    }
}

impl TaskPriority {
    /// Highest priority accepted by contract v1.
    pub const HIGHEST: Self = Self(0);
    /// Lowest priority accepted by contract v1.
    pub const LOWEST: Self = Self(TASK_PRIORITY_LEVELS - 1);

    /// Validates one contract-v1 priority value.
    pub const fn new(value: u8) -> Option<Self> {
        if value < TASK_PRIORITY_LEVELS {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Returns the contract-v1 numeric value.
    pub const fn into_raw(self) -> u8 {
        self.0
    }

    /// Returns true when this priority must run before `other` when both are
    /// eligible. Equal priorities are ordered by the runtime's FIFO policy.
    pub const fn outranks(self, other: Self) -> bool {
        self.0 < other.0
    }
}

impl TryFrom<u8> for TaskPriority {
    type Error = InvalidTaskPriority;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::new(value).ok_or(InvalidTaskPriority { value })
    }
}

impl From<TaskPriority> for u8 {
    fn from(value: TaskPriority) -> Self {
        value.into_raw()
    }
}

/// Version of the runtime-neutral radio contract.
///
/// A backend is compatible with a requirement when the major versions match
/// and the backend minor version is at least the required minor version.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeContractVersion {
    /// Breaking-contract generation.
    pub major: u16,
    /// Backward-compatible capability generation.
    pub minor: u16,
}

impl RuntimeContractVersion {
    /// Contract implemented by this crate release.
    pub const V1_0: Self = Self { major: 1, minor: 0 };
    /// Adds generation-bearing resource handles and cancellable waits.
    pub const V1_1: Self = Self { major: 1, minor: 1 };
    /// Adds an advisory dynamic-task capacity snapshot.
    pub const V1_2: Self = Self { major: 1, minor: 2 };
    /// Adds owner-bound dynamic-task reservations consumed by task creation.
    pub const V1_3: Self = Self { major: 1, minor: 3 };
    /// Adds atomic task-slot and stack-memory reservations.
    pub const V1_4: Self = Self { major: 1, minor: 4 };

    /// Returns whether this backend version can satisfy `required`.
    pub const fn satisfies(self, required: Self) -> bool {
        self.major == required.major && self.minor >= required.minor
    }
}

/// Runtime capabilities advertised before a radio adapter starts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct RuntimeCapabilities(u32);

impl RuntimeCapabilities {
    /// No runtime service is available.
    pub const NONE: Self = Self(0);
    /// Spawn, yield, sleep, current-task, and task-priority operations.
    pub const TASKS: Self = Self(1 << 0);
    /// Nested scheduler lock with deferred rescheduling.
    pub const SCHEDULER_LOCK: Self = Self(1 << 1);
    /// Interrupt enter/exit tracking and ISR-safe wake delivery.
    pub const INTERRUPT_WAKE: Self = Self(1 << 2);
    /// Counting semaphores with no-wait, timed, and forever waits.
    pub const SEMAPHORE: Self = Self(1 << 3);
    /// Recursive mutexes with priority inheritance.
    pub const RECURSIVE_PI_MUTEX: Self = Self(1 << 4);
    /// Opaque synchronization handles reject stale generations and reuse.
    pub const RESOURCE_HANDLE_GENERATIONS: Self = Self(1 << 5);
    /// Queued waits and unconsumed direct handoffs can be cancelled safely.
    pub const WAIT_CANCELLATION: Self = Self(1 << 6);
    /// Dynamic task capacity can be queried before radio initialization.
    pub const TASK_CAPACITY_QUERY: Self = Self(1 << 7);
    /// Dynamic task slots can be reserved before radio initialization.
    pub const TASK_RESERVATION: Self = Self(1 << 8);
    /// Task stacks can be allocated atomically with dynamic-slot admission.
    pub const TASK_STACK_RESERVATION: Self = Self(1 << 9);

    /// Complete capability set required by runtime contract v1.0.
    pub const V1_0_REQUIRED: Self = Self(
        Self::TASKS.0
            | Self::SCHEDULER_LOCK.0
            | Self::INTERRUPT_WAKE.0
            | Self::SEMAPHORE.0
            | Self::RECURSIVE_PI_MUTEX.0,
    );
    /// Complete capability set required by runtime contract v1.1.
    pub const V1_1_REQUIRED: Self = Self(
        Self::V1_0_REQUIRED.0 | Self::RESOURCE_HANDLE_GENERATIONS.0 | Self::WAIT_CANCELLATION.0,
    );
    /// Complete capability set required by runtime contract v1.2.
    pub const V1_2_REQUIRED: Self = Self(Self::V1_1_REQUIRED.0 | Self::TASK_CAPACITY_QUERY.0);
    /// Complete capability set required by runtime contract v1.3.
    pub const V1_3_REQUIRED: Self = Self(Self::V1_2_REQUIRED.0 | Self::TASK_RESERVATION.0);
    /// Complete capability set required by runtime contract v1.4.
    pub const V1_4_REQUIRED: Self = Self(Self::V1_3_REQUIRED.0 | Self::TASK_STACK_RESERVATION.0);
    /// Minimum capability set accepted when installing a runtime.
    pub const V1_MINIMUM: Self = Self::V1_1_REQUIRED;
    /// Complete capability set implemented by the current native backend.
    pub const V1_CURRENT: Self = Self::V1_4_REQUIRED;

    /// Creates a capability set from its stable bit representation.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Returns the stable bit representation.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns true when every bit in `required` is present.
    pub const fn contains(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }
}

impl BitOr for RuntimeCapabilities {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for RuntimeCapabilities {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// Versioned capabilities offered by one runtime backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeContract {
    /// Contract version implemented by the backend.
    pub version: RuntimeContractVersion,
    /// Runtime services implemented with contract-v1 semantics.
    pub capabilities: RuntimeCapabilities,
}

/// Scheduling modes a runtime can execute with its installed target support.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct RuntimeExecutionModes(u32);

impl RuntimeExecutionModes {
    /// No executable scheduling mode is available.
    pub const NONE: Self = Self(0);
    /// Port-less cooperative scheduling at explicit scheduling points only.
    pub const PORTLESS_COOPERATIVE: Self = Self(1 << 0);
    /// Cooperative tasks backed by timer/SWI switch delivery.
    pub const PORTED_COOPERATIVE: Self = Self(1 << 1);
    /// Periodic CPU quota with timer-enforced throttling.
    pub const BUDGETED: Self = Self(1 << 2);
    /// Priority preemption and equal-priority time slicing.
    pub const PREEMPTIVE: Self = Self(1 << 3);

    /// Creates a mode set from its stable bit representation.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Returns the stable bit representation.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns true when every required mode is implemented.
    pub const fn contains(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }
}

impl BitOr for RuntimeExecutionModes {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for RuntimeExecutionModes {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// Versioned scheduling guarantees offered by one installed runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeExecutionProfile {
    /// Profile schema revision. Revisions are compared exactly.
    pub revision: u16,
    /// Scheduling modes backed by the installed target resources.
    pub modes: RuntimeExecutionModes,
}

impl RuntimeExecutionProfile {
    /// Port-less runtime that can switch only at explicit cooperative points.
    pub const V1_PORTLESS_COOPERATIVE: Self = Self {
        revision: 1,
        modes: RuntimeExecutionModes::PORTLESS_COOPERATIVE,
    };

    /// Runtime with timer and deferred-reschedule delivery installed.
    pub const V1_PORTED: Self = Self {
        revision: 1,
        modes: RuntimeExecutionModes(
            RuntimeExecutionModes::PORTED_COOPERATIVE.0
                | RuntimeExecutionModes::BUDGETED.0
                | RuntimeExecutionModes::PREEMPTIVE.0,
        ),
    };

    /// Minimum profile required by the current WS63 radio compatibility path.
    pub const V1_PORTED_COOPERATIVE: Self = Self {
        revision: 1,
        modes: RuntimeExecutionModes::PORTED_COOPERATIVE,
    };

    /// Returns whether this profile satisfies one adapter requirement.
    pub const fn satisfies(self, required: Self) -> bool {
        self.revision == required.revision && self.modes.contains(required.modes)
    }
}

/// Versioned semantic and execution requirements of one radio adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeRequirements {
    /// Required task and synchronization contract.
    pub contract: RuntimeContract,
    /// Required scheduling guarantees.
    pub execution_profile: RuntimeExecutionProfile,
}

impl RuntimeRequirements {
    /// Contract v1 with timer/SWI-backed cooperative execution.
    pub const V1_PORTED_COOPERATIVE: Self = Self {
        contract: RuntimeContract::V1,
        execution_profile: RuntimeExecutionProfile::V1_PORTED_COOPERATIVE,
    };

    /// Contract v1.3 with atomic task reservation and ported cooperative execution.
    pub const V1_3_PORTED_COOPERATIVE: Self = Self {
        contract: RuntimeContract::V1_3,
        execution_profile: RuntimeExecutionProfile::V1_PORTED_COOPERATIVE,
    };

    /// Contract v1.4 with atomic task-stack admission and ported cooperative execution.
    pub const V1_4_PORTED_COOPERATIVE: Self = Self {
        contract: RuntimeContract::V1_4,
        execution_profile: RuntimeExecutionProfile::V1_PORTED_COOPERATIVE,
    };
}

impl RuntimeContract {
    /// Original runtime contract without resource generations or cancellation.
    pub const V1_0: Self = Self {
        version: RuntimeContractVersion::V1_0,
        capabilities: RuntimeCapabilities::V1_0_REQUIRED,
    };

    /// Runtime contract v1.1, retained for compatibility with existing adapters.
    pub const V1: Self = Self {
        version: RuntimeContractVersion::V1_1,
        capabilities: RuntimeCapabilities::V1_1_REQUIRED,
    };

    /// Runtime contract v1.2 with advisory task-capacity queries.
    pub const V1_2: Self = Self {
        version: RuntimeContractVersion::V1_2,
        capabilities: RuntimeCapabilities::V1_2_REQUIRED,
    };

    /// Runtime contract v1.3 with owner-bound task reservations.
    pub const V1_3: Self = Self {
        version: RuntimeContractVersion::V1_3,
        capabilities: RuntimeCapabilities::V1_3_REQUIRED,
    };

    /// Runtime contract v1.4 with owner-bound task-stack reservations.
    pub const V1_4: Self = Self {
        version: RuntimeContractVersion::V1_4,
        capabilities: RuntimeCapabilities::V1_4_REQUIRED,
    };

    /// Returns whether this backend satisfies `required`.
    pub const fn satisfies(self, required: Self) -> bool {
        self.version.satisfies(required.version)
            && self.capabilities.contains(required.capabilities)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_range_and_order_are_contract_facts() {
        assert_eq!(TaskPriority::new(0), Some(TaskPriority::HIGHEST));
        assert_eq!(TaskPriority::new(31), Some(TaskPriority::LOWEST));
        assert_eq!(TaskPriority::new(32), None);
        assert_eq!(TaskPriority::try_from(32).unwrap_err().value(), 32);
        assert!(TaskPriority::HIGHEST.outranks(TaskPriority::LOWEST));
        assert!(!TaskPriority::LOWEST.outranks(TaskPriority::HIGHEST));
    }

    #[test]
    fn version_and_capability_compatibility_fail_closed() {
        let newer_minor = RuntimeContract {
            version: RuntimeContractVersion { major: 1, minor: 3 },
            capabilities: RuntimeCapabilities::V1_3_REQUIRED,
        };
        assert!(newer_minor.satisfies(RuntimeContract::V1));

        assert!(!RuntimeContract::V1_0.satisfies(RuntimeContract::V1));
        assert!(RuntimeContract::V1.satisfies(RuntimeContract::V1_0));

        let wrong_major = RuntimeContract {
            version: RuntimeContractVersion { major: 2, minor: 0 },
            capabilities: RuntimeCapabilities::V1_2_REQUIRED,
        };
        assert!(!wrong_major.satisfies(RuntimeContract::V1));

        let missing_mutex = RuntimeContract {
            version: RuntimeContractVersion::V1_1,
            capabilities: RuntimeCapabilities::from_bits(
                RuntimeCapabilities::V1_1_REQUIRED.bits()
                    & !RuntimeCapabilities::RECURSIVE_PI_MUTEX.bits(),
            ),
        };
        assert!(!missing_mutex.satisfies(RuntimeContract::V1));

        let missing_cancellation = RuntimeContract {
            version: RuntimeContractVersion::V1_1,
            capabilities: RuntimeCapabilities::from_bits(
                RuntimeCapabilities::V1_1_REQUIRED.bits()
                    & !RuntimeCapabilities::WAIT_CANCELLATION.bits(),
            ),
        };
        assert!(!missing_cancellation.satisfies(RuntimeContract::V1));

        assert!(RuntimeContract::V1_2.satisfies(RuntimeContract::V1));
        assert!(!RuntimeContract::V1.satisfies(RuntimeContract::V1_2));
    }

    #[test]
    fn execution_profiles_do_not_conflate_portless_and_ported_modes() {
        assert!(
            RuntimeExecutionProfile::V1_PORTED
                .satisfies(RuntimeExecutionProfile::V1_PORTED_COOPERATIVE)
        );
        assert!(
            !RuntimeExecutionProfile::V1_PORTLESS_COOPERATIVE
                .satisfies(RuntimeExecutionProfile::V1_PORTED_COOPERATIVE)
        );
        assert!(
            !RuntimeExecutionProfile::V1_PORTED.satisfies(RuntimeExecutionProfile {
                revision: 2,
                modes: RuntimeExecutionModes::PORTED_COOPERATIVE,
            })
        );
    }
}
