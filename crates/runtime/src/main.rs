use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use async_task::Task;
use runtime::{Deadline, DeadlineTrigger};
use smol::channel::{Receiver, Sender};
use smol::lock::Mutex;

type Trace = Arc<Mutex<Vec<String>>>;

struct Spawn {
    token: u64,
    id: u32,
    runner: u32,
    alarm: Deadline,
    steps: Sender<Step>,
    effect: Effect,
    weak: Option<Weak<()>>,
    action: Option<WorkAction>,
    affinity: Option<Arc<std::sync::Mutex<Option<thread::ThreadId>>>>,
    trace: Trace,
}

async fn event(spawn: Spawn, sequence: Option<Arc<Mutex<()>>>) {
    if let Ok(now) = spawn.alarm.wait().await {
        let target = spawn.weak.as_ref().and_then(Weak::upgrade);
        if spawn.weak.is_some() && target.is_none() {
            spawn.steps.send(Step::Done).await.expect("task observer");
            return;
        }
        let _sequence = match &sequence {
            Some(sequence) => Some(sequence.lock().await),
            None => None,
        };
        if let Some(affinity) = &spawn.affinity {
            let mut expected = affinity.lock().expect("thread affinity lock");
            let current = thread::current().id();
            if let Some(id) = *expected { assert_eq!(id, current, "thread affinity changed"); }
            else { *expected = Some(current); }
        }
        let visible = !matches!(&spawn.action, Some(WorkAction::ShutdownProbe(_) | WorkAction::Barrier(_) | WorkAction::TrackerQuery { .. }));
        if let Some(action) = spawn.action { action.run(); }
        if visible {
            spawn.trace.lock().await.push(format!("E {} {} {}\n", spawn.id, now.as_millis(), spawn.runner));
        }
        if matches!(spawn.effect, Effect::Post { .. } | Effect::Cancel { .. } | Effect::CancelTracked { .. }) {
            let (ack, wait) = smol::channel::bounded(1);
            spawn.steps.send(Step::Nested(spawn.effect, ack)).await.expect("task observer");
            wait.recv().await.expect("nested task acknowledged");
        }
        spawn.steps.send(Step::Done).await.expect("task observer");
    }
}

struct Pool {
    executor: Arc<smol::Executor<'static>>,
    stop: Sender<()>,
    threads: Vec<JoinHandle<()>>,
}

impl Pool {
    fn new() -> Self {
        let executor = Arc::new(smol::Executor::new());
        let (stop, stopped) = smol::channel::bounded::<()>(1);
        let mut threads = Vec::with_capacity(2);
        for index in 0..2 {
            let worker_executor = executor.clone();
            let worker_stop = stopped.clone();
            let worker = thread::Builder::new().name(format!("manganese-pool-{index}")).spawn(move || {
                smol::block_on(worker_executor.run(async { let _ = worker_stop.recv().await; }));
            });
            match worker {
                Ok(worker) => threads.push(worker),
                Err(error) => {
                    stop.close();
                    for worker in threads {
                        if worker.join().is_err() { eprintln!("pool worker failed during startup"); }
                    }
                    panic!("create pool worker: {error}");
                }
            }
        }
        Self { executor, stop, threads }
    }

    fn close(&mut self) -> Result<(), &'static str> {
        self.stop.close();
        let mut failed = false;
        for worker in self.threads.drain(..) { failed |= worker.join().is_err(); }
        if failed { Err("pool worker failed") } else { Ok(()) }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        if let Err(error) = self.close() { eprintln!("{error}"); }
    }
}

enum Message {
    Spawn(Spawn),
    Cancel(u64),
    Forget(u64),
    Stop,
}

enum Runner {
    Local {
        executor: smol::LocalExecutor<'static>,
        tasks: HashMap<u64, Task<()>>,
    },
    Pooled {
        executor: Arc<smol::Executor<'static>>,
        sequence: Arc<Mutex<()>>,
        tasks: HashMap<u64, Task<()>>,
    },
    Thread {
        commands: Sender<Message>,
        thread: Option<JoinHandle<()>>,
    },
}

