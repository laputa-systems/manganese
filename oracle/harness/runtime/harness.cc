#include <cstdint>
#include <barrier>
#include <iostream>
#include <map>
#include <memory>
#include <mutex>
#include <optional>
#include <sstream>
#include <string>
#include <utility>
#include <vector>

#include "base/command_line.h"
#include "base/functional/bind.h"
#include "base/location.h"
#include "base/memory/weak_ptr.h"
#include "base/memory/ref_counted.h"
#include "base/task/cancelable_task_tracker.h"
#include "base/task/delayed_task_handle.h"
#include "base/task/sequenced_task_runner.h"
#include "base/task/single_thread_task_runner.h"
#include "base/task/task_traits.h"
#include "base/task/thread_pool.h"
#include "base/task/thread_pool/sequence.h"
#include "base/task/thread_pool/task.h"
#include "base/task/thread_pool/task_tracker.h"
#include "base/test/task_environment.h"
#include "base/test/test_timeouts.h"
#include "base/threading/platform_thread.h"
#include "base/threading/scoped_blocking_call.h"
#include "base/threading/sequence_bound.h"
#include "base/time/time.h"
#include "base/timer/timer.h"

struct Context;

struct Bound {
  Bound(Context* context, int runner, int construct, int destroy);
  ~Bound();
  void Hit(int id);
  int HitAndReturn(int id);
  Context* context;
  int runner;
  int destroy;
};

struct WeakTarget {
  WeakTarget(Context* context, int id);
  void Run();
  Context* context;
  int id;
  base::WeakPtrFactory<WeakTarget> weak_factory{this};
};

struct Runner {
  scoped_refptr<base::SequencedTaskRunner> task_runner;
  int kind;
  int shutdown;
  std::optional<base::PlatformThreadId> thread_id;
};

struct Context {
  base::TimeTicks origin;
  std::mutex mutex;
  std::vector<std::string> trace;
  std::map<int, Runner> runners;
  std::map<int, base::DelayedTaskHandle> handles;
  std::map<int, base::CancelableTaskTracker::TaskId> tracked;
  std::map<int, bool> tracked_pending;
  std::map<int, base::CancelableTaskTracker::IsCanceledCallback> tracked_callbacks;
  std::map<int, std::unique_ptr<WeakTarget>> weak;
  std::map<int, std::unique_ptr<base::OneShotTimer>> once;
  std::map<int, std::unique_ptr<base::RepeatingTimer>> repeating;
  std::map<int, std::unique_ptr<base::SequenceBound<Bound>>> bound;
  base::CancelableTaskTracker tracker;

  void Emit(int id, int runner) {
    if (!runners.at(runner).task_runner->RunsTasksInCurrentSequence())
      std::abort();
    auto now = (base::TimeTicks::Now() - origin).InMilliseconds();
    std::lock_guard<std::mutex> guard(mutex);
    auto& current = runners.at(runner);
    if (current.kind == 2) {
      auto thread_id = base::PlatformThread::CurrentId();
      if (current.thread_id && *current.thread_id != thread_id) std::abort();
      current.thread_id = thread_id;
    }
    trace.push_back("E " + std::to_string(id) + " " + std::to_string(now) + " " + std::to_string(runner) + "\n");
  }

  void Cancel(int id) {
    auto it = handles.find(id);
    bool valid = it != handles.end() && it->second.IsValid();
    if (it != handles.end()) it->second.CancelTask();
    std::lock_guard<std::mutex> guard(mutex);
    trace.push_back("C " + std::to_string(id) + " " + std::to_string(valid) + "\n");
  }

  void Run(int runner, int id, int effect, int target_runner, int target, int delay) {
    Emit(id, runner);
    if (effect == 1) Post(target_runner, target, delay, 0, 0, 0, 0);
    if (effect == 2) Cancel(target);
    if (effect == 3) CancelTracked(target);
  }

  void Post(int runner, int id, int delay, int effect, int target_runner, int target, int target_delay) {
    runners.at(runner).task_runner->PostDelayedTask(
        FROM_HERE,
        base::BindOnce(&Context::Run, base::Unretained(this), runner, id, effect,
                       target_runner, target, target_delay),
        base::Milliseconds(delay));
  }

