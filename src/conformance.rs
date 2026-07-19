//! Runtime-neutral executable semantics for radio backends.
//!
//! The production [`crate::Runtime`] trait intentionally stays small. This
//! module provides a separate deterministic harness for proving the behavior
//! behind that trait without adding test controls to the production ABI.

use core::fmt;

use crate::{Error, RuntimeContract, TaskPriority};

/// Version of the conformance scenario and report schema.
pub const SCHEMA_VERSION: u16 = 1;

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
    /// Exit the current task.
    ExitTask,
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
}

impl ScenarioId {
    /// Returns the stable machine-readable scenario name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PriorityThenFifo => "priority_then_fifo",
            Self::NestedSchedulerLock => "nested_scheduler_lock",
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
            "{{\"schema_version\":{},\"contract\":{{\"major\":{},\"minor\":{},\"capabilities\":{}}},\"backend_revision\":",
            self.schema_version,
            self.contract.version.major,
            self.contract.version.minor,
            self.contract.capabilities.bits(),
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

/// Initial shared scenarios. Additional synchronization, timeout, IRQ and task
/// lifecycle scenarios must be added before A5R can be declared complete.
pub const V1_SCENARIOS: [Scenario; 2] = [
    Scenario {
        id: ScenarioId::PriorityThenFifo,
        steps: PRIORITY_FIFO_STEPS,
    },
    Scenario {
        id: ScenarioId::NestedSchedulerLock,
        steps: NESTED_LOCK_STEPS,
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
        assert!(json.contains("\"status\":\"failed\""));
    }
}