impl Runner {
    fn local() -> Self {
        Self::Local { executor: smol::LocalExecutor::new(), tasks: HashMap::new() }
    }

    fn threaded(name: String) -> Self {
        let (commands, input) = smol::channel::bounded(1);
        let thread = thread::Builder::new().name(name).spawn(move || {
            let executor = smol::LocalExecutor::new();
            smol::block_on(executor.run(async {
                let mut tasks = HashMap::new();
                while let Ok(message) = input.recv().await {
                    match message {
                        Message::Spawn(spawn) => {
                            let token = spawn.token;
                            tasks.insert(token, executor.spawn(event(spawn, None)));
                        }
                        Message::Cancel(token) | Message::Forget(token) => {
                            tasks.remove(&token);
                        }
                        Message::Stop => break,
                    }
                }
            }));
        }).expect("create runner thread");
        Self::Thread { commands, thread: Some(thread) }
    }

    fn pooled(executor: Arc<smol::Executor<'static>>) -> Self {
        Self::Pooled { executor, sequence: Arc::new(Mutex::new(())), tasks: HashMap::new() }
    }

    fn spawn(&mut self, spawn: Spawn) {
        match self {
            Self::Local { executor, tasks } => {
                let token = spawn.token;
                tasks.insert(token, executor.spawn(event(spawn, None)));
            }
            Self::Pooled { executor, sequence, tasks } => {
                let token = spawn.token;
                tasks.insert(token, executor.spawn(event(spawn, Some(sequence.clone()))));
            }
            Self::Thread { commands, .. } => commands.send_blocking(Message::Spawn(spawn)).expect("runner available"),
        }
    }

    fn next_step(&mut self, steps: &Receiver<Step>) -> Step {
        match self {
            Self::Local { executor, .. } => {
                loop {
                    if let Ok(step) = steps.try_recv() { return step; }
                    assert!(executor.try_tick(), "scheduled task did not wake");
                }
            }
            Self::Thread { .. } | Self::Pooled { .. } => steps.recv_blocking().expect("task observer"),
        }
    }

    fn forget(&mut self, token: u64) {
        match self {
            Self::Local { tasks, .. } | Self::Pooled { tasks, .. } => { tasks.remove(&token); }
            Self::Thread { commands, .. } => commands.send_blocking(Message::Forget(token)).expect("runner available"),
        }
    }

    fn cancel(&mut self, token: u64) {
        match self {
            Self::Local { tasks, .. } | Self::Pooled { tasks, .. } => { tasks.remove(&token); }
            Self::Thread { commands, .. } => commands.send_blocking(Message::Cancel(token)).expect("runner available"),
        }
    }

    fn close(&mut self) -> Result<(), &'static str> {
        if let Self::Thread { commands, thread } = self
            && let Some(worker) = thread.take() {
            let sent = commands.send_blocking(Message::Stop).is_ok();
            let joined = worker.join().is_ok();
            if !sent || !joined { return Err("runner thread failed"); }
        }
        Ok(())
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        if let Err(error) = self.close() { eprintln!("{error}"); }
    }
}

#[derive(Clone, Copy)]
enum Effect {
    None,
    Post { runner: u32, id: u32, delay: u64 },
    Cancel { id: u32 },
    CancelTracked { id: u32 },
    Reply { id: u32 },
    TrackedReply { task: u32, reply: u32 },
    Repeat { timer: u32, generation: u64, id: u32, period: u64 },
}

enum Step {
    Nested(Effect, Sender<()>),
    Done,
}

struct Job {
    runner: u32,
    alarm: DeadlineTrigger,
    steps: Receiver<Step>,
    effect: Effect,
}

struct Timer {
    generation: u64,
    token: Option<u64>,
}