  void RunTracked(int runner, int id) {
    Emit(id, runner);
    std::lock_guard<std::mutex> guard(mutex);
    tracked_pending[id] = false;
  }

  void CancelTracked(int id) {
    bool valid;
    {
      std::lock_guard<std::mutex> guard(mutex);
      valid = tracked_pending[id];
      tracked_pending[id] = false;
      trace.push_back("C " + std::to_string(id) + " " + std::to_string(valid) + "\n");
    }
    auto it = tracked.find(id);
    if (it != tracked.end()) tracker.TryCancel(it->second);
  }

  void RunTrackedReply(int runner, int id) { Emit(id, runner); }

  void FinishTrackedReply(int id, int reply) {
    Emit(reply, 0);
    std::lock_guard<std::mutex> guard(mutex);
    tracked_pending[id] = false;
  }

  void CheckCancellation(int id, base::CancelableTaskTracker::IsCanceledCallback callback) {
    bool canceled = callback.Run();
    std::lock_guard<std::mutex> guard(mutex);
    trace.push_back("O " + std::to_string(id) + " " + std::to_string(canceled) + "\n");
  }

  void StartTimer(int timer, int kind, int delay, int id) {
    if (kind == 0) {
      repeating.erase(timer);
      auto& value = once[timer];
      if (!value) value = std::make_unique<base::OneShotTimer>();
      value->Start(FROM_HERE, base::Milliseconds(delay),
                   base::BindOnce(&Context::Emit, base::Unretained(this), id, 0));
    } else {
      once.erase(timer);
      auto& value = repeating[timer];
      if (!value) value = std::make_unique<base::RepeatingTimer>();
      value->Start(FROM_HERE, base::Milliseconds(delay),
                   base::BindRepeating(&Context::Emit, base::Unretained(this), id, 0));
    }
  }

  void StopTimer(int timer) {
    auto one = once.find(timer);
    if (one != once.end()) one->second->Stop();
    auto repeat = repeating.find(timer);
    if (repeat != repeating.end()) repeat->second->Stop();
  }

  void Reply(int id, int value) {
    if (value < 0) std::abort();
    Emit(id, 0);
  }
};

Bound::Bound(Context* context, int runner, int construct, int destroy)
    : context(context), runner(runner), destroy(destroy) { context->Emit(construct, runner); }
Bound::~Bound() { context->Emit(destroy, runner); }
void Bound::Hit(int id) { context->Emit(id, runner); }
int Bound::HitAndReturn(int id) { context->Emit(id, runner); return id; }
WeakTarget::WeakTarget(Context* context, int id) : context(context), id(id) {}
void WeakTarget::Run() { context->Emit(id, 0); }

base::TaskPriority Priority(int value) {
  if (value == 0) return base::TaskPriority::BEST_EFFORT;
  if (value == 1) return base::TaskPriority::USER_VISIBLE;
  return base::TaskPriority::USER_BLOCKING;
}

base::TaskShutdownBehavior Shutdown(int value) {
  if (value == 0) return base::TaskShutdownBehavior::CONTINUE_ON_SHUTDOWN;
  if (value == 1) return base::TaskShutdownBehavior::SKIP_ON_SHUTDOWN;
  return base::TaskShutdownBehavior::BLOCK_SHUTDOWN;
}

void ProbeShutdown(Context& context, int runner, int id) {
  const auto behavior = Shutdown(context.runners.at(runner).shutdown);
  base::internal::TaskTracker tracker;
  bool ran = false;
  base::internal::Task task(FROM_HERE,
      base::BindOnce([](bool* value) { *value = true; }, base::Unretained(&ran)),
      base::TimeTicks::Now(), base::TimeDelta());
  if (!tracker.WillPostTask(&task, behavior)) std::abort();
  auto sequence = base::MakeRefCounted<base::internal::Sequence>(
      base::TaskTraits{behavior}, nullptr, base::internal::TaskSourceExecutionMode::kParallel,
      base::ThreadType::kDefault);
  {
    auto transaction = sequence->BeginTransaction();
    transaction.WillPushImmediateTask();
    transaction.PushImmediateTask(std::move(task));
  }
  auto source = tracker.RegisterTaskSource(std::move(sequence));
  if (!source) std::abort();
  tracker.StartShutdown();
  source.WillRunTask();
  if (tracker.RunAndPopNextTask(std::move(source))) std::abort();
  tracker.CompleteShutdown();
  std::lock_guard<std::mutex> guard(context.mutex);
  context.trace.push_back("Z " + std::to_string(id) + " " + std::to_string(ran) + "\n");
}

