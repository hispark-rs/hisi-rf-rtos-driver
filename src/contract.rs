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

    /// Complete capability set required by runtime contract v1.
    pub const V1_REQUIRED: Self = Self(
        Self::TASKS.0
            | Self::SCHEDULER_LOCK.0
            | Self::INTERRUPT_WAKE.0
            | Self::SEMAPHORE.0
            | Self::RECURSIVE_PI_MUTEX.0,
    );

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

impl RuntimeContract {
    /// Complete runtime contract implemented by current native backends.
    pub const V1: Self = Self {
        version: RuntimeContractVersion::V1_0,
        capabilities: RuntimeCapabilities::V1_REQUIRED,
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
            version: RuntimeContractVersion { major: 1, minor: 1 },
            capabilities: RuntimeCapabilities::V1_REQUIRED,
        };
        assert!(newer_minor.satisfies(RuntimeContract::V1));

        let wrong_major = RuntimeContract {
            version: RuntimeContractVersion { major: 2, minor: 0 },
            capabilities: RuntimeCapabilities::V1_REQUIRED,
        };
        assert!(!wrong_major.satisfies(RuntimeContract::V1));

        let missing_mutex = RuntimeContract {
            version: RuntimeContractVersion::V1_0,
            capabilities: RuntimeCapabilities::from_bits(
                RuntimeCapabilities::V1_REQUIRED.bits()
                    & !RuntimeCapabilities::RECURSIVE_PI_MUTEX.bits(),
            ),
        };
        assert!(!missing_mutex.satisfies(RuntimeContract::V1));
    }
}