struct Bound {
    runner: u32,
    destroy: u32,
    value: BoundSlot,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ShutdownBehavior {
    Continue,
    Skip,
    Block,
}

impl ShutdownBehavior {
    fn from_wire(value: u32) -> Self {
        match value {
            0 => Self::Continue,
            1 => Self::Skip,
            2 => Self::Block,
            _ => panic!("invalid shutdown behavior"),
        }
    }
}

#[derive(Clone, Copy)]
enum Priority {
    BestEffort,
    UserVisible,
    UserBlocking,
}

impl Priority {
    fn from_wire(value: u32) -> Self {
        match value {
            0 => Self::BestEffort,
            1 => Self::UserVisible,
            2 => Self::UserBlocking,
            _ => panic!("invalid priority"),
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::BestEffort => 2,
            Self::UserVisible => 1,
            Self::UserBlocking => 0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RunnerKind {
    Main,
    Sequenced,
    SingleThread,
}

impl RunnerKind {
    fn from_wire(value: u32) -> Self {
        match value {
            1 => Self::Sequenced,
            2 => Self::SingleThread,
            _ => panic!("invalid runner kind"),
        }
    }
}

type BoundSlot = Arc<std::sync::Mutex<Option<BoundValue>>>;

struct BoundValue {
    creator: thread::ThreadId,
    pinned: bool,
}

impl BoundValue {
    fn check(&self) {
        assert!(!self.pinned || self.creator == thread::current().id(), "thread affinity changed");
    }
}

impl Drop for BoundValue {
    fn drop(&mut self) { self.check(); }
}

enum WorkAction {
    Create(BoundSlot, bool),
    Call(BoundSlot),
    Destroy(BoundSlot),
    ShutdownProbe(Arc<AtomicBool>),
    Barrier(Arc<std::sync::Barrier>),
    TrackerQuery { id: u32, flag: Arc<AtomicBool>, trace: Trace },
}

impl WorkAction {
    fn run(self) {
        match self {
            Self::Create(slot, pinned) => {
                let mut value = slot.lock().expect("bound object lock");
                assert!(value.replace(BoundValue { creator: thread::current().id(), pinned }).is_none());
            }
            Self::Call(slot) => slot.lock().expect("bound object lock").as_ref().expect("bound object").check(),
            Self::Destroy(slot) => { slot.lock().expect("bound object lock").take().expect("bound object"); }
            Self::ShutdownProbe(ran) => ran.store(true, Ordering::SeqCst),
            Self::Barrier(barrier) => { barrier.wait(); }
            Self::TrackerQuery { id, flag, trace } => {
                let canceled = flag.load(Ordering::SeqCst);
                smol::block_on(trace.lock()).push(format!("O {} {}\n", id, u8::from(canceled)));
            }
        }
    }
}

struct Runtime {
    now: u64,
    serial: u64,
    runners: HashMap<u32, Runner>,
    pool: Option<Pool>,
    queue: BinaryHeap<Reverse<(u64, u8, u64)>>,
    jobs: HashMap<u64, Job>,
    handles: HashMap<u32, u64>,
    tracked: HashMap<u32, Tracked>,
    weak: HashMap<u32, Arc<()>>,
    timers: HashMap<u32, Timer>,
    bound: HashMap<u32, Bound>,
    priorities: HashMap<u32, Priority>,
    kinds: HashMap<u32, RunnerKind>,
    affinities: HashMap<u32, Arc<std::sync::Mutex<Option<thread::ThreadId>>>>,
    shutdown: HashMap<u32, ShutdownBehavior>,
    trace: Trace,
}

enum Tracked {
    Task(u64),
    Flag(Arc<AtomicBool>),
}

impl Runtime {
    fn new() -> Self {
        let mut runners = HashMap::new();
        runners.insert(0, Runner::local());
        Self {
            now: 0,
            serial: 0,
            runners,
            pool: None,
            queue: BinaryHeap::new(),
            jobs: HashMap::new(),
            handles: HashMap::new(),
            tracked: HashMap::new(),
            weak: HashMap::new(),
            timers: HashMap::new(),
            bound: HashMap::new(),
            priorities: HashMap::from([(0, Priority::UserBlocking)]),
            kinds: HashMap::from([(0, RunnerKind::Main)]),
            affinities: HashMap::from([(0, Arc::new(std::sync::Mutex::new(Some(thread::current().id()))))]),
            shutdown: HashMap::new(),
            trace: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn schedule(&mut self, runner: u32, id: u32, delay: u64, effect: Effect) -> u64 {
        self.schedule_work(runner, id, delay, effect, None, None)
    }

    fn schedule_work(&mut self, runner: u32, id: u32, delay: u64, effect: Effect,
                     weak: Option<Weak<()>>, action: Option<WorkAction>) -> u64 {
        self.serial = self.serial.checked_add(1).expect("task id overflow");
        let token = self.serial;
        let deadline = self.now.checked_add(delay).expect("deadline overflow");
        let (alarm, receiver) = Deadline::controlled();
        let (steps, completed) = smol::channel::bounded(1);
        let affinity = self.affinities.get(&runner).cloned();
        let spawn = Spawn { token, id, runner, alarm: receiver, steps, effect, weak, action, affinity, trace: self.trace.clone() };
        self.runners.get_mut(&runner).expect("unknown runner").spawn(spawn);
        self.jobs.insert(token, Job { runner, alarm, steps: completed, effect });
        self.queue.push(Reverse((deadline, self.priorities[&runner].rank(), token)));
        token
    }

    fn cancel_token(&mut self, token: u64) -> bool {
        if let Some(job) = self.jobs.remove(&token) {
            self.runners.get_mut(&job.runner).expect("runner").cancel(token);
            true
        } else {
            false
        }
    }

    fn cancellation(&mut self, id: u32, tracked: bool) {
        let valid = if tracked {
            match self.tracked.get(&id) {
                Some(Tracked::Task(token)) => { let token = *token; self.cancel_token(token) }
                Some(Tracked::Flag(flag)) => !flag.swap(true, Ordering::SeqCst),
                None => false,
            }
        } else {
            self.handles.get(&id).copied().is_some_and(|token| self.cancel_token(token))
        };
        smol::block_on(self.trace.lock()).push(format!("C {} {}\n", id, u8::from(valid)));
    }

    fn stop_timer(&mut self, timer: u32) {
        let token = self.timers.get(&timer).and_then(|entry| entry.token);
        if let Some(token) = token { self.cancel_token(token); }
        if let Some(entry) = self.timers.get_mut(&timer) {
            entry.generation += 1;
            entry.token = None;
        }
    }

    fn start_timer(&mut self, timer: u32, repeating: bool, delay: u64, id: u32) {
        self.stop_timer(timer);
        let generation = self.timers.get(&timer).map_or(1, |entry| entry.generation);
        let effect = if repeating { Effect::Repeat { timer, generation, id, period: delay } } else { Effect::None };
        let token = self.schedule(0, id, delay, effect);
        self.timers.insert(timer, Timer { generation, token: Some(token) });
    }

    fn run_token(&mut self, token: u64) {
        let Some(job) = self.jobs.remove(&token) else { return; };
        job.alarm.fire(Duration::from_millis(self.now)).expect("task alarm receiver");
        loop {
            let step = self.runners.get_mut(&job.runner).expect("runner").next_step(&job.steps);
            match step {
                Step::Nested(Effect::Post { runner, id, delay }, ack) => {
                    self.schedule(runner, id, delay, Effect::None);
                    ack.send_blocking(()).expect("nested task acknowledgment");
                }
                Step::Nested(Effect::Cancel { id }, ack) => {
                    self.cancellation(id, false);
                    ack.send_blocking(()).expect("nested task acknowledgment");
                }
                Step::Nested(Effect::CancelTracked { id }, ack) => {
                    self.cancellation(id, true);
                    ack.send_blocking(()).expect("nested task acknowledgment");
                }
                Step::Nested(_, _) => unreachable!(),
                Step::Done => break,
            }
        }
        self.runners.get_mut(&job.runner).expect("runner").forget(token);
        match job.effect {
            Effect::None | Effect::Post { .. } | Effect::Cancel { .. } | Effect::CancelTracked { .. } => {}
            Effect::Reply { id } => { self.schedule(0, id, 0, Effect::None); }
            Effect::TrackedReply { task, reply } => {
                let token = self.schedule(0, reply, 0, Effect::None);
                self.tracked.insert(task, Tracked::Task(token));
            }
            Effect::Repeat { timer, generation, id, period } => {
                if self.timers.get(&timer).is_some_and(|entry| entry.generation == generation) {
                    let next = self.schedule(0, id, period, job.effect);
                    self.timers.get_mut(&timer).expect("timer").token = Some(next);
                }
            }
        }
    }

    fn drain(&mut self, until: u64) {
        while let Some(&Reverse((deadline, _, token))) = self.queue.peek() {
            if deadline > until { break; }
            self.queue.pop();
            if self.jobs.contains_key(&token) {
                self.now = self.now.max(deadline);
                self.run_token(token);
            }
        }
        self.now = until;
    }

    fn shutdown(&mut self) {
        let bounds = std::mem::take(&mut self.bound);
        for (_, value) in bounds {
            self.schedule_work(value.runner, value.destroy, 0, Effect::None,
                               None, Some(WorkAction::Destroy(value.value)));
        }
        loop {
            let mut ready = None;
            while let Some(&Reverse((deadline, _, token))) = self.queue.peek() {
                if deadline > self.now { break; }
                self.queue.pop();
                if self.jobs.get(&token).is_some_and(|job| job.runner != 0) {
                    ready = Some(token);
                    break;
                }
            }
            match ready {
                Some(token) => self.run_token(token),
                None => break,
            }
        }
    }

    fn probe_shutdown(&mut self, runner: u32, id: u32) {
        let ran = Arc::new(AtomicBool::new(false));
        let token = self.schedule_work(runner, id, 0, Effect::None, None,
                                       Some(WorkAction::ShutdownProbe(ran.clone())));
        if self.shutdown[&runner] == ShutdownBehavior::Block { self.run_token(token); } else { self.cancel_token(token); }
        smol::block_on(self.trace.lock()).push(format!("Z {} {}\n", id, u8::from(ran.load(Ordering::SeqCst))));
    }

    fn probe_concurrency(&mut self, first: u32, second: u32) {
        assert_ne!(first, second);
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let first_token = self.schedule_work(first, 0, 0, Effect::None, None, Some(WorkAction::Barrier(barrier.clone())));
        let second_token = self.schedule_work(second, 0, 0, Effect::None, None, Some(WorkAction::Barrier(barrier)));
        let Job { alarm: first_alarm, steps: first_steps, .. } = self.jobs.remove(&first_token).expect("first barrier task");
        let Job { alarm: second_alarm, steps: second_steps, .. } = self.jobs.remove(&second_token).expect("second barrier task");
        first_alarm.fire(Duration::from_millis(self.now)).expect("first barrier alarm");
        second_alarm.fire(Duration::from_millis(self.now)).expect("second barrier alarm");
        for (runner, token, steps) in [(first, first_token, first_steps), (second, second_token, second_steps)] {
            assert!(matches!(self.runners.get_mut(&runner).expect("runner").next_step(&steps), Step::Done));
            self.runners.get_mut(&runner).expect("runner").forget(token);
        }
        smol::block_on(self.trace.lock()).push("G 1\n".to_owned());
    }

    fn close(&mut self) {
        for runner in self.runners.values_mut() { runner.close().expect("runner shutdown"); }
        self.runners.clear();
        if let Some(mut pool) = self.pool.take() { pool.close().expect("pool shutdown"); }
    }
}

fn number<'a>(words: &mut impl Iterator<Item = &'a str>) -> u32 {
    words.next().expect("missing argument").parse().expect("invalid integer")
}

fn execute(script: &str) -> String {
    let mut runtime = Runtime::new();
    for line in script.lines() {
        let mut words = line.split_whitespace();
        let Some(op) = words.next() else { continue; };
        match op {
            "R" => {
                let id = number(&mut words);
                let kind = RunnerKind::from_wire(number(&mut words));
                let priority = Priority::from_wire(number(&mut words));
                let shutdown = ShutdownBehavior::from_wire(number(&mut words));
                let runner = match kind {
                    RunnerKind::Sequenced => {
                        let pool = runtime.pool.get_or_insert_with(Pool::new);
                        Runner::pooled(pool.executor.clone())
                    }
                    RunnerKind::SingleThread => Runner::threaded(format!("manganese-thread-{id}")),
                    RunnerKind::Main => unreachable!(),
                };
                assert!(runtime.runners.insert(id, runner).is_none());
                runtime.priorities.insert(id, priority);
                runtime.kinds.insert(id, kind);
                if kind == RunnerKind::SingleThread {
                    runtime.affinities.insert(id, Arc::new(std::sync::Mutex::new(None)));
                }
                runtime.shutdown.insert(id, shutdown);
            }
            "P" => {
                let runner = number(&mut words);
                let id = number(&mut words);
                let delay = u64::from(number(&mut words));
                let kind = number(&mut words);
                let target_runner = number(&mut words);
                let target = number(&mut words);
                let target_delay = u64::from(number(&mut words));
                let effect = match kind {
                    0 => Effect::None,
                    1 => Effect::Post { runner: target_runner, id: target, delay: target_delay },
                    2 => Effect::Cancel { id: target },
                    3 => Effect::CancelTracked { id: target },
                    _ => panic!("unknown task effect"),
                };
                runtime.schedule(runner, id, delay, effect);
            }
            "L" => {
                let id = number(&mut words);
                let delay = u64::from(number(&mut words));
                let kind = number(&mut words);
                let target_runner = number(&mut words);
                let target = number(&mut words);
                let target_delay = u64::from(number(&mut words));
                let effect = match kind {
                    0 => Effect::None,
                    1 => Effect::Post { runner: target_runner, id: target, delay: target_delay },
                    2 => Effect::Cancel { id: target },
                    3 => Effect::CancelTracked { id: target },
                    _ => panic!("unknown task effect"),
                };
                let token = runtime.schedule(0, id, delay, effect);
                assert!(runtime.handles.insert(id, token).is_none());
            }
            "C" => { let id = number(&mut words); runtime.cancellation(id, false); }
            "K" => {
                let runner = number(&mut words);
                let id = number(&mut words);
                let token = runtime.schedule(runner, id, 0, Effect::None);
                assert!(runtime.tracked.insert(id, Tracked::Task(token)).is_none());
            }
            "Y" => {
                let runner = number(&mut words);
                let id = number(&mut words);
                let reply = number(&mut words);
                let token = runtime.schedule(runner, id, 0, Effect::TrackedReply { task: id, reply });
                assert!(runtime.tracked.insert(id, Tracked::Task(token)).is_none());
            }
            "X" => { let id = number(&mut words); runtime.cancellation(id, true); }
            "U" => {
                let id = number(&mut words);
                assert!(runtime.tracked.insert(id, Tracked::Flag(Arc::new(AtomicBool::new(false)))).is_none());
            }
            "O" | "N" => {
                let runner = if op == "N" { number(&mut words) } else { 0 };
                let id = number(&mut words);
                let flag = match runtime.tracked.get(&id).expect("unknown tracked callback") {
                    Tracked::Flag(flag) => flag.clone(),
                    Tracked::Task(_) => panic!("tracked task has no cancellation callback"),
                };
                if op == "N" {
                    let trace = runtime.trace.clone();
                    runtime.schedule_work(runner, id, 0, Effect::None, None,
                                          Some(WorkAction::TrackerQuery { id, flag, trace }));
                } else {
                    let canceled = flag.load(Ordering::SeqCst);
                    smol::block_on(runtime.trace.lock()).push(format!("O {} {}\n", id, u8::from(canceled)));
                }
            }
            "W" => {
                let id = number(&mut words);
                let delay = u64::from(number(&mut words));
                let target = Arc::new(());
                runtime.schedule_work(0, id, delay, Effect::None, Some(Arc::downgrade(&target)), None);
                assert!(runtime.weak.insert(id, target).is_none());
            }
            "V" => {
                let id = number(&mut words);
                runtime.weak.remove(&id).expect("unknown weak task");
            }
            "Q" => {
                let runner = number(&mut words);
                let id = number(&mut words);
                let reply = number(&mut words);
                runtime.schedule(runner, id, 0, Effect::Reply { id: reply });
            }
            "T" => {
                let timer = number(&mut words);
                let kind = number(&mut words);
                let delay = u64::from(number(&mut words));
                let id = number(&mut words);
                assert!(kind == 0 || delay > 0);
                runtime.start_timer(timer, kind == 1, delay, id);
            }
            "S" => { let timer = number(&mut words); runtime.stop_timer(timer); }
            "Z" => {
                let runner = number(&mut words);
                let id = number(&mut words);
                runtime.probe_shutdown(runner, id);
            }
            "G" => {
                let first = number(&mut words);
                let second = number(&mut words);
                runtime.probe_concurrency(first, second);
            }
            "B" => {
                let bound = number(&mut words);
                let runner = number(&mut words);
                let construct = number(&mut words);
                let destroy = number(&mut words);
                let value = Arc::new(std::sync::Mutex::new(None));
                assert!(runtime.bound.insert(bound, Bound { runner, destroy, value: value.clone() }).is_none());
                runtime.schedule_work(runner, construct, 0, Effect::None, None,
                                      Some(WorkAction::Create(value, runtime.kinds[&runner] != RunnerKind::Sequenced)));
            }
            "F" => {
                let bound = number(&mut words);
                let id = number(&mut words);
                let value = runtime.bound.get(&bound).expect("unknown bound object");
                let runner = value.runner;
                let slot = value.value.clone();
                runtime.schedule_work(runner, id, 0, Effect::None, None, Some(WorkAction::Call(slot)));
            }
            "J" => {
                let bound = number(&mut words);
                let id = number(&mut words);
                let reply = number(&mut words);
                let value = runtime.bound.get(&bound).expect("unknown bound object");
                let runner = value.runner;
                let slot = value.value.clone();
                runtime.schedule_work(runner, id, 0, Effect::Reply { id: reply }, None, Some(WorkAction::Call(slot)));
            }
            "D" => {
                let bound = number(&mut words);
                let value = runtime.bound.remove(&bound).expect("unknown bound object");
                runtime.schedule_work(value.runner, value.destroy, 0, Effect::None,
                                      None, Some(WorkAction::Destroy(value.value)));
            }
            "A" => {
                let delay = u64::from(number(&mut words));
                runtime.drain(runtime.now.checked_add(delay).expect("clock overflow"));
            }
            "I" => runtime.drain(runtime.now),
            "H" => break,
            _ => panic!("unknown operation"),
        }
        assert!(words.next().is_none(), "trailing arguments");
    }
    runtime.shutdown();
    let result = smol::block_on(runtime.trace.lock()).concat();
    runtime.close();
    result
}

fn main() -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut magic = [0; 4];
    input.read_exact(&mut magic)?;
    if &magic != b"MGB1" { return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid protocol")); }
    output.write_all(b"MGB1")?;
    output.flush()?;
    loop {
        let mut header = [0; 4];
        match input.read_exact(&mut header[..1]) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error),
        }
        input.read_exact(&mut header[1..])?;
        let mut script = vec![0; u32::from_be_bytes(header) as usize];
        input.read_exact(&mut script)?;
        let script = std::str::from_utf8(&script).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let response = execute(script);
        let length = u32::try_from(response.len()).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        output.write_all(&length.to_be_bytes())?;
        output.write_all(response.as_bytes())?;
        output.flush()?;
    }
}