void ProbeConcurrency(Context& context, base::test::TaskEnvironment& environment, int first, int second) {
  std::barrier<> barrier(2);
  auto wait = [](std::barrier<>* value) {
    base::ScopedBlockingCall blocking(FROM_HERE, base::BlockingType::MAY_BLOCK);
    value->arrive_and_wait();
  };
  context.runners.at(first).task_runner->PostTask(FROM_HERE,
      base::BindOnce(wait, base::Unretained(&barrier)));
  context.runners.at(second).task_runner->PostTask(FROM_HERE,
      base::BindOnce(wait, base::Unretained(&barrier)));
  environment.RunUntilIdle();
  std::lock_guard<std::mutex> guard(context.mutex);
  context.trace.push_back("G 1\n");
}

std::string Execute(const std::string& script) {
  Context context;
  {
    base::test::TaskEnvironment environment(
        base::test::TaskEnvironment::TimeSource::MOCK_TIME,
        base::test::TaskEnvironment::ThreadPoolExecutionMode::QUEUED);
    context.origin = base::TimeTicks::Now();
    context.runners.emplace(0, Runner{base::SingleThreadTaskRunner::GetCurrentDefault(), 0, 1});
    std::istringstream input(script);
    std::string line;
    while (std::getline(input, line)) {
      std::istringstream command(line);
      char op;
      if (!(command >> op)) continue;
      int a, b, c, d, e, f, g;
      switch (op) {
        case 'R': {
          command >> a >> b >> c >> d;
          base::TaskTraits traits{Priority(c), Shutdown(d), base::MayBlock()};
          scoped_refptr<base::SequencedTaskRunner> runner;
          if (b == 1) runner = base::ThreadPool::CreateSequencedTaskRunner(traits);
          else runner = base::ThreadPool::CreateSingleThreadTaskRunner(traits, base::SingleThreadTaskRunnerThreadMode::DEDICATED);
          context.runners.emplace(a, Runner{std::move(runner), b, d});
          break;
        }
        case 'P':
          command >> a >> b >> c >> d >> e >> f >> g;
          context.Post(a, b, c, d, e, f, g);
          break;
        case 'L':
          command >> a >> b >> c >> d >> e >> f;
          context.handles.emplace(a, context.runners.at(0).task_runner->PostCancelableDelayedTask(
              base::subtle::PostDelayedTaskPassKeyForTesting(), FROM_HERE,
              base::BindOnce(&Context::Run, base::Unretained(&context), 0, a, c, d, e, f),
              base::Milliseconds(b)));
          break;
        case 'C': command >> a; context.Cancel(a); break;
        case 'K':
          command >> a >> b;
          context.tracked_pending[b] = true;
          context.tracked.emplace(b, context.tracker.PostTask(context.runners.at(a).task_runner.get(), FROM_HERE,
              base::BindOnce(&Context::RunTracked, base::Unretained(&context), a, b)));
          break;
        case 'Y':
          command >> a >> b >> c;
          context.tracked_pending[b] = true;
          context.tracked.emplace(b, context.tracker.PostTaskAndReply(context.runners.at(a).task_runner.get(), FROM_HERE,
              base::BindOnce(&Context::RunTrackedReply, base::Unretained(&context), a, b),
              base::BindOnce(&Context::FinishTrackedReply, base::Unretained(&context), b, c)));
          break;
        case 'X': command >> a; context.CancelTracked(a); break;
        case 'U': {
          command >> a;
          base::CancelableTaskTracker::IsCanceledCallback callback;
          context.tracked.emplace(a, context.tracker.NewTrackedTaskId(&callback));
          context.tracked_pending[a] = true;
          context.tracked_callbacks.emplace(a, std::move(callback));
          break;
        }
        case 'O':
          command >> a;
          context.CheckCancellation(a, context.tracked_callbacks.at(a));
          break;
        case 'N':
          command >> a >> b;
          context.runners.at(a).task_runner->PostTask(FROM_HERE,
              base::BindOnce(&Context::CheckCancellation, base::Unretained(&context), b,
                             context.tracked_callbacks.at(b)));
          break;
        case 'W': {
          command >> a >> b;
          auto target = std::make_unique<WeakTarget>(&context, a);
          auto pointer = target->weak_factory.GetWeakPtr();
          context.weak.emplace(a, std::move(target));
          context.runners.at(0).task_runner->PostDelayedTask(FROM_HERE,
              base::BindOnce(&WeakTarget::Run, pointer), base::Milliseconds(b));
          break;
        }
        case 'V': command >> a; context.weak.at(a)->weak_factory.InvalidateWeakPtrs(); break;
        case 'Q':
          command >> a >> b >> c;
          context.runners.at(a).task_runner->PostTaskAndReply(FROM_HERE,
              base::BindOnce(&Context::Emit, base::Unretained(&context), b, a),
              base::BindOnce(&Context::Emit, base::Unretained(&context), c, 0));
          break;
        case 'T': command >> a >> b >> c >> d; context.StartTimer(a, b, c, d); break;
        case 'S': command >> a; context.StopTimer(a); break;
        case 'Z': command >> a >> b; ProbeShutdown(context, a, b); break;
        case 'G': command >> a >> b; ProbeConcurrency(context, environment, a, b); break;
        case 'B':
          command >> a >> b >> c >> d;
          context.bound.emplace(a, std::make_unique<base::SequenceBound<Bound>>(
              context.runners.at(b).task_runner, &context, b, c, d));
          break;
        case 'F': command >> a >> b; context.bound.at(a)->AsyncCall(&Bound::Hit).WithArgs(b); break;
        case 'J':
          command >> a >> b >> c;
          context.bound.at(a)->AsyncCall(&Bound::HitAndReturn).WithArgs(b).Then(
              base::BindOnce(&Context::Reply, base::Unretained(&context), c));
          break;
        case 'D': command >> a; context.bound.erase(a); break;
        case 'A': command >> a; environment.FastForwardBy(base::Milliseconds(a)); break;
        case 'I': environment.RunUntilIdle(); break;
        case 'H': goto finish;
        default: std::abort();
      }
      if (command.fail()) std::abort();
    }
finish:
    context.bound.clear();
    context.once.clear();
    context.repeating.clear();
    context.handles.clear();
  }
  std::string result;
  for (const auto& line : context.trace) result += line;
  return result;
}

