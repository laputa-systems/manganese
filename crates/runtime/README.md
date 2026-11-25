# Deadlines

`Deadline::after(delay)` creates a Smolith monotonic timer. It returns `DeadlineOverflow` when the clock cannot represent the deadline. `Deadline::wait` returns the duration between deadline creation and the scheduled firing instant. The task may resume later.

`Deadline::controlled()` returns a trigger and a deadline. `DeadlineTrigger::fire` supplies the elapsed duration once. Dropping the trigger before firing makes `wait` return `TimerCanceled`. Dropping the deadline makes `fire` return `TimerCanceled`.

The runtime box executable drives controlled deadlines through the task runners described in `boxes/runtime/box.toml`. Its runner interface is internal to the box. Browser service integration remains an assembly obligation.
