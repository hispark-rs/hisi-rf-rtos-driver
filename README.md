# hisi-rf-rtos-driver

`no_std`, chip-neutral task and synchronization contract between HiSilicon
radio adapters and a selected runtime. It contains no scheduler, radio policy,
chip registers, allocator, or network stack.

Exactly one runtime is installed per firmware. Radio callbacks may wake a task
through the bounded semaphore operation, but user callbacks never execute in an
ISR, critical section, or scheduler lock.