bool ReadExact(char* buffer, size_t size) {
  std::cin.read(buffer, size);
  return static_cast<size_t>(std::cin.gcount()) == size;
}

int main(int argc, char** argv) {
  base::CommandLine::Init(argc, argv);
  TestTimeouts::Initialize();
  char magic[4];
  if (!ReadExact(magic, 4) || std::string(magic, 4) != "MGB1") return 1;
  std::cout.write("MGB1", 4).flush();
  for (;;) {
    std::cin.read(magic, 4);
    if (std::cin.gcount() == 0 && std::cin.eof()) return 0;
    if (std::cin.gcount() != 4) return 1;
    uint32_t size = (static_cast<uint32_t>(static_cast<uint8_t>(magic[0])) << 24) |
                    (static_cast<uint32_t>(static_cast<uint8_t>(magic[1])) << 16) |
                    (static_cast<uint32_t>(static_cast<uint8_t>(magic[2])) << 8) |
                    static_cast<uint8_t>(magic[3]);
    std::string input(size, '\0');
    if (!ReadExact(input.data(), size)) return 1;
    std::string output = Execute(input);
    uint32_t length = output.size();
    char header[4] = {static_cast<char>(length >> 24), static_cast<char>(length >> 16),
                      static_cast<char>(length >> 8), static_cast<char>(length)};
    std::cout.write(header, 4).write(output.data(), output.size()).flush();
  }
}
